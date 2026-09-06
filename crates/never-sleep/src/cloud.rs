#[path = "cloud_socket.rs"]
mod socket;

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

use never_sleep_core::{
    apply_remote_command, device_credentials_are_valid, identity_from_bytes, CloudIdentity, Engine,
    JsonStatus, Lang, RemoteCommand, PAIRING_TTL_SECS, PUBLIC_SITE_ORIGIN,
};
use serde::{Deserialize, Serialize};

use crate::apply::apply_effects_or_abort;
use crate::paths::{cloud_identity_path, ensure_data_dir};
use crate::platform::Platform;

pub const CLOUD_URL_ENV: &str = "NEVER_SLEEP_CLOUD_URL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudEvent {
    Pairing {
        code: String,
        url: String,
        expires_unix: u64,
    },
    PairingCleared,
    Commands(Vec<RemoteCommand>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReporterWake {
    Snapshot,
    Shutdown,
    /// Stop the loop without an offline heartbeat (live handoff to the menu).
    Detach,
    /// Finish the in-flight POST, then park until Detach or Shutdown.
    Quiesce,
}

pub struct CloudHandle {
    /// Latest-wins snapshot. A capacity-one wake channel only signals that
    /// `latest` changed; it must not carry the payload or a newer inactive
    /// status is dropped while the reporter is in a slow POST.
    latest: Arc<Mutex<Option<(JsonStatus, Lang)>>>,
    wake: Option<SyncSender<ReporterWake>>,
    events: mpsc::Receiver<CloudEvent>,
    join: Option<thread::JoinHandle<()>>,
    held_commands: Mutex<Vec<RemoteCommand>>,
    applied_ids: Arc<Mutex<Vec<String>>>,
    /// Ids this process applied (or the donor already applied). Pruned after
    /// the Worker drops them from pending so a long session cannot grow forever.
    applied_history: Arc<Mutex<Vec<String>>>,
    /// Set before the wake sender is dropped so a full channel cannot lose Detach.
    detached: Arc<AtomicBool>,
    /// Stop POSTing so handoff can drain the last commands without a racing ack.
    paused: Arc<AtomicBool>,
    /// Keep applied ids for the successor even after the Worker drops them.
    retain_applied: Arc<AtomicBool>,
    idle: Arc<(Mutex<bool>, Condvar)>,
    /// Last successfully parsed Worker pending-command ids. `None` until the
    /// reporter has seen a heartbeat; `Some([])` means the Worker has none.
    last_pending: Arc<Mutex<Option<Vec<String>>>>,
}

impl CloudHandle {
    /// True only while the reporter thread actually started.
    #[cfg(target_os = "macos")]
    pub fn reporter_is_running(&self) -> bool {
        self.join.is_some()
    }

    #[cfg(any(test, target_os = "macos"))]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
    pub fn push_status(&self, status: JsonStatus, lang: Lang) {
        self.queue_latest(status, lang);
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(ReporterWake::Snapshot);
        }
    }

    fn queue_latest(&self, status: JsonStatus, lang: Lang) {
        if let Ok(mut slot) = self.latest.lock() {
            *slot = Some((status, lang));
        }
    }

    /// Disconnect the reporter and wait until it has POSTed the latest snapshot.
    pub fn flush_and_join(mut self) {
        self.disconnect_and_join();
    }

    /// Stop heartbeats without marking the Mac offline. The menu reporter owns
    /// the next POST.
    pub fn detach(mut self) {
        self.detached.store(true, Ordering::SeqCst);
        self.paused.store(true, Ordering::SeqCst);
        self.notify_idle();
        if let Some(wake) = self.wake.take() {
            let _ = wake.try_send(ReporterWake::Detach);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    /// Wait until the reporter has finished its in-flight POST (or there is no
    /// live thread). Further heartbeats stay parked until `detach` / shutdown.
    pub fn quiesce(&self) {
        self.retain_applied.store(true, Ordering::SeqCst);
        if self.join.is_none() {
            return;
        }
        self.paused.store(true, Ordering::SeqCst);
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(ReporterWake::Quiesce);
        }
        let (lock, cv) = &*self.idle;
        let started = std::time::Instant::now();
        let Ok(mut idle) = lock.lock() else {
            return;
        };
        while !*idle {
            let wait = Duration::from_secs(8).saturating_sub(started.elapsed());
            if wait.is_zero() {
                break;
            }
            let (guard, result) = cv
                .wait_timeout(idle, wait)
                .unwrap_or_else(|e| e.into_inner());
            idle = guard;
            if result.timed_out() {
                break;
            }
        }
    }

    /// Unpark heartbeats after a handoff that was not accepted.
    pub fn resume(&self) {
        forget_pending_ids(&self.last_pending);
        if let Ok(mut idle) = self.idle.0.lock() {
            *idle = false;
        }
        self.paused.store(false, Ordering::SeqCst);
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(ReporterWake::Snapshot);
        }
    }

    /// Allow history prune after the successor is gone (or never existed).
    pub fn release_applied_retention(&self) {
        self.retain_applied.store(false, Ordering::SeqCst);
    }

    fn notify_idle(&self) {
        signal_reporter_idle(&self.idle);
    }

    fn mark_applied(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        if let Ok(mut slot) = self.applied_ids.lock() {
            slot.extend(ids.iter().cloned());
        }
        if let Ok(mut hist) = self.applied_history.lock() {
            for id in &ids {
                if !hist.iter().any(|seen| seen == id) {
                    hist.push(id.clone());
                }
            }
        }
    }

    pub fn applied_command_ids(&self) -> Vec<String> {
        self.applied_history
            .lock()
            .map(|hist| hist.clone())
            .unwrap_or_default()
    }

    /// Drop held commands the donor already applied, and ack them so the Worker
    /// does not keep replaying the same On after this process takes over.
    #[cfg(any(test, target_os = "macos"))]
    pub fn skip_applied(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        if let Ok(mut held) = self.held_commands.lock() {
            held.retain(|cmd| !ids.iter().any(|id| id == &cmd.id));
        }
        self.mark_applied(ids);
    }

    fn already_applied(&self, id: &str) -> bool {
        self.applied_history
            .lock()
            .map(|hist| hist.iter().any(|seen| seen == id))
            .unwrap_or(false)
    }

    #[cfg(test)]
    fn take_applied(&self) -> Vec<String> {
        self.applied_ids
            .lock()
            .map(|mut slot| std::mem::take(&mut *slot))
            .unwrap_or_default()
    }

    fn disconnect_and_join(&mut self) {
        if let Some(wake) = self.wake.take() {
            // try_send so Drop cannot deadlock when the reporter is gone or
            // the channel already holds coalesced snapshot wakes.
            let _ = wake.try_send(ReporterWake::Shutdown);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    #[cfg(test)]
    fn queued_status(&self) -> Option<JsonStatus> {
        clone_latest(&self.latest).map(|(status, _)| status)
    }

    pub fn poll_events(&self) -> Vec<CloudEvent> {
        let mut out = Vec::new();
        if let Ok(mut held) = self.held_commands.lock() {
            if !held.is_empty() {
                out.push(CloudEvent::Commands(std::mem::take(&mut *held)));
            }
        }
        while let Ok(ev) = self.events.try_recv() {
            out.push(ev);
        }
        out
    }

    fn hold_commands(&self, commands: Vec<RemoteCommand>) {
        if commands.is_empty() {
            return;
        }
        if let Ok(mut held) = self.held_commands.lock() {
            held.extend(commands);
        }
    }

    fn last_pending_ids(&self) -> Option<Vec<String>> {
        self.last_pending.lock().ok().and_then(|slot| slot.clone())
    }

    #[cfg(test)]
    fn note_pending(&self, pending: &[RemoteCommand]) {
        store_pending_ids(&self.last_pending, pending);
    }
}

impl Drop for CloudHandle {
    fn drop(&mut self) {
        self.disconnect_and_join();
    }
}

/// Queue a snapshot and wait until the reporter has POSTed it (or disconnected).
pub fn publish_and_flush(handle: CloudHandle, status: JsonStatus, lang: Lang) {
    handle.queue_latest(status, lang);
    handle.flush_and_join();
}

/// Retry pair/start until it succeeds, and again after unauthorized heartbeats
/// or an expired pairing offer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReporterGate {
    registered: bool,
}

impl ReporterGate {
    pub fn needs_pair_start(&self) -> bool {
        !self.registered
    }

    pub fn on_pair_start_ok(&mut self) {
        self.registered = true;
    }

    pub fn on_unauthorized(&mut self) {
        self.registered = false;
    }

    pub fn on_pairing_cleared(&mut self) {
        self.registered = false;
    }
}

/// Dedup remote commands by id and remember ids to ack on the next heartbeat.
#[derive(Debug, Default)]
pub struct CommandInbox {
    seen: Vec<String>,
    known: HashSet<String>,
}

impl CommandInbox {
    pub fn take_new(&mut self, commands: Vec<RemoteCommand>) -> Vec<RemoteCommand> {
        let mut out = Vec::new();
        for cmd in commands {
            if self.known.insert(cmd.id.clone()) {
                out.push(cmd);
            }
        }
        out
    }

    /// Record ids this process applied, including ids a donor already applied
    /// that this inbox has not listed yet. Those still go out as acks so the
    /// Worker drops them before a relaunch can replay a timed On.
    pub fn mark_applied<I, S>(&mut self, ids: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for id in ids {
            let id = id.as_ref();
            self.known.insert(id.to_string());
            if !self.seen.iter().any(|seen| seen == id) {
                self.seen.push(id.to_string());
            }
        }
    }

    /// Keep every delivered id the Worker still lists. Call after a successful
    /// heartbeat so acked commands leave the inbox; never drop still-pending ids
    /// (a size cap here would replay on/off after the next beat).
    pub fn retain_pending(&mut self, pending: &[RemoteCommand]) {
        let keep: HashSet<&str> = pending.iter().map(|c| c.id.as_str()).collect();
        self.seen.retain(|id| keep.contains(id.as_str()));
        self.known.retain(|id| keep.contains(id.as_str()));
    }

    pub fn ack_ids(&self) -> &[String] {
        &self.seen
    }
}

pub fn cloud_origin() -> String {
    std::env::var(CLOUD_URL_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| PUBLIC_SITE_ORIGIN.to_string())
}

pub fn cloud_enabled(config: &never_sleep_core::AppConfig) -> bool {
    config.remote_enabled && (cfg!(target_os = "macos") || std::env::var(CLOUD_URL_ENV).is_ok())
}

#[cfg(any(test, target_os = "macos"))]
pub fn reconcile_remote(
    config: &never_sleep_core::AppConfig,
    ipc_owned: bool,
    cloud: &mut Option<CloudHandle>,
    identity: &mut Option<never_sleep_core::CloudIdentity>,
    pairing: &mut Option<(String, String, u64)>,
) {
    if !ipc_owned || !cloud_enabled(config) {
        // Detach stops the transport without scheduling a final HTTP heartbeat.
        if let Some(handle) = cloud.take() {
            handle.detach();
        }
        *identity = None;
        *pairing = None;
        return;
    }
    if cloud.is_none() {
        match load_or_create_identity() {
            Ok(id) => {
                *cloud = Some(spawn_reporter_paused(
                    id.clone(),
                    default_display_name(),
                    config.lang(),
                    crate::session_lock::should_pause_menu_reporter(
                        crate::session_lock::peer_reporter_lock_is_live(std::process::id()),
                    ),
                ));
                *identity = Some(id);
            }
            Err(err) => eprintln!("never-sleep cloud identity: {err}"),
        }
    }
}

/// Phone-board cards and localStorage reservations share this cap.
pub const MAX_DISPLAY_NAME_CHARS: usize = 128;

pub fn bound_display_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "Mac".into();
    }
    trimmed.chars().take(MAX_DISPLAY_NAME_CHARS).collect()
}

pub fn default_display_name() -> String {
    if let Ok(name) = std::env::var("NEVER_SLEEP_DEVICE_NAME") {
        if !name.trim().is_empty() {
            return bound_display_name(&name);
        }
    }
    bound_display_name(&hostname_from_os().unwrap_or_else(|| "Mac".into()))
}

/// Parse a gethostname / C-string buffer. Used so tests can lock the Mac name
/// path without touching /etc/hostname.
pub fn hostname_from_c_buffer(buf: &[u8]) -> Option<String> {
    let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let s = std::str::from_utf8(&buf[..nul]).ok()?.trim();
    if s.is_empty() {
        return None;
    }
    Some(s.to_string())
}

fn hostname_from_os() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: POSIX gethostname writes a NUL-terminated name into `buf`.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if rc == 0 {
            return hostname_from_c_buffer(&buf);
        }
    }
    None
}

fn fill_random(buf: &mut [u8]) {
    let mut file = fs::File::open("/dev/urandom").expect("urandom");
    file.read_exact(buf).expect("urandom read");
}

pub fn load_or_create_identity() -> io::Result<CloudIdentity> {
    let path = cloud_identity_path();
    if let Some(id) = read_complete_identity(&path)? {
        return Ok(id);
    }
    let mut id_bytes = [0u8; 16];
    let mut token_bytes = [0u8; 32];
    fill_random(&mut id_bytes);
    fill_random(&mut token_bytes);
    let identity = identity_from_bytes(&id_bytes, &token_bytes);
    match save_identity(&identity) {
        Ok(()) => Ok(identity),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            if let Some(id) = wait_for_complete_identity(&path)? {
                return Ok(id);
            }
            recover_stranded_identity(&path, identity)
        }
        Err(err) => Err(err),
    }
}

fn read_complete_identity(path: &Path) -> io::Result<Option<CloudIdentity>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if let Ok(id) = toml::from_str::<CloudIdentity>(&text) {
        if device_credentials_are_valid(&id.device_id, &id.device_token) {
            restrict_owner_only(path)?;
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn wait_for_complete_identity(path: &Path) -> io::Result<Option<CloudIdentity>> {
    for _ in 0..40 {
        if let Some(id) = read_complete_identity(path)? {
            return Ok(Some(id));
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(None)
}

fn recover_stranded_identity(path: &Path, identity: CloudIdentity) -> io::Result<CloudIdentity> {
    if let Some(id) = read_complete_identity(path)? {
        return Ok(id);
    }
    let mut file = match fs::OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return match save_identity(&identity) {
                Ok(()) => Ok(identity),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    wait_for_complete_identity(path)?.ok_or(err)
                }
                Err(err) => Err(err),
            };
        }
        Err(err) => return Err(err),
    };
    // SAFETY: `file` is an open fd we own for the duration of this recover;
    // flock(LOCK_EX) is the POSIX exclusive lock used to serialize stranded
    // cloud.toml replacement.
    let lock = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if lock != 0 {
        return Err(io::Error::last_os_error());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    if let Ok(id) = toml::from_str::<CloudIdentity>(&text) {
        if device_credentials_are_valid(&id.device_id, &id.device_token) {
            restrict_owner_only(path)?;
            return Ok(id);
        }
    }
    let text = toml::to_string_pretty(&identity)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    restrict_owner_only(path)?;
    Ok(identity)
}

fn restrict_owner_only(path: &Path) -> io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)
}

fn write_owner_only(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_file_name(format!(
        "{}.tmp.{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("cloud.toml"),
        std::process::id()
    ));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    restrict_owner_only(&tmp)?;
    let linked = fs::hard_link(&tmp, path);
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => restrict_owner_only(path),
        Err(err) => Err(err),
    }
}

fn save_identity(identity: &CloudIdentity) -> io::Result<()> {
    ensure_data_dir()?;
    let text = toml::to_string_pretty(identity)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    write_owner_only(&cloud_identity_path(), text.as_bytes())
}

#[derive(Debug, Serialize)]
struct PairStartBody<'a> {
    device_id: &'a str,
    device_token: &'a str,
    display_name: &'a str,
    lang: &'a str,
}

#[derive(Debug, Serialize)]
struct HeartbeatBody<'a> {
    device_id: &'a str,
    device_token: &'a str,
    display_name: &'a str,
    lang: &'a str,
    ack_command_ids: &'a [String],
    status: &'a JsonStatus,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    offline: bool,
}

#[derive(Debug, Deserialize)]
struct PairStartResponse {
    ok: bool,
    #[serde(default)]
    pairing_code: Option<String>,
    #[serde(default)]
    pairing_url: Option<String>,
    #[serde(default)]
    expires_unix: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct HeartbeatResponse {
    ok: bool,
    #[serde(default)]
    pairing_code: Option<String>,
    #[serde(default)]
    pairing_url: Option<String>,
    #[serde(default)]
    expires_unix: Option<u64>,
    #[serde(default)]
    commands: Vec<RemoteCommand>,
}

pub fn heartbeat_request_json(
    identity: &CloudIdentity,
    display_name: &str,
    status: &JsonStatus,
    lang: &str,
    ack_command_ids: &[String],
    offline: bool,
) -> String {
    serde_json::to_string(&HeartbeatBody {
        device_id: &identity.device_id,
        device_token: &identity.device_token,
        display_name,
        lang,
        ack_command_ids,
        status,
        offline,
    })
    .expect("heartbeat json")
}

pub fn parse_heartbeat_response(raw: &str) -> Result<HeartbeatOutcome, String> {
    let parsed: HeartbeatResponse = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if !parsed.ok {
        return Err("heartbeat rejected".into());
    }
    Ok(HeartbeatOutcome {
        pairing_code: parsed.pairing_code,
        pairing_url: parsed.pairing_url,
        expires_unix: parsed.expires_unix,
        commands: parsed
            .commands
            .into_iter()
            .filter(|c| RemoteCommand::is_allowed_cmd(&c.cmd))
            .collect(),
    })
}

#[derive(Debug, Clone)]
pub struct HeartbeatOutcome {
    pub pairing_code: Option<String>,
    pub pairing_url: Option<String>,
    pub expires_unix: Option<u64>,
    pub commands: Vec<RemoteCommand>,
}

impl HeartbeatOutcome {
    pub fn pairing_present(&self) -> bool {
        self.pairing_code.is_some() && self.pairing_url.is_some()
    }

    pub fn pairing_cleared(&self) -> bool {
        !self.pairing_present()
    }
}

pub fn parse_pair_start_response(raw: &str) -> Result<(String, String, u64), String> {
    let parsed: PairStartResponse = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    match (parsed.ok, parsed.pairing_code, parsed.pairing_url) {
        (true, Some(code), Some(url)) => {
            let expires_unix = parsed
                .expires_unix
                .unwrap_or_else(|| unix_now_secs() + PAIRING_TTL_SECS);
            Ok((code, url, expires_unix))
        }
        _ => Err("pair start rejected".into()),
    }
}

pub fn apply_cloud_commands(
    engine: &mut Engine,
    platform: &mut dyn Platform,
    commands: &[RemoteCommand],
) {
    for cmd in commands {
        let host = platform.snapshot();
        if let Ok(effects) = apply_remote_command(engine, &host, cmd) {
            apply_effects_or_abort(engine, platform, &effects);
        }
    }
}

#[derive(Debug)]
pub(crate) enum CloudPost {
    Ok(String),
    Unauthorized,
}

pub(crate) trait CloudTransport {
    fn post_json(&self, path: &str, body: &str) -> Result<CloudPost, String>;
}

pub fn spawn_reporter(identity: CloudIdentity, display_name: String, lang: Lang) -> CloudHandle {
    spawn_reporter_paused(identity, display_name, lang, false)
}

pub fn spawn_reporter_paused(
    identity: CloudIdentity,
    display_name: String,
    _lang: Lang,
    start_paused: bool,
) -> CloudHandle {
    let latest = Arc::new(Mutex::new(None));
    let detached = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(start_paused));
    let retain_applied = Arc::new(AtomicBool::new(false));
    let idle = Arc::new((Mutex::new(false), Condvar::new()));
    let applied_ids = Arc::new(Mutex::new(Vec::new()));
    let applied_history = Arc::new(Mutex::new(Vec::new()));
    let last_pending = Arc::new(Mutex::new(None));
    let (wake_tx, wake_rx) = mpsc::sync_channel(2);
    let (event_tx, event_rx) = mpsc::channel();
    let latest_for_thread = Arc::clone(&latest);
    let detached_for_thread = Arc::clone(&detached);
    let paused_for_thread = Arc::clone(&paused);
    let retain_for_thread = Arc::clone(&retain_applied);
    let idle_for_thread = Arc::clone(&idle);
    let applied_for_thread = Arc::clone(&applied_ids);
    let history_for_thread = Arc::clone(&applied_history);
    let pending_for_thread = Arc::clone(&last_pending);
    let join = thread::Builder::new()
        .name("never-sleep-cloud".into())
        .spawn(move || {
            reporter_loop(
                identity,
                display_name,
                UreqTransport {
                    origin: cloud_origin(),
                },
                wake_rx,
                latest_for_thread,
                event_tx,
                detached_for_thread,
                paused_for_thread,
                retain_for_thread,
                idle_for_thread,
                applied_for_thread,
                history_for_thread,
                pending_for_thread,
            );
        })
        .ok();
    CloudHandle {
        latest,
        wake: Some(wake_tx),
        events: event_rx,
        join,
        held_commands: Mutex::new(Vec::new()),
        applied_ids,
        applied_history,
        detached,
        paused,
        retain_applied,
        idle,
        last_pending,
    }
}

#[cfg(test)]
fn clone_latest(latest: &Mutex<Option<(JsonStatus, Lang)>>) -> Option<(JsonStatus, Lang)> {
    latest.lock().ok().and_then(|slot| slot.clone())
}

fn take_latest(latest: &Mutex<Option<(JsonStatus, Lang)>>) -> Option<(JsonStatus, Lang)> {
    latest.lock().ok().and_then(|mut slot| slot.take())
}

// Independent of the local policy/UI clock. Keep below the Worker's 35s TTL.
const CLOUD_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

fn live_status_changed(before: &JsonStatus, after: &JsonStatus, elapsed: u64) -> bool {
    let mut projected = before.clone();
    projected.elapsed_secs = before.elapsed_secs.map(|n| n.saturating_add(elapsed));
    projected.remaining_secs = before.remaining_secs.map(|n| n.saturating_sub(elapsed));
    for (expected, actual) in [
        (projected.elapsed_secs, after.elapsed_secs),
        (projected.remaining_secs, after.remaining_secs),
    ] {
        match (expected, actual) {
            (Some(a), Some(b)) if a.abs_diff(b) <= 2 => {}
            (None, None) => {}
            _ => return true,
        }
    }
    projected.elapsed_secs = after.elapsed_secs;
    projected.remaining_secs = after.remaining_secs;
    projected != *after
}

fn heartbeat_due(elapsed: Duration, shutting_down: bool) -> bool {
    shutting_down || elapsed >= CLOUD_HEARTBEAT_INTERVAL
}

fn should_reporter_tick(has_snapshot: bool, shutting_down: bool) -> bool {
    has_snapshot || shutting_down
}

fn shutting_down_from_wake(
    recv: Result<ReporterWake, RecvTimeoutError>,
    drained_shutdown: bool,
) -> bool {
    match recv {
        Ok(ReporterWake::Shutdown) | Ok(ReporterWake::Detach) => true,
        Ok(ReporterWake::Snapshot) | Ok(ReporterWake::Quiesce) => drained_shutdown,
        Err(RecvTimeoutError::Timeout) => drained_shutdown,
        Err(RecvTimeoutError::Disconnected) => true,
    }
}

fn drain_reporter_wakes(rx: &mpsc::Receiver<ReporterWake>) -> (bool, bool) {
    let mut shutdown = false;
    let mut detach = false;
    loop {
        match rx.try_recv() {
            Ok(ReporterWake::Shutdown) => shutdown = true,
            Ok(ReporterWake::Detach) => detach = true,
            Ok(ReporterWake::Snapshot) | Ok(ReporterWake::Quiesce) => {}
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                shutdown = true;
                break;
            }
        }
    }
    (shutdown, detach)
}

fn reporter_marks_offline(
    recv: Result<ReporterWake, RecvTimeoutError>,
    drained_shutdown: bool,
    drained_detach: bool,
) -> bool {
    match recv {
        Ok(ReporterWake::Shutdown) => true,
        Ok(ReporterWake::Detach) => false,
        Ok(ReporterWake::Snapshot) | Ok(ReporterWake::Quiesce) => drained_shutdown,
        Err(RecvTimeoutError::Timeout) => drained_shutdown,
        Err(RecvTimeoutError::Disconnected) => !drained_detach,
    }
}

fn reporter_goes_offline(
    recv: Result<ReporterWake, RecvTimeoutError>,
    drained_shutdown: bool,
    drained_detach: bool,
    detached: bool,
) -> bool {
    if detached {
        return false;
    }
    reporter_marks_offline(recv, drained_shutdown, drained_detach)
}

fn signal_reporter_idle(idle: &Arc<(Mutex<bool>, Condvar)>) {
    if let Ok(mut flag) = idle.0.lock() {
        *flag = true;
        idle.1.notify_all();
    }
}

enum QuiescePark {
    Break,
    Shutdown,
    Resume,
}

fn park_quiesced_reporter(
    wake_rx: &mpsc::Receiver<ReporterWake>,
    detached: &AtomicBool,
    paused: &AtomicBool,
    idle: &Arc<(Mutex<bool>, Condvar)>,
) -> QuiescePark {
    signal_reporter_idle(idle);
    loop {
        if detached.load(Ordering::SeqCst) {
            return QuiescePark::Break;
        }
        if !paused.load(Ordering::SeqCst) {
            return QuiescePark::Resume;
        }
        match wake_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ReporterWake::Shutdown) => return QuiescePark::Shutdown,
            Ok(ReporterWake::Detach) => return QuiescePark::Break,
            Ok(_) => signal_reporter_idle(idle),
            Err(RecvTimeoutError::Disconnected) => {
                return if detached.load(Ordering::SeqCst) {
                    QuiescePark::Break
                } else {
                    QuiescePark::Shutdown
                };
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn take_applied_ids(slot: &Mutex<Vec<String>>) -> Vec<String> {
    slot.lock()
        .map(|mut ids| std::mem::take(&mut *ids))
        .unwrap_or_default()
}

fn prune_applied_history_after_heartbeat(
    hist: &Mutex<Vec<String>>,
    pending: Option<&[RemoteCommand]>,
    queued: &Mutex<Vec<String>>,
    retain_for_successor: bool,
) {
    if !should_prune_applied_history(pending.is_some(), retain_for_successor) {
        return;
    }
    let Some(pending) = pending else {
        return;
    };
    prune_applied_history(hist, pending, queued);
}

fn should_prune_applied_history(parsed_pending: bool, retain_for_successor: bool) -> bool {
    parsed_pending && !retain_for_successor
}

pub(crate) fn should_release_applied_retention(successor_live: bool) -> bool {
    !successor_live
}

fn should_pair_start_after_heartbeat(
    needs_pair: bool,
    offline: bool,
    heartbeat_resolved: bool,
) -> bool {
    needs_pair && !offline && heartbeat_resolved
}

fn prune_applied_history(
    hist: &Mutex<Vec<String>>,
    pending: &[RemoteCommand],
    queued: &Mutex<Vec<String>>,
) {
    let keep_queued: Vec<String> = queued.lock().map(|ids| ids.clone()).unwrap_or_default();
    let keep: HashSet<&str> = pending
        .iter()
        .map(|c| c.id.as_str())
        .chain(keep_queued.iter().map(String::as_str))
        .collect();
    if let Ok(mut hist) = hist.lock() {
        hist.retain(|id| keep.contains(id.as_str()));
    }
}

#[allow(clippy::too_many_arguments)]
fn reporter_loop(
    identity: CloudIdentity,
    display_name: String,
    transport: UreqTransport,
    wake_rx: mpsc::Receiver<ReporterWake>,
    latest: Arc<Mutex<Option<(JsonStatus, Lang)>>>,
    event_tx: mpsc::Sender<CloudEvent>,
    detached: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    retain_applied: Arc<AtomicBool>,
    idle: Arc<(Mutex<bool>, Condvar)>,
    applied_ids: Arc<Mutex<Vec<String>>>,
    applied_history: Arc<Mutex<Vec<String>>>,
    last_pending: Arc<Mutex<Option<Vec<String>>>>,
) {
    let mut gate = ReporterGate::default();
    let mut inbox = CommandInbox::default();
    let mut last: Option<(JsonStatus, Lang)> = None;
    let mut last_attempt: Option<Instant> = None;
    let mut live = socket::LiveSocket::new();
    loop {
        if paused.load(Ordering::SeqCst) && !detached.load(Ordering::SeqCst) {
            for raw in live.read_pending() {
                if let Ok(outcome) = parse_heartbeat_response(&raw) {
                    emit_outcome(&mut gate, &mut inbox, &event_tx, &last_pending, outcome);
                }
            }
            live.disconnect();
            match park_quiesced_reporter(&wake_rx, &detached, &paused, &idle) {
                QuiescePark::Break => break,
                QuiescePark::Shutdown => {}
                QuiescePark::Resume => {}
            }
        }
        let shutting_down = {
            let recv = if paused.load(Ordering::SeqCst) && detached.load(Ordering::SeqCst) {
                Ok(ReporterWake::Detach)
            } else if paused.load(Ordering::SeqCst) {
                Ok(ReporterWake::Shutdown)
            } else {
                let wait = last_attempt.map_or(CLOUD_HEARTBEAT_INTERVAL, |at| {
                    CLOUD_HEARTBEAT_INTERVAL.saturating_sub(at.elapsed())
                });
                wake_rx.recv_timeout(if live.connected() {
                    Duration::from_millis(200)
                } else {
                    wait
                })
            };
            let (drained_shutdown, drained_detach) = drain_reporter_wakes(&wake_rx);
            let shutting_down = shutting_down_from_wake(recv, drained_shutdown || drained_detach);
            let offline = reporter_goes_offline(
                recv,
                drained_shutdown,
                drained_detach,
                detached.load(Ordering::SeqCst) || drained_detach,
            );
            if shutting_down && !offline {
                break;
            }
            offline
        };
        if let Some(pair) = take_latest(&latest) {
            last = Some(pair);
        }
        if !should_reporter_tick(last.is_some(), shutting_down) {
            continue;
        }
        let Some((status, lang)) = last.as_ref() else {
            if shutting_down {
                break;
            }
            continue;
        };
        inbox.mark_applied(take_applied_ids(&applied_ids));
        if !shutting_down && !paused.load(Ordering::SeqCst) {
            if !gate.needs_pair_start() {
                live.connect_if_due(&transport.origin, &identity);
            }
            let was_connected = live.connected();
            for raw in live.tick(&identity, &display_name, status, *lang, inbox.ack_ids()) {
                if let Ok(outcome) = parse_heartbeat_response(&raw) {
                    let pending =
                        emit_outcome(&mut gate, &mut inbox, &event_tx, &last_pending, outcome);
                    prune_applied_history_after_heartbeat(
                        &applied_history,
                        Some(&pending),
                        &applied_ids,
                        retain_applied.load(Ordering::SeqCst),
                    );
                } else {
                    live.reject();
                    forget_pending_ids(&last_pending);
                }
            }
            if was_connected && !live.connected() {
                forget_pending_ids(&last_pending);
            }
            if live.connected() && !gate.needs_pair_start() {
                continue;
            }
        }
        // Snapshot wakes replace `last`, but never reset the network deadline.
        // Count failed attempts too: frequent GUI wakes must not amplify outages.
        if last_attempt.is_some_and(|at| !heartbeat_due(at.elapsed(), shutting_down)) {
            continue;
        }
        last_attempt = Some(Instant::now());
        inbox.mark_applied(take_applied_ids(&applied_ids));
        let pending = reporter_tick(
            &mut gate,
            &mut inbox,
            &transport,
            &identity,
            &display_name,
            lang.cloud_tag(),
            status,
            &event_tx,
            shutting_down,
            &last_pending,
        );
        prune_applied_history_after_heartbeat(
            &applied_history,
            pending.as_deref(),
            &applied_ids,
            retain_applied.load(Ordering::SeqCst),
        );
        if shutting_down {
            break;
        }
        if paused.load(Ordering::SeqCst) {
            match park_quiesced_reporter(&wake_rx, &detached, &paused, &idle) {
                QuiescePark::Break => break,
                QuiescePark::Resume => {}
                QuiescePark::Shutdown => {
                    inbox.mark_applied(take_applied_ids(&applied_ids));
                    let pending = reporter_tick(
                        &mut gate,
                        &mut inbox,
                        &transport,
                        &identity,
                        &display_name,
                        lang.cloud_tag(),
                        status,
                        &event_tx,
                        true,
                        &last_pending,
                    );
                    prune_applied_history_after_heartbeat(
                        &applied_history,
                        pending.as_deref(),
                        &applied_ids,
                        retain_applied.load(Ordering::SeqCst),
                    );
                    break;
                }
            }
        }
    }
    live.disconnect();
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn reporter_tick(
    gate: &mut ReporterGate,
    inbox: &mut CommandInbox,
    transport: &impl CloudTransport,
    identity: &CloudIdentity,
    display_name: &str,
    lang: &str,
    status: &JsonStatus,
    event_tx: &mpsc::Sender<CloudEvent>,
    offline: bool,
    last_pending: &Mutex<Option<Vec<String>>>,
) -> Option<Vec<RemoteCommand>> {
    let body = heartbeat_request_json(
        identity,
        display_name,
        status,
        lang,
        inbox.ack_ids(),
        offline,
    );
    let heartbeat_resolved;
    let pending = match transport.post_json("/api/heartbeat", &body) {
        Ok(CloudPost::Unauthorized) => {
            heartbeat_resolved = true;
            gate.on_unauthorized();
            None
        }
        Ok(CloudPost::Ok(raw)) => {
            heartbeat_resolved = true;
            parse_heartbeat_response(&raw)
                .ok()
                .map(|outcome| emit_outcome(gate, inbox, event_tx, last_pending, outcome))
        }
        Err(_) => {
            heartbeat_resolved = false;
            None
        }
    };
    if pending.is_none() {
        forget_pending_ids(last_pending);
    }
    if should_pair_start_after_heartbeat(gate.needs_pair_start(), offline, heartbeat_resolved) {
        let body = serde_json::to_string(&PairStartBody {
            device_id: &identity.device_id,
            device_token: &identity.device_token,
            display_name,
            lang,
        })
        .expect("pair json");
        match transport.post_json("/api/pair/start", &body) {
            Ok(CloudPost::Ok(raw)) => {
                if let Ok((code, url, expires_unix)) = parse_pair_start_response(&raw) {
                    gate.on_pair_start_ok();
                    let _ = event_tx.send(CloudEvent::Pairing {
                        code,
                        url,
                        expires_unix,
                    });
                }
            }
            Ok(CloudPost::Unauthorized) => gate.on_unauthorized(),
            Err(_) => {}
        }
    }
    pending
}

fn emit_outcome(
    gate: &mut ReporterGate,
    inbox: &mut CommandInbox,
    event_tx: &mpsc::Sender<CloudEvent>,
    last_pending: &Mutex<Option<Vec<String>>>,
    outcome: HeartbeatOutcome,
) -> Vec<RemoteCommand> {
    if outcome.pairing_cleared() {
        gate.on_pairing_cleared();
        let _ = event_tx.send(CloudEvent::PairingCleared);
    } else if let (Some(code), Some(url)) = (outcome.pairing_code, outcome.pairing_url) {
        gate.on_pair_start_ok();
        let expires_unix = outcome
            .expires_unix
            .unwrap_or_else(|| unix_now_secs() + PAIRING_TTL_SECS);
        let _ = event_tx.send(CloudEvent::Pairing {
            code,
            url,
            expires_unix,
        });
    }
    let pending = outcome.commands;
    let commands = inbox.take_new(pending.clone());
    inbox.retain_pending(&pending);
    store_pending_ids(last_pending, &pending);
    if !commands.is_empty() {
        let _ = event_tx.send(CloudEvent::Commands(commands));
    }
    pending
}

struct UreqTransport {
    origin: String,
}

impl CloudTransport for UreqTransport {
    fn post_json(&self, path: &str, body: &str) -> Result<CloudPost, String> {
        let url = format!("{}{path}", self.origin.trim_end_matches('/'));
        match ureq::post(&url)
            .timeout(Duration::from_secs(3))
            .set("content-type", "application/json")
            .send_string(body)
        {
            Ok(resp) => resp
                .into_string()
                .map(CloudPost::Ok)
                .map_err(|e| e.to_string()),
            Err(ureq::Error::Status(401, _)) => Ok(CloudPost::Unauthorized),
            Err(ureq::Error::Status(code, resp)) => {
                if code == 401 {
                    return Ok(CloudPost::Unauthorized);
                }
                let text = resp.into_string().unwrap_or_default();
                Err(format!("{code}: {text}"))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

fn store_pending_ids(slot: &Mutex<Option<Vec<String>>>, pending: &[RemoteCommand]) {
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(pending.iter().map(|cmd| cmd.id.clone()).collect());
    }
}

fn forget_pending_ids(slot: &Mutex<Option<Vec<String>>>) {
    if let Ok(mut guard) = slot.lock() {
        *guard = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldCommandDisposition {
    Apply,
    Keep,
    Drop,
}

/// Idle menus must not replay a held On after the donor already acked it.
fn held_command_disposition(
    engine_active: bool,
    last_pending: Option<&[String]>,
    command_id: &str,
) -> HeldCommandDisposition {
    if engine_active {
        return HeldCommandDisposition::Apply;
    }
    match last_pending {
        None => HeldCommandDisposition::Keep,
        Some(ids) if ids.iter().any(|id| id == command_id) => HeldCommandDisposition::Apply,
        Some(_) => HeldCommandDisposition::Drop,
    }
}

/// Drop the retained pairing state if its deadline has passed.
/// Call before rendering the pairing code in the panel or answering
/// `IpcRequest::Pair` so connectivity loss doesn't show a stale code forever.
pub fn expire_stale_pairing(pairing: &mut Option<(String, String, u64)>) {
    if let Some((_, _, expires_unix)) = pairing.as_ref() {
        if unix_now_secs() >= *expires_unix {
            *pairing = None;
        }
    }
}

/// Apply pending remote commands using the same Engine path as local IPC.
pub fn apply_polled_commands(
    engine: &mut Engine,
    platform: &mut dyn Platform,
    handle: &CloudHandle,
    pairing: &mut Option<(String, String, u64)>,
) {
    for event in handle.poll_events() {
        match event {
            CloudEvent::Commands(commands) => {
                let commands: Vec<RemoteCommand> = commands
                    .into_iter()
                    .filter(|cmd| !handle.already_applied(&cmd.id))
                    .collect();
                if commands.is_empty() {
                    continue;
                }
                if crate::session_lock::should_hold_cloud_commands(
                    engine.is_active(),
                    std::process::id(),
                ) {
                    handle.hold_commands(commands);
                    continue;
                }
                let pending = handle.last_pending_ids();
                let pending = pending.as_deref();
                let mut apply = Vec::new();
                let mut keep = Vec::new();
                for cmd in commands {
                    match held_command_disposition(engine.is_active(), pending, &cmd.id) {
                        HeldCommandDisposition::Apply => apply.push(cmd),
                        HeldCommandDisposition::Keep => keep.push(cmd),
                        HeldCommandDisposition::Drop => {}
                    }
                }
                if !keep.is_empty() {
                    handle.hold_commands(keep);
                }
                if apply.is_empty() {
                    continue;
                }
                let ids: Vec<String> = apply.iter().map(|c| c.id.clone()).collect();
                apply_cloud_commands(engine, platform, &apply);
                handle.mark_applied(ids);
            }
            CloudEvent::Pairing {
                code,
                url,
                expires_unix,
            } => {
                if unix_now_secs() < expires_unix {
                    *pairing = Some((code, url, expires_unix));
                } else {
                    *pairing = None;
                }
            }
            CloudEvent::PairingCleared => *pairing = None,
        }
    }
    expire_stale_pairing(pairing);
}

/// Apply remote commands first, then queue the resulting snapshot.
pub fn sync_cloud(
    engine: &mut Engine,
    platform: &mut dyn Platform,
    handle: &CloudHandle,
    pairing: &mut Option<(String, String, u64)>,
) {
    apply_polled_commands(engine, platform, handle, pairing);
    handle.push_status(
        engine.json_status(&platform.snapshot()),
        engine.config.lang(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::TestDataDir;
    use crate::platform::StubPlatform;
    use never_sleep_core::{AppConfig, Engine, JsonStatus};
    use std::sync::Mutex;

    fn test_cloud_handle(
        wake: SyncSender<ReporterWake>,
        events: mpsc::Receiver<CloudEvent>,
    ) -> CloudHandle {
        CloudHandle {
            latest: Arc::new(Mutex::new(None)),
            wake: Some(wake),
            events,
            join: None,
            held_commands: Mutex::new(Vec::new()),
            applied_ids: Arc::new(Mutex::new(Vec::new())),
            applied_history: Arc::new(Mutex::new(Vec::new())),
            detached: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            retain_applied: Arc::new(AtomicBool::new(false)),
            idle: Arc::new((Mutex::new(false), Condvar::new())),
            last_pending: Arc::new(Mutex::new(None)),
        }
    }

    fn sample_status() -> JsonStatus {
        JsonStatus {
            active: true,
            display: "asleep".into(),
            lid: "open".into(),
            on_ac: true,
            battery: Some(80),
            remaining_secs: Some(3600),
            user_present: false,
            elapsed_secs: Some(12),
            stop_reason: None,
            stop_reason_code: None,
            screen_off_enabled: true,
            lid_awake_enabled: true,
        }
    }

    #[test]
    fn local_only_start_never_creates_identity_or_reporter() {
        let _dir = TestDataDir::install();
        let config = AppConfig::default();
        assert!(!cloud_enabled(&config));
        let (mut cloud, mut identity, mut pairing) = (None, None, None);
        for _ in 0..3 {
            reconcile_remote(&config, true, &mut cloud, &mut identity, &mut pairing);
        }
        assert!(cloud.is_none());
        assert!(identity.is_none());
        assert!(pairing.is_none());
        assert!(!cloud_identity_path().exists());
    }

    #[test]
    fn disabling_remote_discards_events_and_stops_without_final_heartbeat() {
        let _dir = TestDataDir::install();
        let (wake_tx, wake_rx) = mpsc::sync_channel(2);
        let (event_tx, events) = mpsc::channel();
        event_tx
            .send(CloudEvent::Pairing {
                code: "AB7K2Q9M".into(),
                url: "https://example.com/".into(),
                expires_unix: u64::MAX,
            })
            .unwrap();
        let handle = test_cloud_handle(wake_tx, events);
        let detached = handle.detached.clone();
        let mut cloud = Some(handle);
        let mut identity = Some(load_or_create_identity().unwrap());
        let mut pairing = Some(("AB7K2Q9M".into(), "https://example.com/".into(), u64::MAX));
        reconcile_remote(
            &AppConfig::default(),
            true,
            &mut cloud,
            &mut identity,
            &mut pairing,
        );
        assert!(cloud.is_none());
        assert!(identity.is_none());
        assert!(pairing.is_none());
        assert!(detached.load(Ordering::SeqCst));
        assert!(matches!(wake_rx.recv().unwrap(), ReporterWake::Detach));
        assert!(
            wake_rx.try_recv().is_err(),
            "no Shutdown wake may send an offline POST"
        );
    }

    #[test]
    fn menu_without_ipc_ownership_cannot_start_remote() {
        let _dir = TestDataDir::install();
        let config = AppConfig {
            remote_enabled: true,
            ..AppConfig::default()
        };
        let (mut cloud, mut identity, mut pairing) = (None, None, None);
        reconcile_remote(&config, false, &mut cloud, &mut identity, &mut pairing);
        assert!(cloud.is_none());
        assert!(!cloud_identity_path().exists());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn enabling_remote_can_restart_with_the_same_saved_pairing_identity() {
        let _dir = TestDataDir::install();
        let mut config = AppConfig {
            remote_enabled: true,
            ..AppConfig::default()
        };
        let (mut cloud, mut identity, mut pairing) = (None, None, None);
        // No snapshot is queued: the real reporter has nothing to send to the network.
        reconcile_remote(&config, true, &mut cloud, &mut identity, &mut pairing);
        assert!(cloud.as_ref().unwrap().reporter_is_running());
        let first = identity.clone().unwrap();
        config.remote_enabled = false;
        reconcile_remote(&config, true, &mut cloud, &mut identity, &mut pairing);
        config.remote_enabled = true;
        reconcile_remote(&config, true, &mut cloud, &mut identity, &mut pairing);
        assert!(cloud.as_ref().unwrap().reporter_is_running());
        assert_eq!(identity.unwrap().device_id, first.device_id);
        cloud.take().unwrap().detach();
    }

    #[test]
    fn heartbeat_pairing_keeps_the_workers_expires_unix() {
        let outcome = parse_heartbeat_response(
            r#"{"ok":true,"pairing_code":"AB7K-2Q9M","pairing_url":"https://x/board/?code=AB7K-2Q9M","expires_unix":1500,"commands":[]}"#,
        )
        .unwrap();
        assert_eq!(
            outcome.expires_unix,
            Some(1500),
            "heartbeat must surface the Worker's stored offer deadline"
        );
        let transport = ScriptedTransport {
            pair: Mutex::new(vec![]),
            beat: Mutex::new(vec![Ok(CloudPost::Ok(
                r#"{"ok":true,"pairing_code":"AB7K-2Q9M","pairing_url":"https://x/board/?code=AB7K-2Q9M","expires_unix":1500,"commands":[]}"#
                    .into(),
            ))]),
            pair_calls: Mutex::new(0),
            beat_calls: Mutex::new(0),
        };
        let id = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let mut gate = ReporterGate { registered: true };
        let mut inbox = CommandInbox::default();
        let (event_tx, event_rx) = mpsc::channel();
        let pending_slot = Mutex::new(None);
        reporter_tick(
            &mut gate,
            &mut inbox,
            &transport,
            &id,
            "Studio",
            "en",
            &sample_status(),
            &event_tx,
            false,
            &pending_slot,
        );
        match event_rx.try_recv().unwrap() {
            CloudEvent::Pairing { expires_unix, .. } => {
                assert_eq!(
                    expires_unix, 1500,
                    "do not replace the Worker deadline with now+PAIRING_TTL_SECS"
                );
            }
            other => panic!("expected pairing event, got {other:?}"),
        }
    }

    #[test]
    fn pair_start_keeps_the_workers_expires_unix() {
        let (code, url, expires) = parse_pair_start_response(
            r#"{"ok":true,"pairing_code":"AB7K-2Q9M","pairing_url":"https://x/board/?code=AB7K-2Q9M","expires_unix":4242}"#,
        )
        .unwrap();
        assert_eq!(code, "AB7K-2Q9M");
        assert!(url.contains("/board/"));
        assert_eq!(expires, 4242);
    }

    #[test]
    fn write_owner_only_create_new_leaves_winner_intact() {
        let _dir = TestDataDir::install();
        let path = cloud_identity_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"winner-identity").unwrap();
        let err = write_owner_only(&path, b"loser-identity")
            .expect_err("a late writer must not truncate the winner");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "winner-identity",
            "create_new must leave the first writer's credentials on disk"
        );
    }

    #[test]
    fn load_or_create_identity_reloads_winner_after_create_race() {
        let src = include_str!("cloud.rs");
        assert!(
            src.contains("hard_link"),
            "cloud.toml must appear only after the identity bytes are fully written"
        );
        let load = src
            .split("pub fn load_or_create_identity")
            .nth(1)
            .unwrap()
            .split("fn restrict_owner_only")
            .next()
            .unwrap();
        assert!(
            load.contains("AlreadyExists"),
            "the losing process must reload the winner instead of advertising a different identity"
        );
    }

    #[test]
    fn load_or_create_identity_recovers_an_empty_stranded_file() {
        let _dir = TestDataDir::install();
        let path = cloud_identity_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        let id =
            load_or_create_identity().expect("empty cloud.toml must not disable cloud forever");
        assert_eq!(id.device_id.len(), 32);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("device_token"));
    }

    #[test]
    fn load_or_create_identity_recovers_malformed_persisted_credentials() {
        let _dir = TestDataDir::install();
        let path = cloud_identity_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stale = CloudIdentity {
            device_id: "aa".repeat(8),
            device_token: "not-hex-but-long-enough".into(),
        };
        fs::write(&path, toml::to_string_pretty(&stale).unwrap()).unwrap();
        let id =
            load_or_create_identity().expect("legacy or corrupted cloud.toml must be regenerated");
        assert!(device_credentials_are_valid(
            &id.device_id,
            &id.device_token
        ));
        assert_ne!(id, stale, "do not keep credentials the Worker will reject");
        let disk: CloudIdentity = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(disk, id);
        let src = include_str!("cloud.rs");
        let load = src
            .split("fn read_complete_identity")
            .nth(1)
            .expect("read_complete_identity")
            .split("fn wait_for_complete_identity")
            .next()
            .unwrap();
        assert!(
            load.contains("device_credentials_are_valid"),
            "persisted identities must match the Worker 32/64-hex contract"
        );
    }

    #[test]
    fn recover_stranded_identity_keeps_a_complete_peer_file() {
        let _dir = TestDataDir::install();
        let path = cloud_identity_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let winner = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        fs::write(&path, toml::to_string_pretty(&winner).unwrap()).unwrap();
        let loser = CloudIdentity {
            device_id: "11".repeat(16),
            device_token: "22".repeat(32),
        };
        let got = recover_stranded_identity(&path, loser).unwrap();
        assert_eq!(got, winner, "must not unlink a peer that already published");
        let disk: CloudIdentity = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(disk, winner);
    }

    #[test]
    fn recover_stranded_identity_second_claim_reloads_winner() {
        let _dir = TestDataDir::install();
        let path = cloud_identity_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        let first = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let second = CloudIdentity {
            device_id: "11".repeat(16),
            device_token: "22".repeat(32),
        };
        let a = recover_stranded_identity(&path, first.clone()).unwrap();
        let b = recover_stranded_identity(&path, second).unwrap();
        assert_eq!(a, first);
        assert_eq!(b, first, "the losing recoverer must reload the winner");
    }

    #[test]
    fn stranded_identity_recovery_is_exclusive_across_threads() {
        let _dir = TestDataDir::install();
        let path = cloud_identity_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        let first = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let second = CloudIdentity {
            device_id: "11".repeat(16),
            device_token: "22".repeat(32),
        };
        let path_a = path.clone();
        let path_b = path.clone();
        let id_a = first.clone();
        let id_b = second.clone();
        let a = std::thread::spawn(move || recover_stranded_identity(&path_a, id_a));
        let b = std::thread::spawn(move || recover_stranded_identity(&path_b, id_b));
        let got_a = a.join().unwrap().expect("thread a");
        let got_b = b.join().unwrap().expect("thread b");
        assert_eq!(
            got_a, got_b,
            "two recoverers of an empty cloud.toml must share one identity"
        );
        let disk: CloudIdentity = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(disk, got_a);
        assert!(got_a == first || got_a == second);
    }

    #[test]
    fn load_or_create_identity_persists_random_token() {
        let _dir = TestDataDir::install();
        let first = load_or_create_identity().expect("persist identity");
        let second = load_or_create_identity().expect("reload identity");
        assert_eq!(first, second);
        assert_eq!(first.device_id.len(), 32);
        assert_eq!(first.device_token.len(), 64);
        assert_ne!(first.device_id, first.device_token);
        let text = fs::read_to_string(cloud_identity_path()).unwrap();
        assert!(text.contains("device_id"));
        assert!(text.contains("device_token"));
    }

    #[test]
    fn load_or_create_identity_fails_when_write_cannot_persist() {
        let _dir = TestDataDir::install();
        let path = cloud_identity_path();
        fs::create_dir_all(&path).unwrap();
        assert!(
            load_or_create_identity().is_err(),
            "an unpersisted identity must not be advertised for pairing"
        );
    }

    #[test]
    fn persisted_device_token_is_owner_readable_only() {
        use std::os::unix::fs::PermissionsExt;
        let _dir = TestDataDir::install();
        load_or_create_identity().expect("persist identity");
        let mode = fs::metadata(cloud_identity_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "device_token must not be world-readable under a 022 umask"
        );
    }

    #[test]
    fn loading_identity_tightens_world_readable_token_file() {
        use std::os::unix::fs::PermissionsExt;
        let _dir = TestDataDir::install();
        let identity = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let text = toml::to_string_pretty(&identity).unwrap();
        fs::create_dir_all(cloud_identity_path().parent().unwrap()).unwrap();
        fs::write(cloud_identity_path(), text).unwrap();
        let mut perms = fs::metadata(cloud_identity_path()).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(cloud_identity_path(), perms).unwrap();
        load_or_create_identity().expect("reload identity");
        let mode = fs::metadata(cloud_identity_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "existing cloud.toml must be tightened on load");
    }

    #[test]
    fn load_identity_fails_when_existing_token_cannot_be_restricted() {
        let src = include_str!("cloud.rs");
        let load = src
            .split("pub fn load_or_create_identity")
            .nth(1)
            .unwrap()
            .split("fn restrict_owner_only")
            .next()
            .unwrap();
        assert!(
            load.contains("restrict_owner_only(&path)?")
                || load.contains("restrict_owner_only(path)?"),
            "chmod failure must fail identity load, not advertise a world-readable token"
        );
        assert!(
            !load.contains("let _ ="),
            "do not ignore restrict_owner_only errors on load"
        );
    }

    #[test]
    fn heartbeat_json_uses_stable_field_names() {
        let id = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let v: serde_json::Value = serde_json::from_str(&heartbeat_request_json(
            &id,
            "Studio",
            &sample_status(),
            "zh",
            &["cmd-1".into()],
            false,
        ))
        .unwrap();
        assert_eq!(v["device_id"], id.device_id);
        assert_eq!(v["device_token"], id.device_token);
        assert_eq!(v["display_name"], "Studio");
        assert_eq!(v["lang"], "zh");
        assert_eq!(v["ack_command_ids"][0], "cmd-1");
        assert_eq!(v["status"]["active"], true);
        assert_eq!(v["status"]["display"], "asleep");
        assert!(v["cmd"].is_null());
        assert!(
            v["offline"].is_null(),
            "live heartbeats must omit the quit offline marker"
        );
    }

    #[test]
    fn shutdown_heartbeat_json_marks_the_mac_offline() {
        let id = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let v: serde_json::Value = serde_json::from_str(&heartbeat_request_json(
            &id,
            "Studio",
            &sample_status(),
            "en",
            &[],
            true,
        ))
        .unwrap();
        assert_eq!(v["offline"], true);
        let src = include_str!("cloud.rs");
        assert!(
            src.contains("heartbeat_request_json(") && src.contains("shutting_down"),
            "the quit flush must send offline:true on the last heartbeat"
        );
    }

    #[test]
    fn heartbeat_response_keeps_display_sleep_on_off_drops_toggle() {
        let raw = r#"{
            "ok": true,
            "pairing_code": "AB7K-2Q9M",
            "pairing_url": "https://xyz-ai.app/never-sleep/board/?code=AB7K-2Q9M",
            "commands": [
                {"id": "1", "cmd": "on", "duration": "8h"},
                {"id": "2", "cmd": "toggle"},
                {"id": "3", "cmd": "off"},
                {"id": "4", "cmd": "sleep_display"}
            ]
        }"#;
        let outcome = parse_heartbeat_response(raw).unwrap();
        assert_eq!(outcome.pairing_code.as_deref(), Some("AB7K-2Q9M"));
        assert!(outcome.pairing_present());
        assert_eq!(outcome.commands.len(), 3);
        assert_eq!(outcome.commands[0].cmd, "on");
        assert_eq!(outcome.commands[1].cmd, "off");
        assert_eq!(outcome.commands[2].cmd, "sleep_display");
    }

    #[test]
    fn heartbeat_rejects_unauthorized_payload() {
        let err = parse_heartbeat_response(r#"{"ok":false,"error":"unauthorized"}"#).unwrap_err();
        assert!(err.contains("rejected"));
    }

    #[test]
    fn expired_pairing_fields_are_a_clear() {
        let outcome = parse_heartbeat_response(
            r#"{"ok":true,"pairing_code":null,"pairing_url":null,"commands":[]}"#,
        )
        .unwrap();
        assert!(outcome.pairing_cleared());
        let (event_tx, event_rx) = mpsc::channel();
        let mut gate = ReporterGate::default();
        gate.on_pair_start_ok();
        let mut inbox = CommandInbox::default();
        emit_outcome(&mut gate, &mut inbox, &event_tx, &Mutex::new(None), outcome);
        assert!(gate.needs_pair_start(), "expired offer must re-register");
        assert_eq!(event_rx.try_recv().unwrap(), CloudEvent::PairingCleared);
    }

    #[test]
    fn retained_pairing_state_is_cleared_when_expiry_passes() {
        let (event_tx, event_rx) = mpsc::channel();
        // send a fresh (non-expired) pairing event
        event_tx
            .send(CloudEvent::Pairing {
                code: "AB7K-2Q9M".into(),
                url: "https://example/board/?code=AB7K-2Q9M".into(),
                expires_unix: unix_now_secs() + 60,
            })
            .unwrap();
        let handle = test_cloud_handle(mpsc::sync_channel(2).0, event_rx);
        let mut pairing = None;
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(pairing.is_some(), "fresh pairing code must be retained");
        // simulate expiry by backdating the stored deadline
        if let Some((_, _, ref mut exp)) = pairing.as_mut() {
            *exp = 1;
        }
        // refresh should clear it
        expire_stale_pairing(&mut pairing);
        assert!(pairing.is_none(), "expired pairing state must be cleared");
        let src = include_str!("cloud.rs");
        assert!(
            src.contains("expire_stale_pairing"),
            "gui must call expire_stale_pairing before rendering or answering IpcRequest::Pair"
        );
    }

    #[test]
    fn expired_pairing_event_does_not_set_gui_code() {
        let (event_tx, event_rx) = mpsc::channel();
        event_tx
            .send(CloudEvent::Pairing {
                code: "AB7K-2Q9M".into(),
                url: "https://example/board/?code=AB7K-2Q9M".into(),
                expires_unix: 1, // epoch+1 is always in the past
            })
            .unwrap();
        let handle = test_cloud_handle(mpsc::sync_channel(2).0, event_rx);
        let mut pairing = Some(("OLD-CODE".into(), "https://example/board/".into(), 1u64));
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(
            pairing.is_none(),
            "an expired pairing code must be cleared, not shown"
        );
    }

    #[test]
    fn pairing_cleared_event_drops_gui_code() {
        let (event_tx, event_rx) = mpsc::channel();
        event_tx.send(CloudEvent::PairingCleared).unwrap();
        let handle = test_cloud_handle(mpsc::sync_channel(2).0, event_rx);
        let mut pairing = Some((
            "AB7K-2Q9M".into(),
            "https://example/board/?code=x".into(),
            u64::MAX,
        ));
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(pairing.is_none());
    }

    #[test]
    fn queued_phone_off_applies_after_handoff_not_while_idle() {
        let _guard = TestDataDir::install();
        crate::paths::ensure_data_dir().unwrap();
        std::fs::write(crate::paths::session_lock_path(), "pid=1\nclamshell=1\n").unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        event_tx
            .send(CloudEvent::Commands(vec![RemoteCommand::off("end")]))
            .unwrap();
        let handle = test_cloud_handle(mpsc::sync_channel(2).0, event_rx);
        let mut pairing = None;
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(!engine.is_active());
        let host = platform.snapshot();
        engine.handle(
            never_sleep_core::Input::Handoff {
                pref: never_sleep_core::DurationPref::Hours { hours: 8 },
                remaining_secs: Some(3600),
                elapsed_secs: None,
            },
            &host,
        );
        assert!(engine.is_active(), "handoff must start the adopted session");
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(
            !engine.is_active(),
            "phone Off queued before adopt must end the session the menu just took over"
        );
    }

    #[test]
    fn donor_applied_phone_on_is_not_replayed_after_handoff() {
        let _guard = TestDataDir::install();
        crate::paths::ensure_data_dir().unwrap();
        std::fs::write(crate::paths::session_lock_path(), "pid=1\nclamshell=1\n").unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        event_tx
            .send(CloudEvent::Commands(vec![RemoteCommand::on(
                "phone-on",
                Some("8h".into()),
            )]))
            .unwrap();
        let handle = test_cloud_handle(mpsc::sync_channel(2).0, event_rx);
        let mut pairing = None;
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(!engine.is_active(), "idle menu must hold the On");
        handle.skip_applied(vec!["phone-on".into()]);
        let host = platform.snapshot();
        engine.handle(
            never_sleep_core::Input::Handoff {
                pref: never_sleep_core::DurationPref::Hours { hours: 8 },
                remaining_secs: Some(3600),
                elapsed_secs: Some(7 * 3600),
            },
            &host,
        );
        assert!(engine.is_active(), "handoff must start the adopted session");
        assert_eq!(engine.json_status(&host).remaining_secs, Some(3600));
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(
            engine.is_active(),
            "held On must not stop the adopted session"
        );
        assert_eq!(
            engine.json_status(&platform.snapshot()).remaining_secs,
            Some(3600),
            "replaying the held 8h On would reset the adopted leftover"
        );
        let gui = include_str!("gui.rs");
        assert!(
            gui.contains("skip_applied"),
            "menu must drop donor-applied command ids before draining held phone commands"
        );
        let fg = include_str!("foreground.rs");
        assert!(
            fg.contains("applied_command_ids"),
            "handoff IPC must carry ids the foreground reporter already applied"
        );
    }

    #[test]
    fn held_phone_on_is_dropped_when_the_donor_already_completed_it() {
        assert_eq!(
            held_command_disposition(false, Some(&[]), "phone-on"),
            HeldCommandDisposition::Drop,
            "Worker no longer listing the On means the donor already acked it"
        );
        assert_eq!(
            held_command_disposition(false, None, "phone-on"),
            HeldCommandDisposition::Keep,
            "do not start standby from a held On until a later heartbeat confirms it"
        );
        assert_eq!(
            held_command_disposition(false, Some(&["phone-on".into()]), "phone-on"),
            HeldCommandDisposition::Apply
        );
        assert_eq!(
            held_command_disposition(true, None, "phone-on"),
            HeldCommandDisposition::Apply,
            "handoff drain still applies held Off/On once this engine is active"
        );
        let _guard = TestDataDir::install();
        crate::paths::ensure_data_dir().unwrap();
        std::fs::write(crate::paths::session_lock_path(), "pid=1\nclamshell=1\n").unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        event_tx
            .send(CloudEvent::Commands(vec![RemoteCommand::on(
                "phone-on", None,
            )]))
            .unwrap();
        let handle = test_cloud_handle(mpsc::sync_channel(2).0, event_rx);
        let mut pairing = None;
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(!engine.is_active(), "idle menu must hold the On");
        std::fs::remove_file(crate::paths::session_lock_path()).unwrap();
        handle.note_pending(&[]);
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(
            !engine.is_active(),
            "a held no-duration On must not restart standby after the donor already completed it"
        );
        let src = include_str!("cloud.rs");
        assert!(
            src.contains("held_command_disposition") && src.contains("store_pending_ids"),
            "reporter heartbeats must drop held commands the Worker no longer lists"
        );
        let emit = src
            .split("fn emit_outcome")
            .nth(1)
            .expect("emit_outcome")
            .split("struct UreqTransport")
            .next()
            .unwrap();
        let store_at = emit
            .find("store_pending_ids")
            .expect("pending ids must be stored in emit_outcome");
        let send_at = emit
            .find("CloudEvent::Commands")
            .expect("new commands are published as events");
        assert!(
            store_at < send_at,
            "menu poll must not see Commands while last_pending is still the prior empty set"
        );
        let last_pending = Mutex::new(Some(Vec::new()));
        let (event_tx, event_rx) = mpsc::channel();
        let mut gate = ReporterGate::default();
        let mut inbox = CommandInbox::default();
        let outcome =
            parse_heartbeat_response(r#"{"ok":true,"commands":[{"id":"phone-on","cmd":"on"}]}"#)
                .unwrap();
        emit_outcome(&mut gate, &mut inbox, &event_tx, &last_pending, outcome);
        assert_eq!(
            last_pending.lock().unwrap().clone().unwrap(),
            vec!["phone-on".to_string()],
            "last_pending must already list the new On before the menu can poll Commands"
        );
        assert!(
            event_rx
                .try_iter()
                .any(|ev| matches!(ev, CloudEvent::Commands(ref cmds) if cmds.iter().any(|c| c.id == "phone-on"))),
            "the new On is still published after last_pending is stored"
        );
    }

    #[test]
    fn failed_heartbeat_forgets_stale_pending_ids() {
        let transport = ScriptedTransport {
            pair: Mutex::new(vec![Ok(CloudPost::Ok(
                r#"{"ok":true,"pairing_code":"AB7K-2Q9M","pairing_url":"https://x/board/?code=AB7K-2Q9M"}"#
                    .into(),
            ))]),
            beat: Mutex::new(vec![
                Err("timeout".into()),
                Ok(CloudPost::Unauthorized),
                Ok(CloudPost::Ok("not-json".into())),
            ]),
            pair_calls: Mutex::new(0),
            beat_calls: Mutex::new(0),
        };
        let id = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let mut gate = ReporterGate::default();
        gate.on_pair_start_ok();
        let mut inbox = CommandInbox::default();
        let (event_tx, event_rx) = mpsc::channel();
        event_tx
            .send(CloudEvent::Commands(vec![RemoteCommand::on(
                "phone-on", None,
            )]))
            .unwrap();
        let last_pending = Arc::new(Mutex::new(Some(vec!["phone-on".to_string()])));
        let handle = CloudHandle {
            latest: Arc::new(Mutex::new(None)),
            wake: Some(mpsc::sync_channel(2).0),
            events: event_rx,
            join: None,
            held_commands: Mutex::new(Vec::new()),
            applied_ids: Arc::new(Mutex::new(Vec::new())),
            applied_history: Arc::new(Mutex::new(Vec::new())),
            detached: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            retain_applied: Arc::new(AtomicBool::new(false)),
            idle: Arc::new((Mutex::new(false), Condvar::new())),
            last_pending: Arc::clone(&last_pending),
        };

        for _ in 0..3 {
            gate.on_pair_start_ok();
            *last_pending.lock().unwrap() = Some(vec!["phone-on".to_string()]);
            let pending = reporter_tick(
                &mut gate,
                &mut inbox,
                &transport,
                &id,
                "Studio",
                "en",
                &sample_status(),
                &event_tx,
                false,
                &last_pending,
            );
            assert!(
                pending.is_none(),
                "a failed heartbeat must not look like a parsed pending list"
            );
            assert!(
                last_pending.lock().unwrap().is_none(),
                "do not keep a successful nonempty snapshot after the next heartbeat fails"
            );
        }
        assert_eq!(
            held_command_disposition(false, last_pending.lock().unwrap().as_deref(), "phone-on"),
            HeldCommandDisposition::Keep,
            "unknown pending after a failed heartbeat must not Apply a held On"
        );

        let _guard = TestDataDir::install();
        crate::paths::ensure_data_dir().unwrap();
        let mut pairing = None;
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(
            !engine.is_active(),
            "a stale nonempty last_pending must not restart standby after the donor already escaped"
        );

        let src = include_str!("cloud.rs");
        let tick = src
            .split("pub(crate) fn reporter_tick")
            .nth(1)
            .expect("reporter_tick")
            .split("fn emit_outcome")
            .next()
            .unwrap();
        assert!(
            tick.contains("forget_pending_ids"),
            "heartbeat Unauthorized / Err / unparsed Ok must clear last_pending"
        );
        assert_eq!(
            held_command_disposition(false, Some(&[]), "phone-on"),
            HeldCommandDisposition::Drop,
            "a later successful empty pending list must still drop the completed held On"
        );
    }

    #[test]
    fn hostname_from_c_buffer_reads_gethostname_bytes() {
        assert_eq!(
            hostname_from_c_buffer(b"Studio.local\0trailing"),
            Some("Studio.local".into())
        );
        assert_eq!(hostname_from_c_buffer(b"\0"), None);
        assert!(hostname_from_c_buffer(b"MacBook-Pro.local\0").is_some());
        let src = include_str!("cloud.rs");
        assert!(src.contains("libc::gethostname"));
    }

    #[test]
    fn command_inbox_dedups_and_acks() {
        let mut inbox = CommandInbox::default();
        let first = inbox.take_new(vec![RemoteCommand::on("seed", None)]);
        assert_eq!(first.len(), 1);
        let again = inbox.take_new(vec![RemoteCommand::on("seed", None)]);
        assert!(again.is_empty());
        assert!(
            inbox.ack_ids().is_empty(),
            "do not ack a command until the engine applied it"
        );
        inbox.mark_applied(["seed"]);
        assert_eq!(inbox.ack_ids(), ["seed"]);
        let batch: Vec<_> = (0..70)
            .map(|i| RemoteCommand::on(format!("c{i}"), None))
            .collect();
        let many = inbox.take_new(batch.clone());
        assert_eq!(many.len(), 70);
        assert_eq!(
            inbox.ack_ids().len(),
            1,
            "unapplied commands must not be acked by a Snapshot heartbeat"
        );
        inbox.mark_applied(batch.iter().map(|c| c.id.as_str()));
        assert_eq!(inbox.ack_ids().len(), 71);
        inbox.retain_pending(&batch);
        assert_eq!(
            inbox.ack_ids().len(),
            70,
            "delivered ids stay until the worker drops them"
        );
        assert!(
            inbox.ack_ids().iter().all(|id| id != "seed"),
            "ids the worker no longer lists can leave after ack"
        );
        inbox.retain_pending(&[]);
        assert!(inbox.ack_ids().is_empty());
    }

    #[test]
    fn transferred_ids_are_acked_before_the_inbox_sees_them() {
        let mut inbox = CommandInbox::default();
        inbox.mark_applied(["phone-on"]);
        assert_eq!(
            inbox.ack_ids(),
            ["phone-on"],
            "a donor-applied id must go out as an ack even before this reporter lists it"
        );
        let replayed = inbox.take_new(vec![RemoteCommand::on("phone-on", Some("8h".into()))]);
        assert!(
            replayed.is_empty(),
            "the Worker-delivered copy must not be applied after the transferred ack"
        );
        assert_eq!(inbox.ack_ids(), ["phone-on"]);
    }

    #[test]
    fn applied_history_drops_ids_the_worker_no_longer_lists() {
        let (wake, _wake_rx) = mpsc::sync_channel(2);
        let (_event_tx, event_rx) = mpsc::channel();
        let handle = test_cloud_handle(wake, event_rx);
        handle.skip_applied(vec!["old".into(), "live".into()]);
        let _ = handle.take_applied();
        prune_applied_history(
            &handle.applied_history,
            &[RemoteCommand::on("live", None)],
            &handle.applied_ids,
        );
        assert_eq!(
            handle.applied_command_ids(),
            vec!["live".to_string()],
            "acked commands the Worker dropped must leave history"
        );
        handle.skip_applied(vec!["queued".into()]);
        prune_applied_history(&handle.applied_history, &[], &handle.applied_ids);
        assert!(
            handle.applied_command_ids().iter().any(|id| id == "queued"),
            "ids still waiting to be acked must survive a prune"
        );
    }

    #[test]
    fn applied_history_survives_a_failed_heartbeat() {
        let (wake, _wake_rx) = mpsc::sync_channel(2);
        let (_event_tx, event_rx) = mpsc::channel();
        let handle = test_cloud_handle(wake, event_rx);
        handle.skip_applied(vec!["phone-on".into()]);
        let _ = handle.take_applied();
        prune_applied_history_after_heartbeat(
            &handle.applied_history,
            None,
            &handle.applied_ids,
            false,
        );
        assert_eq!(
            handle.applied_command_ids(),
            vec!["phone-on".to_string()],
            "a failed heartbeat must not look like the Worker dropped every command"
        );
        prune_applied_history_after_heartbeat(
            &handle.applied_history,
            Some(&[]),
            &handle.applied_ids,
            false,
        );
        assert!(
            handle.applied_command_ids().is_empty(),
            "a successfully parsed empty pending list may prune acked ids"
        );
        let src = include_str!("cloud.rs");
        assert!(
            src.contains("prune_applied_history_after_heartbeat"),
            "reporter_loop must not prune from reporter_tick's failure empty vec"
        );
    }

    #[test]
    fn applied_history_survives_until_successor_reconciles() {
        let (wake, _wake_rx) = mpsc::sync_channel(2);
        let (_event_tx, event_rx) = mpsc::channel();
        let handle = test_cloud_handle(wake, event_rx);
        handle.skip_applied(vec!["phone-on".into()]);
        let _ = handle.take_applied();
        assert!(
            should_prune_applied_history(true, false),
            "a standalone reporter may drop Worker-acked ids"
        );
        assert!(
            !should_prune_applied_history(true, true),
            "keep ids the successor may still hold after the Worker dropped them"
        );
        assert!(!should_prune_applied_history(false, false));
        prune_applied_history_after_heartbeat(
            &handle.applied_history,
            Some(&[]),
            &handle.applied_ids,
            true,
        );
        assert_eq!(
            handle.applied_command_ids(),
            vec!["phone-on".to_string()],
            "quiesce/handoff must not drop ids before skip_applied on the menu"
        );
        let src = include_str!("cloud.rs");
        assert!(
            src.contains("should_prune_applied_history") && src.contains("retain_applied"),
            "reporter_loop must not prune while the successor still holds copies"
        );
        handle.quiesce();
        assert!(
            handle.retain_applied.load(Ordering::SeqCst),
            "quiesce keeps applied history for the handoff payload"
        );
        handle.resume();
        assert!(
            handle.retain_applied.load(Ordering::SeqCst),
            "a rejected handoff must not resume pruning while the menu still holds copies"
        );
        assert!(
            !should_release_applied_retention(true),
            "keep ids while a live successor may still hold copies"
        );
        assert!(should_release_applied_retention(false));
        handle.release_applied_retention();
        assert!(
            !handle.retain_applied.load(Ordering::SeqCst),
            "prune again after the successor socket is gone"
        );
        let fg = include_str!("foreground.rs");
        assert!(
            fg.contains("release_applied_retention") && fg.contains("menu_socket_absent"),
            "do not clear retention on Ping timeout while the menu still owns the socket"
        );
    }

    #[test]
    fn apply_polled_commands_marks_ids_so_the_next_heartbeat_can_ack() {
        let (event_tx, event_rx) = mpsc::channel();
        event_tx
            .send(CloudEvent::Commands(vec![RemoteCommand::off("end")]))
            .unwrap();
        let handle = test_cloud_handle(mpsc::sync_channel(2).0, event_rx);
        let mut pairing = None;
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        engine.handle(never_sleep_core::Input::Start, &platform.snapshot());
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(!engine.is_active());
        assert_eq!(
            handle.take_applied(),
            vec!["end".to_string()],
            "the reporter must not ack Off until this process applied it"
        );
    }

    #[test]
    fn push_status_replaces_queued_active_with_inactive() {
        let (wake, _wake_rx) = mpsc::sync_channel(2);
        wake.try_send(ReporterWake::Snapshot).unwrap();
        let (_event_tx, event_rx) = mpsc::channel();
        let handle = test_cloud_handle(wake, event_rx);
        let mut active = sample_status();
        active.active = true;
        handle.push_status(active, Lang::En);
        let mut inactive = sample_status();
        inactive.active = false;
        handle.push_status(inactive, Lang::En);
        let status = handle.queued_status().expect("queued snapshot");
        assert!(
            !status.active,
            "a full capacity-one wake must not keep a stale active snapshot"
        );
    }

    #[test]
    fn disconnected_wake_still_flushes_latest_slot() {
        let latest = Mutex::new(Some({
            let mut inactive = sample_status();
            inactive.active = false;
            (inactive, Lang::En)
        }));
        let (wake_tx, wake_rx) = mpsc::sync_channel::<ReporterWake>(2);
        drop(wake_tx);
        assert!(matches!(
            wake_rx.recv_timeout(Duration::from_secs(0)),
            Err(RecvTimeoutError::Disconnected)
        ));
        let (status, _) = clone_latest(&latest).expect("shutdown must still read the slot");
        assert!(!status.active);
    }

    #[test]
    fn flush_and_join_delivers_inactive_after_slow_reporter() {
        let posted = Arc::new(Mutex::new(None));
        let latest = Arc::new(Mutex::new(None));
        let (wake_tx, wake_rx) = mpsc::sync_channel(2);
        let (_event_tx, event_rx) = mpsc::channel();
        let posted_t = Arc::clone(&posted);
        let latest_t = Arc::clone(&latest);
        let join = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            loop {
                match wake_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        *posted_t.lock().unwrap() = clone_latest(&latest_t).map(|(s, _)| s.active);
                        break;
                    }
                }
            }
        });
        let handle = CloudHandle {
            latest,
            wake: Some(wake_tx),
            events: event_rx,
            join: Some(join),
            held_commands: Mutex::new(Vec::new()),
            applied_ids: Arc::new(Mutex::new(Vec::new())),
            applied_history: Arc::new(Mutex::new(Vec::new())),
            detached: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            retain_applied: Arc::new(AtomicBool::new(false)),
            idle: Arc::new((Mutex::new(false), Condvar::new())),
            last_pending: Arc::new(Mutex::new(None)),
        };
        let mut inactive = sample_status();
        inactive.active = false;
        handle.push_status(inactive, Lang::En);
        handle.flush_and_join();
        assert_eq!(
            *posted.lock().unwrap(),
            Some(false),
            "exit must wait until the reporter has the inactive snapshot"
        );
    }

    #[test]
    fn wake_then_disconnect_is_a_single_shutdown_tick() {
        assert!(
            shutting_down_from_wake(Ok(ReporterWake::Shutdown), false),
            "flush_and_join must send an explicit shutdown, not infer it from a later disconnect"
        );
        assert!(shutting_down_from_wake(Ok(ReporterWake::Snapshot), true));
        assert!(
            !shutting_down_from_wake(Ok(ReporterWake::Snapshot), false),
            "a snapshot wake with an empty drain is not shutdown"
        );
        assert!(!shutting_down_from_wake(
            Err(RecvTimeoutError::Timeout),
            false
        ));
        assert!(shutting_down_from_wake(
            Err(RecvTimeoutError::Disconnected),
            false
        ));
        assert!(
            shutting_down_from_wake(Ok(ReporterWake::Detach), false),
            "Detach still ends the reporter loop"
        );
        assert!(
            !shutting_down_from_wake(Ok(ReporterWake::Quiesce), false),
            "Quiesce parks the reporter; it is not Shutdown"
        );
        assert!(!reporter_marks_offline(
            Ok(ReporterWake::Detach),
            false,
            true
        ));
        assert!(reporter_marks_offline(
            Ok(ReporterWake::Shutdown),
            false,
            false
        ));
        assert!(
            !reporter_goes_offline(Err(RecvTimeoutError::Disconnected), false, false, true),
            "dropping the sender after a lost Detach must not POST offline:true"
        );
        assert!(reporter_goes_offline(
            Err(RecvTimeoutError::Disconnected),
            false,
            false,
            false
        ));
        assert!(
            !reporter_goes_offline(Ok(ReporterWake::Snapshot), true, false, true),
            "Snapshot + drained shutdown after a full wake channel must honor detach"
        );
    }

    #[test]
    fn detach_records_intent_when_wake_channel_is_full() {
        let (wake_tx, wake_rx) = mpsc::sync_channel(2);
        wake_tx.try_send(ReporterWake::Snapshot).unwrap();
        wake_tx.try_send(ReporterWake::Snapshot).unwrap();
        let detached = Arc::new(AtomicBool::new(false));
        let (_event_tx, event_rx) = mpsc::channel();
        let handle = CloudHandle {
            latest: Arc::new(Mutex::new(None)),
            wake: Some(wake_tx),
            events: event_rx,
            join: None,
            held_commands: Mutex::new(Vec::new()),
            applied_ids: Arc::new(Mutex::new(Vec::new())),
            applied_history: Arc::new(Mutex::new(Vec::new())),
            detached: Arc::clone(&detached),
            paused: Arc::new(AtomicBool::new(false)),
            retain_applied: Arc::new(AtomicBool::new(false)),
            idle: Arc::new((Mutex::new(false), Condvar::new())),
            last_pending: Arc::new(Mutex::new(None)),
        };
        handle.detach();
        assert!(
            detached.load(Ordering::SeqCst),
            "Detach intent must outlive a full wake channel"
        );
        assert_eq!(wake_rx.try_recv(), Ok(ReporterWake::Snapshot));
        assert_eq!(wake_rx.try_recv(), Ok(ReporterWake::Snapshot));
        assert!(
            wake_rx.try_recv().is_err(),
            "the Detach wake itself may be dropped when the channel is full"
        );
    }

    #[test]
    fn resume_unparks_a_quiesced_reporter() {
        let (wake_tx, wake_rx) = mpsc::sync_channel(4);
        let (_event_tx, event_rx) = mpsc::channel();
        let paused = Arc::new(AtomicBool::new(true));
        let idle = Arc::new((Mutex::new(true), Condvar::new()));
        let handle = CloudHandle {
            latest: Arc::new(Mutex::new(None)),
            wake: Some(wake_tx),
            events: event_rx,
            join: None,
            held_commands: Mutex::new(Vec::new()),
            applied_ids: Arc::new(Mutex::new(Vec::new())),
            applied_history: Arc::new(Mutex::new(Vec::new())),
            detached: Arc::new(AtomicBool::new(false)),
            paused: Arc::clone(&paused),
            retain_applied: Arc::new(AtomicBool::new(false)),
            idle: Arc::clone(&idle),
            last_pending: Arc::new(Mutex::new(None)),
        };
        assert!(handle.is_paused());
        handle.resume();
        assert!(!handle.is_paused());
        assert!(
            !paused.load(Ordering::SeqCst),
            "a rejected handoff must let the foreground reporter tick again"
        );
        assert!(
            !*idle.0.lock().unwrap(),
            "the next quiesce must wait for a fresh idle signal"
        );
        assert_eq!(
            wake_rx.try_recv(),
            Ok(ReporterWake::Snapshot),
            "wake the parked recv so it notices paused=false"
        );
    }

    #[test]
    fn detach_emits_a_detach_wake_without_offline() {
        let (wake_tx, wake_rx) = mpsc::sync_channel(4);
        let (_event_tx, event_rx) = mpsc::channel();
        let signals = Arc::new(Mutex::new(Vec::new()));
        let signals_t = Arc::clone(&signals);
        let join = thread::spawn(move || {
            while let Ok(sig) = wake_rx.recv_timeout(Duration::from_millis(400)) {
                signals_t.lock().unwrap().push(sig);
            }
        });
        let handle = CloudHandle {
            latest: Arc::new(Mutex::new(None)),
            wake: Some(wake_tx),
            events: event_rx,
            join: Some(join),
            held_commands: Mutex::new(Vec::new()),
            applied_ids: Arc::new(Mutex::new(Vec::new())),
            applied_history: Arc::new(Mutex::new(Vec::new())),
            detached: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            retain_applied: Arc::new(AtomicBool::new(false)),
            idle: Arc::new((Mutex::new(false), Condvar::new())),
            last_pending: Arc::new(Mutex::new(None)),
        };
        handle.detach();
        assert_eq!(
            *signals.lock().unwrap(),
            vec![ReporterWake::Detach],
            "live handoff must not send Shutdown (offline:true)"
        );
    }

    #[test]
    fn publish_and_flush_emits_only_an_explicit_shutdown_wake() {
        let (wake_tx, wake_rx) = mpsc::sync_channel(4);
        let (_event_tx, event_rx) = mpsc::channel();
        let signals = Arc::new(Mutex::new(Vec::new()));
        let signals_t = Arc::clone(&signals);
        let join = thread::spawn(move || {
            while let Ok(sig) = wake_rx.recv_timeout(Duration::from_millis(400)) {
                signals_t.lock().unwrap().push(sig);
            }
        });
        let handle = CloudHandle {
            latest: Arc::new(Mutex::new(None)),
            wake: Some(wake_tx),
            events: event_rx,
            join: Some(join),
            held_commands: Mutex::new(Vec::new()),
            applied_ids: Arc::new(Mutex::new(Vec::new())),
            applied_history: Arc::new(Mutex::new(Vec::new())),
            detached: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            retain_applied: Arc::new(AtomicBool::new(false)),
            idle: Arc::new((Mutex::new(false), Condvar::new())),
            last_pending: Arc::new(Mutex::new(None)),
        };
        let mut inactive = sample_status();
        inactive.active = false;
        publish_and_flush(handle, inactive, Lang::En);
        assert_eq!(
            *signals.lock().unwrap(),
            vec![ReporterWake::Shutdown],
            "queue the final snapshot then shut down; do not send Snapshot then drop the sender"
        );
    }

    #[test]
    fn pair_ipc_drains_queued_pairing_event() {
        let (event_tx, event_rx) = mpsc::channel();
        event_tx
            .send(CloudEvent::Pairing {
                code: "AB7K-2Q9M".into(),
                url: "https://example/board/?code=AB7K-2Q9M".into(),
                expires_unix: u64::MAX,
            })
            .unwrap();
        let handle = test_cloud_handle(mpsc::sync_channel(2).0, event_rx);
        let mut pairing = None;
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        apply_polled_commands(&mut engine, &mut platform, &handle, &mut pairing);
        assert_eq!(
            pairing.as_ref().map(|(code, _, _)| code.as_str()),
            Some("AB7K-2Q9M")
        );
    }

    #[test]
    fn sync_cloud_reports_inactive_after_remote_off() {
        let _dir = TestDataDir::install();
        let (wake, _wake_rx) = mpsc::sync_channel(2);
        wake.try_send(ReporterWake::Snapshot).unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        event_tx
            .send(CloudEvent::Commands(vec![RemoteCommand::off("c-off")]))
            .unwrap();
        let handle = test_cloud_handle(wake, event_rx);
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = StubPlatform;
        crate::apply::dispatch(&mut engine, &mut platform, never_sleep_core::Input::Start);
        assert!(engine.is_active());
        let mut pairing = None;
        sync_cloud(&mut engine, &mut platform, &handle, &mut pairing);
        assert!(!engine.is_active());
        let status = handle.queued_status().expect("queued snapshot");
        assert!(
            !status.active,
            "phone must see standby ended before the reporter exits"
        );
    }

    struct ScriptedTransport {
        pair: Mutex<Vec<Result<CloudPost, String>>>,
        beat: Mutex<Vec<Result<CloudPost, String>>>,
        pair_calls: Mutex<usize>,
        beat_calls: Mutex<usize>,
    }

    impl CloudTransport for ScriptedTransport {
        fn post_json(&self, path: &str, _body: &str) -> Result<CloudPost, String> {
            if path.ends_with("/pair/start") {
                *self.pair_calls.lock().unwrap() += 1;
                self.pair.lock().unwrap().remove(0)
            } else {
                *self.beat_calls.lock().unwrap() += 1;
                self.beat.lock().unwrap().remove(0)
            }
        }
    }

    #[test]
    fn live_heartbeat_pairing_marks_gate_registered() {
        let transport = ScriptedTransport {
            pair: Mutex::new(vec![Err("lost pair/start body".into())]),
            beat: Mutex::new(vec![
                Ok(CloudPost::Ok(
                    r#"{"ok":true,"pairing_code":"AB7K-2Q9M","pairing_url":"https://x/board/?code=AB7K-2Q9M","commands":[]}"#
                        .into(),
                )),
                Ok(CloudPost::Ok(
                    r#"{"ok":true,"pairing_code":"AB7K-2Q9M","pairing_url":"https://x/board/?code=AB7K-2Q9M","commands":[]}"#
                        .into(),
                )),
            ]),
            pair_calls: Mutex::new(0),
            beat_calls: Mutex::new(0),
        };
        let id = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let mut gate = ReporterGate::default();
        let mut inbox = CommandInbox::default();
        let (event_tx, event_rx) = mpsc::channel();
        let status = sample_status();
        let pending_slot = Mutex::new(None);

        reporter_tick(
            &mut gate,
            &mut inbox,
            &transport,
            &id,
            "Studio",
            "en",
            &status,
            &event_tx,
            false,
            &pending_slot,
        );
        assert_eq!(*transport.pair_calls.lock().unwrap(), 0);
        assert!(
            !gate.needs_pair_start(),
            "an authenticated heartbeat that returns a live pairing must stop calling pair/start"
        );
        {
            let event = event_rx.try_recv().unwrap();
            if let CloudEvent::Pairing { code, url, .. } = event {
                assert_eq!(code, "AB7K-2Q9M");
                assert_eq!(url, "https://x/board/?code=AB7K-2Q9M");
            } else {
                panic!("expected CloudEvent::Pairing");
            }
        }

        reporter_tick(
            &mut gate,
            &mut inbox,
            &transport,
            &id,
            "Studio",
            "en",
            &status,
            &event_tx,
            false,
            &pending_slot,
        );
        assert_eq!(
            *transport.pair_calls.lock().unwrap(),
            0,
            "a live heartbeat offer must be reused; do not mint a replacement pair shard"
        );
    }

    #[test]
    fn reporter_retries_pair_start_after_transient_failure_and_401() {
        let transport = ScriptedTransport {
            pair: Mutex::new(vec![
                Err("timeout".into()),
                Ok(CloudPost::Ok(
                    r#"{"ok":true,"pairing_code":"AB7K-2Q9M","pairing_url":"https://x/board/?code=AB7K-2Q9M"}"#
                        .into(),
                )),
                Ok(CloudPost::Ok(
                    r#"{"ok":true,"pairing_code":"ZZZZ-YYYY","pairing_url":"https://x/board/?code=ZZZZ-YYYY"}"#
                        .into(),
                )),
            ]),
            beat: Mutex::new(vec![
                Ok(CloudPost::Ok(
                    r#"{"ok":true,"pairing_code":null,"pairing_url":null,"commands":[]}"#.into(),
                )),
                Ok(CloudPost::Ok(
                    r#"{"ok":true,"pairing_code":"AB7K-2Q9M","pairing_url":"https://x/board/?code=AB7K-2Q9M","commands":[]}"#
                        .into(),
                )),
                Ok(CloudPost::Unauthorized),
                Ok(CloudPost::Ok(
                    r#"{"ok":true,"pairing_code":"ZZZZ-YYYY","pairing_url":"https://x/board/?code=ZZZZ-YYYY","commands":[]}"#
                        .into(),
                )),
            ]),
            pair_calls: Mutex::new(0),
            beat_calls: Mutex::new(0),
        };
        let id = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let mut gate = ReporterGate::default();
        let mut inbox = CommandInbox::default();
        let (event_tx, _event_rx) = mpsc::channel();
        let status = sample_status();
        let pending_slot = Mutex::new(None);

        reporter_tick(
            &mut gate,
            &mut inbox,
            &transport,
            &id,
            "Studio",
            "en",
            &status,
            &event_tx,
            false,
            &pending_slot,
        );
        assert!(gate.needs_pair_start(), "first pair/start timed out");
        assert_eq!(*transport.pair_calls.lock().unwrap(), 1);

        reporter_tick(
            &mut gate,
            &mut inbox,
            &transport,
            &id,
            "Studio",
            "en",
            &status,
            &event_tx,
            false,
            &pending_slot,
        );
        assert!(
            !gate.needs_pair_start(),
            "a later heartbeat that returns the live offer registers without rotating the code"
        );
        assert_eq!(*transport.pair_calls.lock().unwrap(), 1);

        reporter_tick(
            &mut gate,
            &mut inbox,
            &transport,
            &id,
            "Studio",
            "en",
            &status,
            &event_tx,
            false,
            &pending_slot,
        );
        assert_eq!(
            *transport.pair_calls.lock().unwrap(),
            2,
            "unauthorized heartbeat must retry pair/start in the same tick"
        );
        assert!(!gate.needs_pair_start());

        reporter_tick(
            &mut gate,
            &mut inbox,
            &transport,
            &id,
            "Studio",
            "en",
            &status,
            &event_tx,
            false,
            &pending_slot,
        );
        assert!(!gate.needs_pair_start());
        assert_eq!(*transport.pair_calls.lock().unwrap(), 2);
        assert_eq!(*transport.beat_calls.lock().unwrap(), 4);
    }

    #[test]
    fn offline_shutdown_does_not_mint_a_pairing_code() {
        let transport = ScriptedTransport {
            pair: Mutex::new(vec![Ok(CloudPost::Ok(
                r#"{"ok":true,"pairing_code":"AB7K-2Q9M","pairing_url":"https://x/board/?code=AB7K-2Q9M","expires_unix":4242}"#
                    .into(),
            ))]),
            beat: Mutex::new(vec![Ok(CloudPost::Ok(
                r#"{"ok":true,"commands":[]}"#.into(),
            ))]),
            pair_calls: Mutex::new(0),
            beat_calls: Mutex::new(0),
        };
        let id = CloudIdentity {
            device_id: "ab".repeat(16),
            device_token: "cd".repeat(32),
        };
        let mut gate = ReporterGate::default();
        let mut inbox = CommandInbox::default();
        let (event_tx, event_rx) = mpsc::channel();
        let pending_slot = Mutex::new(None);
        reporter_tick(
            &mut gate,
            &mut inbox,
            &transport,
            &id,
            "Studio",
            "en",
            &sample_status(),
            &event_tx,
            true,
            &pending_slot,
        );
        assert_eq!(
            *transport.pair_calls.lock().unwrap(),
            0,
            "quit must not create an abandoned pairing offer"
        );
        assert_eq!(*transport.beat_calls.lock().unwrap(), 1);
        while let Ok(ev) = event_rx.try_recv() {
            assert!(
                !matches!(ev, CloudEvent::Pairing { .. }),
                "quit must not mint a pairing code, got {ev:?}"
            );
        }
        assert!(
            should_pair_start_after_heartbeat(true, false, true),
            "an unregistered live reporter still calls pair/start after a resolved heartbeat"
        );
        assert!(!should_pair_start_after_heartbeat(true, true, true));
        assert!(!should_pair_start_after_heartbeat(false, false, true));
        assert!(
            !should_pair_start_after_heartbeat(true, false, false),
            "a transport error must not rotate an already-shown pairing code"
        );
        let tick = include_str!("cloud.rs")
            .split("pub(crate) fn reporter_tick")
            .nth(1)
            .expect("reporter_tick")
            .split("fn emit_outcome")
            .next()
            .unwrap();
        let beat_at = tick.find("\"/api/heartbeat\"").expect("heartbeat first");
        let pair_at = tick
            .find("\"/api/pair/start\"")
            .expect("pair/start fallback");
        assert!(
            beat_at < pair_at,
            "reuse a live offer from heartbeat before minting a replacement code"
        );
    }

    #[test]
    fn resumed_reporter_requires_a_fresh_pending_snapshot() {
        let handle = test_cloud_handle(mpsc::sync_channel(2).0, mpsc::channel().1);
        handle.note_pending(&[RemoteCommand::on("old-on", None)]);
        handle.resume();
        assert_eq!(handle.last_pending_ids(), None);
    }

    #[test]
    fn live_status_ignores_clock_ticks_but_reports_state_and_deadline_changes() {
        let mut before = sample_status();
        before.active = true;
        before.elapsed_secs = Some(10);
        before.remaining_secs = Some(100);
        let mut after = before.clone();
        after.elapsed_secs = Some(11);
        after.remaining_secs = Some(99);
        assert!(!live_status_changed(&before, &after, 1));
        after.remaining_secs = Some(200);
        assert!(live_status_changed(&before, &after, 1));
        after = before.clone();
        after.display = if before.display == "asleep" {
            "awake"
        } else {
            "asleep"
        }
        .into();
        assert!(live_status_changed(&before, &after, 0));
    }

    #[test]
    fn snapshot_wakes_cannot_exceed_cloud_heartbeat_budget() {
        // Local sampling must not turn into one network request per sample.
        for ms in [0, 250, 1000, 3000, 4999, 9999] {
            assert!(!heartbeat_due(Duration::from_millis(ms), false));
        }
        assert!(heartbeat_due(Duration::from_secs(10), false));
        assert!(
            heartbeat_due(Duration::ZERO, true),
            "quit bypasses batching"
        );
    }

    #[test]
    fn timeout_reuses_last_snapshot_for_heartbeat() {
        assert!(
            !should_reporter_tick(false, false),
            "do not POST until the main loop has pushed a snapshot"
        );
        assert!(
            should_reporter_tick(true, false),
            "a heartbeat deadline must reuse last so a blocked onboarding dialog cannot look offline"
        );
        assert!(
            should_reporter_tick(false, true),
            "quit still POSTs the last snapshot"
        );
        let src = include_str!("cloud.rs");
        assert!(src.contains("take_latest"));
        assert!(
            src.contains("should_reporter_tick(last.is_some(), shutting_down)"),
            "periodic ticks must not require a fresh GUI snapshot"
        );
    }

    #[test]
    fn take_latest_consumes_the_main_loop_snapshot() {
        let slot = Mutex::new(Some((sample_status(), Lang::En)));
        assert!(take_latest(&slot).is_some());
        assert!(
            take_latest(&slot).is_none(),
            "do not replay last after the reporter already sent it"
        );
    }

    #[test]
    fn display_name_is_capped_before_it_is_advertised() {
        assert_eq!(
            bound_display_name(&"x".repeat(200)).chars().count(),
            MAX_DISPLAY_NAME_CHARS
        );
        assert_eq!(
            bound_display_name(&"名".repeat(200)).chars().count(),
            MAX_DISPLAY_NAME_CHARS
        );
        assert_eq!(bound_display_name("  Studio  "), "Studio");
        assert_eq!(bound_display_name("   "), "Mac");
        assert!(default_display_name().chars().count() <= MAX_DISPLAY_NAME_CHARS);
        let src = include_str!("cloud.rs");
        assert!(src.contains("bound_display_name"));
    }

    #[test]
    fn reporter_source_does_not_pair_start_once() {
        let src = include_str!("cloud.rs");
        assert!(src.contains("needs_pair_start"));
        assert!(src.contains("on_unauthorized"));
        assert!(src.contains("apply_effects_or_abort"));
        assert!(src.contains("fn sync_cloud"));
    }

    #[test]
    fn remote_on_aborts_when_power_assertion_fails() {
        let _dir = TestDataDir::install();
        let mut engine = Engine::new(AppConfig::default());
        let mut platform = FailPower;
        apply_cloud_commands(
            &mut engine,
            &mut platform,
            &[RemoteCommand::on("remote-1", None)],
        );
        assert!(
            !engine.is_active(),
            "phone must not see standby when stay-awake assertions were not installed"
        );
        let st = engine.json_status(&platform.snapshot());
        assert_eq!(st.stop_reason_code.as_deref(), Some("assertion_failed"));
        assert!(!st.active);
    }

    struct FailPower;

    impl crate::platform::Platform for FailPower {
        fn snapshot(&self) -> never_sleep_core::HostSnapshot {
            never_sleep_core::HostSnapshot {
                monotonic_ms: 0,
                continuous_ms: 0,
                unix_secs: 1_700_000_000,
                utc_offset_secs: 0,
                on_ac: true,
                battery_percent: Some(80),
                lid_closed: false,
                display_asleep: Some(false),
                hid_idle_ms: 80_000,
                thermal: never_sleep_core::Thermal::Nominal,
            }
        }
        fn apply_power(&mut self, _plan: never_sleep_core::PowerPlan) -> Result<(), String> {
            Err("denied".into())
        }
        fn release_power(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn sleep_display(&self) -> Result<(), String> {
            Ok(())
        }
        fn lock_session(&self) {}
        fn notify(&self, _title: &str, _body: &str) {}
        fn set_launch_at_login(&self, _enabled: bool) -> Result<(), String> {
            Ok(())
        }
        fn cleanup_orphans(&self) {}
        fn doctor(&self) -> String {
            String::new()
        }
    }
}
