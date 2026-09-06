use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
#[cfg(any(test, target_os = "macos"))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(any(test, target_os = "macos"))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Whether this process should restore clamshell sleep and delete `session.lock`.
///
/// A live peer (the menu, after a foreground handoff) owns the file. Releasing
/// the previous process must not globally re-enable clamshell sleep or remove
/// the lock the next launch uses to restore the flag.
#[cfg(any(test, target_os = "macos"))]
pub fn should_release_clamshell_lock(
    our_pid: u32,
    lock_pid: Option<u32>,
    lock_holder_alive: bool,
) -> bool {
    match lock_pid {
        None => true,
        Some(pid) if pid == our_pid => true,
        Some(_) => !lock_holder_alive,
    }
}

/// Whether ApplyPower may replace `session.lock` with this process's PID.
///
/// `already_holding` is this process's `owns_power` *before* the current
/// ApplyPower. The adopting menu is not yet holding, so it may take a live
/// donor's lock. A timed-out donor is already holding, so it must not steal
/// the successor's lock back on Tick.
#[cfg(any(test, target_os = "macos"))]
pub fn should_claim_session_lock(
    our_pid: u32,
    lock_pid: Option<u32>,
    lock_holder_alive: bool,
    already_holding: bool,
) -> bool {
    match lock_pid {
        None => true,
        Some(pid) if pid == our_pid => true,
        Some(_) if !lock_holder_alive => true,
        Some(_) => !already_holding,
    }
}

/// Whether this process should call `set_clamshell_sleep_disabled(false)`.
///
/// A live peer that recorded `clamshell=1` owns the global flag. A live peer
/// that recorded `clamshell=0` did not take ownership, so the previous process
/// must still restore clamshell sleep even while leaving the lock file in place.
#[cfg(any(test, target_os = "macos"))]
pub fn should_restore_clamshell(
    our_pid: u32,
    lock: Option<(u32, bool)>,
    lock_holder_alive: bool,
) -> bool {
    match lock {
        None => true,
        Some((pid, _)) if pid == our_pid => true,
        Some((_, claimed)) if lock_holder_alive => !claimed,
        Some(_) => true,
    }
}

/// Whether the process applying this plan should restore clamshell sleep.
///
/// A successor that does not claim the flag must clear an inherited disable
/// before acknowledging handoff. If the donor dies during `detach()`, the
/// menu never recorded `clamshell_on` and would otherwise leave the flag stuck.
#[cfg(any(test, target_os = "macos"))]
pub fn should_clear_unclaimed_clamshell(claiming: bool) -> bool {
    !claiming
}

/// Whether a failed `set_clamshell_sleep_disabled(false)` must abort adopt.
///
/// Ordinary lid-open / display-off starts also call restore because they are
/// not claiming the flag. The private selector is often unavailable there; that
/// must not tear down a successful idle assertion. Abort when a foreign lock
/// recorded `clamshell=1`, including after the donor has already died — otherwise
/// this process writes `clamshell=0` and never retries the failed restore.
#[cfg(any(test, target_os = "macos"))]
pub fn should_fail_unclaimed_clamshell_restore(
    claiming: bool,
    lock: Option<(u32, bool)>,
    _lock_holder_alive: bool,
    our_pid: u32,
) -> bool {
    if claiming {
        return false;
    }
    matches!(lock, Some((pid, true)) if pid != our_pid)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLockRecord {
    pub pid: u32,
    pub clamshell: bool,
    pub starttime: Option<u64>,
}

#[cfg(test)]
pub fn parse_lock_text(s: &str) -> (u32, bool) {
    let rec = parse_lock_record(s);
    (rec.pid, rec.clamshell)
}

pub fn parse_lock_record(s: &str) -> SessionLockRecord {
    let mut pid = 0u32;
    let mut clamshell = false;
    let mut starttime = None;
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("pid=") {
            pid = v.trim().parse().unwrap_or(0);
        }
        if let Some(v) = line.strip_prefix("clamshell=") {
            clamshell = v.trim() == "1";
        }
        if let Some(v) = line.strip_prefix("starttime=") {
            starttime = v.trim().parse().ok();
        }
    }
    SessionLockRecord {
        pid,
        clamshell,
        starttime,
    }
}

pub fn format_lock_text(pid: u32, clamshell: bool, starttime: Option<u64>) -> String {
    let mut body = format!("pid={pid}\nclamshell={}\n", u8::from(clamshell));
    if let Some(start) = starttime {
        body.push_str(&format!("starttime={start}\n"));
    }
    body
}

pub fn read_lock_record() -> Option<SessionLockRecord> {
    let mut s = String::new();
    std::fs::File::open(crate::paths::session_lock_path())
        .ok()?
        .read_to_string(&mut s)
        .ok()?;
    Some(parse_lock_record(&s))
}

pub fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Written when kernel starttime is unreadable. A live pid stays the owner:
/// a later readable starttime cannot tell lookup recovery from PID reuse.
pub const UNVERIFIED_INSTANCE_TOKEN: u64 = u64::MAX;

/// A live pid is not enough: SIGKILL can leave `session.lock` for a reused pid.
pub fn lock_holder_is_live(
    pid_alive: bool,
    recorded_start: Option<u64>,
    observed_start: Option<u64>,
) -> bool {
    if !pid_alive {
        return false;
    }
    match recorded_start {
        None => true,
        // A later readable starttime can be lookup recovery on the same live
        // process; treating it as PID reuse would drop clamshell under a live owner.
        Some(UNVERIFIED_INSTANCE_TOKEN) => true,
        Some(want) => observed_start == Some(want),
    }
}

#[cfg(any(test, target_os = "linux"))]
pub fn parse_proc_stat_starttime(stat: &str) -> Option<u64> {
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19)?.parse().ok()
}

pub fn process_starttime(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_proc_stat_starttime(&raw)
    }
    #[cfg(target_os = "macos")]
    {
        macos_proc_starttime(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Token written into every new `session.lock`. Kernel starttime when readable,
/// otherwise the peer-visible unverified sentinel.
pub fn process_instance_token(pid: u32) -> u64 {
    process_starttime(pid).unwrap_or(UNVERIFIED_INSTANCE_TOKEN)
}

/// Token to compare against a recorded lock. Our pid uses the same fallback as
/// `write_lock`, so a sysctl miss cannot make this process look dead.
pub fn observed_instance_token(pid: u32) -> Option<u64> {
    if pid == std::process::id() {
        Some(process_instance_token(pid))
    } else {
        process_starttime(pid)
    }
}

#[cfg(target_os = "macos")]
fn macos_proc_starttime(pid: u32) -> Option<u64> {
    const PROC_PIDTBSDINFO: libc::c_int = 3;
    const MAXCOMLEN: usize = 16;

    #[allow(dead_code)]
    #[repr(C)]
    struct ProcBsdInfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_xstatus: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: libc::uid_t,
        pbi_gid: libc::gid_t,
        pbi_ruid: libc::uid_t,
        pbi_rgid: libc::gid_t,
        pbi_svuid: libc::uid_t,
        pbi_svgid: libc::gid_t,
        rfu_1: u32,
        pbi_comm: [libc::c_char; MAXCOMLEN],
        pbi_name: [libc::c_char; 2 * MAXCOMLEN],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }

    unsafe extern "C" {
        fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            arg: u64,
            buffer: *mut libc::c_void,
            buffer_size: libc::c_int,
        ) -> libc::c_int;
    }

    let pid = libc::c_int::try_from(pid).ok()?;
    unsafe {
        let mut info: ProcBsdInfo = std::mem::zeroed();
        let size = libc::c_int::try_from(std::mem::size_of::<ProcBsdInfo>()).ok()?;
        let bytes = proc_pidinfo(
            pid,
            PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        if bytes != size {
            return None;
        }
        Some(info.pbi_start_tvsec)
    }
}

/// Hold phone On/Off while another live process still owns standby.
/// Applying Off against an idle menu is a no-op and would drop the command.
pub fn should_hold_cloud_commands(engine_active: bool, our_pid: u32) -> bool {
    if engine_active {
        return false;
    }
    foreign_session_lock_is_live(our_pid) || foreign_reporter_lock_is_live(our_pid)
}

fn foreign_session_lock_is_live(our_pid: u32) -> bool {
    match read_lock_record() {
        Some(rec) if rec.pid != our_pid => lock_holder_is_live(
            pid_is_alive(rec.pid),
            rec.starttime,
            observed_instance_token(rec.pid),
        ),
        _ => false,
    }
}

fn foreign_reporter_lock_is_live(our_pid: u32) -> bool {
    match read_reporter_lock() {
        Some(rec) if rec.pid != our_pid => lock_holder_is_live(
            pid_is_alive(rec.pid),
            rec.starttime,
            observed_instance_token(rec.pid),
        ),
        _ => false,
    }
}

/// Do not start a local session while a live donor still owns standby.
/// ⌥⌘P / Toggle during that overlap would otherwise race the handoff.
#[cfg(any(test, target_os = "macos"))]
pub fn should_defer_local_controls(engine_active: bool, our_pid: u32) -> bool {
    should_hold_cloud_commands(engine_active, our_pid)
}

/// Remember ⌥⌘P / Toggle while a live donor still owns standby.
#[cfg(any(test, target_os = "macos"))]
pub fn note_deferred_escape(deferred: bool, pending_stop: &mut bool) {
    if deferred {
        *pending_stop = true;
    }
}

/// After a successful adopt or a stop_donor reply, apply the remembered escape.
#[cfg(test)]
pub fn take_pending_stop_on_adopt(adopted: bool, pending_stop: &mut bool) -> bool {
    take_pending_stop_after_handoff(adopted, false, pending_stop)
}

/// Consume the escape after the held-command drain when adopt succeeded or the
/// donor was told to stop (remaining_secs already zero, etc.).
#[cfg(any(test, target_os = "macos"))]
pub fn take_pending_stop_after_handoff(
    adopted: bool,
    stop_donor: bool,
    pending_stop: &mut bool,
) -> bool {
    let apply = *pending_stop && (adopted || stop_donor);
    if apply {
        *pending_stop = false;
    }
    apply
}

/// `never-sleep off` while idle behind a live donor is the same escape as ⌥⌘P.
#[cfg(any(test, target_os = "macos"))]
pub fn should_record_deferred_off(engine_active: bool, deferred: bool) -> bool {
    !engine_active && deferred
}

/// Quit before adopt: the donor will resume. Do not POST `offline:true`.
#[cfg(any(test, target_os = "macos"))]
pub fn should_detach_cloud_on_quit(engine_active: bool, our_pid: u32) -> bool {
    should_defer_local_controls(engine_active, our_pid)
}

/// A remembered Off / ⌥⌘P must still stop the donor if this process cannot adopt.
#[cfg(any(test, target_os = "macos"))]
pub fn should_stop_donor_on_failed_handoff(
    handoff: bool,
    adopted: bool,
    pending_stop: bool,
) -> bool {
    handoff && !adopted && pending_stop
}

/// Only one foreground process may poll commands for the persisted identity.
#[cfg(test)]
pub fn should_claim_reporter_lock(
    our_pid: u32,
    lock_pid: Option<u32>,
    lock_holder_alive: bool,
) -> bool {
    match lock_pid {
        None => true,
        Some(pid) if pid == our_pid => true,
        Some(_) => !lock_holder_alive,
    }
}

/// Write the donor's pid back into `session.lock` before rolling back an adopt.
#[cfg(any(test, target_os = "macos"))]
pub fn should_restore_donor_lock_on_adopt_rollback(reject_adopt: bool) -> bool {
    reject_adopt
}

/// Only Stop the already-adopted session after the donor lock is actually restored.
#[cfg(any(test, target_os = "macos"))]
pub fn should_rollback_adopt_after_donor_lock_restore(
    reject_adopt: bool,
    restore_ok: bool,
) -> bool {
    reject_adopt && restore_ok
}

/// After a successor takes `session.lock`, the previous process must not
/// reassert the global clamshell flag from `MSG_SYSTEM_HAS_POWERED_ON`.
#[cfg(any(test, target_os = "macos"))]
pub fn should_retain_clamshell_power_callback(may_own: bool, clamshell_on: bool) -> bool {
    may_own && clamshell_on
}

/// Claiming `session.lock` is part of ApplyPower; a silent write miss would
/// let the menu acknowledge adopt while the donor still holds the file.
#[cfg(any(test, target_os = "macos"))]
pub fn should_fail_apply_if_lock_write_failed(
    may_own: bool,
    owns_power: bool,
    write_ok: bool,
) -> bool {
    may_own && owns_power && !write_ok
}

/// Clearing the global flag before a failed successor lock write must not
/// leave clamshell sleep disabled under a donor that still owns `clamshell=1`.
/// A dead owner's leftover file must not re-enable the override with no owner.
#[cfg(any(test, target_os = "macos"))]
pub fn should_reassert_donor_clamshell_after_lock_write_failure(
    write_failed: bool,
    cleared_unclaimed: bool,
    donor_clamshell: bool,
    donor_alive: bool,
) -> bool {
    write_failed && cleared_unclaimed && donor_clamshell && donor_alive
}

#[cfg(any(test, target_os = "macos"))]
pub fn write_session_lock(pid: u32, clamshell: bool, starttime: Option<u64>) -> bool {
    if pid == 0 {
        return false;
    }
    if crate::paths::ensure_data_dir().is_err() {
        return false;
    }
    let path = crate::paths::session_lock_path();
    let body = format_lock_text(pid, clamshell, starttime);
    let Some((tmp, mut file)) = create_private_session_lock_tmp(&path) else {
        return false;
    };
    if file.write_all(body.as_bytes()).is_err() || file.sync_all().is_err() {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    drop(file);
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

#[cfg(any(test, target_os = "macos"))]
static SESSION_LOCK_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(any(test, target_os = "macos"))]
fn create_private_session_lock_tmp(path: &Path) -> Option<(PathBuf, File)> {
    let pid = std::process::id();
    for _ in 0..32 {
        let seq = SESSION_LOCK_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_file_name(format!("session.lock.tmp.{pid}.{seq}"));
        match File::create_new(&tmp) {
            Ok(file) => return Some((tmp, file)),
            Err(_) => continue,
        }
    }
    None
}

/// Rollback must write the donor's original clamshell bit, not the successor's.
#[cfg(any(test, target_os = "macos"))]
pub fn restored_donor_clamshell_bit(donor_claimed: bool, _successor_claimed: bool) -> bool {
    donor_claimed
}

#[cfg(any(test, target_os = "macos"))]
pub fn restore_donor_session_lock(pid: u32, starttime: u64, donor_clamshell: bool) -> bool {
    if pid == 0 {
        return false;
    }
    let successor = read_lock_record().map(|rec| rec.clamshell).unwrap_or(false);
    let clamshell = restored_donor_clamshell_bit(donor_clamshell, successor);
    write_session_lock(pid, clamshell, Some(starttime))
}

/// Foreground admission is `reporter.lock`, including when cloud is disabled.
pub fn should_claim_foreground_reporter_lock(_cloud_ok: bool, menu_socket_absent: bool) -> bool {
    menu_socket_absent
}

/// Do not Start a second local session if this process cannot own the reporter.
pub fn should_abort_foreground_without_reporter_lock(needs_reporter: bool, claimed: bool) -> bool {
    needs_reporter && !claimed
}

fn read_reporter_lock() -> Option<SessionLockRecord> {
    let text = std::fs::read_to_string(crate::paths::reporter_lock_path()).ok()?;
    Some(parse_lock_record(&text))
}

thread_local! {
    static REPORTER_LOCK: RefCell<Option<(PathBuf, File)>> = const { RefCell::new(None) };
}

fn lock_reporter_file(file: &File) -> bool {
    #[cfg(target_os = "linux")]
    {
        let mut lock = libc::flock {
            l_type: libc::F_WRLCK as i16,
            l_whence: libc::SEEK_SET as i16,
            l_start: 0,
            l_len: 0,
            l_pid: 0,
        };
        // SAFETY: `file` is an fd we own; F_OFD_SETLK takes a non-blocking
        // exclusive open-file-description lock so two opens cannot both succeed.
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &mut lock) == 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: `file` is an fd we own; flock(LOCK_EX|LOCK_NB) excludes other
        // processes until this fd is dropped.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
    }
}

fn holding_current_reporter_lock() -> bool {
    let path = crate::paths::reporter_lock_path();
    REPORTER_LOCK.with(|slot| {
        let mismatch = slot
            .borrow()
            .as_ref()
            .is_some_and(|(held, _)| held != &path);
        if mismatch {
            slot.borrow_mut().take();
            false
        } else {
            slot.borrow().is_some()
        }
    })
}

/// Exclusive cloud polling for `never-sleep on` when no menu owns IPC.
///
/// Ownership is the held flock / OFD lock, not pid liveness. A leftover file
/// from a crash is taken over without unlink-then-create_new.
pub fn try_claim_reporter_lock(our_pid: u32) -> bool {
    if our_pid == 0 {
        return false;
    }
    if holding_current_reporter_lock() {
        return true;
    }
    let Ok(_) = crate::paths::ensure_data_dir() else {
        return false;
    };
    let path = crate::paths::reporter_lock_path();
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(file) => file,
        Err(_) => return false,
    };
    if !lock_reporter_file(&file) {
        return false;
    }
    let body = format_lock_text(our_pid, false, Some(process_instance_token(our_pid)));
    if file.set_len(0).is_err()
        || file.seek(SeekFrom::Start(0)).is_err()
        || file.write_all(body.as_bytes()).is_err()
    {
        return false;
    }
    REPORTER_LOCK.with(|slot| {
        *slot.borrow_mut() = Some((path, file));
    });
    true
}

pub fn release_reporter_lock(_our_pid: u32) {
    REPORTER_LOCK.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(test)]
fn hold_reporter_lock_for_test() -> File {
    let _ = crate::paths::ensure_data_dir();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(crate::paths::reporter_lock_path())
        .expect("open reporter.lock for the competing test holder");
    assert!(
        lock_reporter_file(&file),
        "test holder must take the exclusive reporter lock"
    );
    file
}

/// Keep a foreign clamshell=1 lock when inherited restore failed, even if the
/// donor is already dead. Next launch still has something to recover from.
#[cfg(any(test, target_os = "macos"))]
pub fn should_keep_inherited_clamshell_lock(
    failed_restore: bool,
    lock: Option<(u32, bool)>,
    our_pid: u32,
) -> bool {
    failed_restore && matches!(lock, Some((pid, true)) if pid != our_pid)
}

/// Local Start vs Stop is captured before draining phone commands so a queued
/// remote Off cannot invert ⌥⌘P / Toggle into a fresh Start.
#[cfg(any(test, target_os = "macos"))]
pub fn local_toggle_wants_stop(engine_active: bool) -> bool {
    engine_active
}

/// `Some(true)` = Stop, `Some(false)` = Start, `None` = already matches intent.
#[cfg(any(test, target_os = "macos"))]
pub fn local_toggle_after_cloud_drain(want_stop: bool, now_active: bool) -> Option<bool> {
    if want_stop == now_active {
        Some(want_stop)
    } else {
        None
    }
}

/// Successor IOKit miss after clearing an inherited clamshell flag: the live
/// donor must ApplyPower again because its plan is unchanged.
#[cfg(any(test, target_os = "macos"))]
pub fn should_request_donor_clamshell_reapply(need_reassert: bool, iokit_ok: bool) -> bool {
    need_reassert && !iokit_ok
}

#[cfg(any(test, target_os = "macos"))]
static DONOR_CLAMSHELL_REAPPLY: AtomicBool = AtomicBool::new(false);

#[cfg(any(test, target_os = "macos"))]
pub fn request_donor_clamshell_reapply() {
    DONOR_CLAMSHELL_REAPPLY.store(true, Ordering::SeqCst);
    let _ = crate::paths::ensure_data_dir();
    let _ = std::fs::write(crate::paths::clamshell_reapply_path(), b"1\n");
}

pub fn take_donor_clamshell_reapply() -> bool {
    std::fs::remove_file(crate::paths::clamshell_reapply_path()).is_ok()
}

/// Cross-process signal for the live donor. Does not use the data directory.
#[cfg(test)]
pub fn take_ipc_donor_clamshell_reapply() -> bool {
    DONOR_CLAMSHELL_REAPPLY.swap(false, Ordering::SeqCst)
}

/// Read the IPC bit without dropping it. A timed-out `reply.send` or a later
/// `write_resp` miss must still see the signal on the donor's next handoff.
#[cfg(any(test, target_os = "macos"))]
pub fn peek_ipc_donor_clamshell_reapply() -> bool {
    DONOR_CLAMSHELL_REAPPLY.load(Ordering::SeqCst)
}

/// Only handoff replies carry the bit; Ping must not consume a signal the donor ignores.
#[cfg(any(test, target_os = "macos"))]
pub fn should_signal_ipc_donor_clamshell_reapply(handoff_reply: bool, signaled: bool) -> bool {
    handoff_reply && signaled
}

pub fn should_reapply_donor_clamshell(engine_active: bool, ipc_signaled: bool) -> bool {
    engine_active && ipc_signaled
}

/// Only the still-active donor may consume the request; an idle menu must not.
pub fn take_pending_donor_clamshell_reapply(engine_active: bool) -> bool {
    engine_active && take_donor_clamshell_reapply()
}

#[cfg(any(test, target_os = "macos"))]
pub fn peer_reporter_lock_is_live(our_pid: u32) -> bool {
    foreign_reporter_lock_is_live(our_pid)
}

/// Idle menu must not POST `active=false` while a foreground donor still reports.
#[cfg(any(test, target_os = "macos"))]
pub fn should_pause_menu_reporter(foreign_reporter_live: bool) -> bool {
    foreign_reporter_live
}

#[cfg(any(test, target_os = "macos"))]
pub fn should_resume_paused_menu_reporter(
    paused: bool,
    foreign_reporter_live: bool,
    engine_active: bool,
) -> bool {
    paused && (engine_active || !foreign_reporter_live)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_out_donor_does_not_overwrite_live_successor_lock() {
        assert!(
            !should_claim_session_lock(11, Some(22), true, true),
            "after a lost adopt reply, Tick/ApplyPower must not replace the menu's session.lock"
        );
        assert!(
            should_retain_clamshell_power_callback(true, true)
                && !should_retain_clamshell_power_callback(false, true)
                && !should_retain_clamshell_power_callback(true, false),
            "a donor that no longer owns session.lock must not reassert clamshell on wake"
        );
        assert!(
            should_fail_apply_if_lock_write_failed(true, true, false)
                && !should_fail_apply_if_lock_write_failed(true, true, true)
                && !should_fail_apply_if_lock_write_failed(false, true, false),
            "a successor that cannot rewrite session.lock must not acknowledge adopt"
        );
        assert!(
            should_claim_session_lock(11, Some(22), true, false),
            "the adopting menu must take the live donor's lock during handoff"
        );
        assert!(should_claim_session_lock(11, Some(11), true, true));
        assert!(should_claim_session_lock(11, Some(22), false, true));
        assert!(should_claim_session_lock(11, None, false, false));
        let macos = include_str!("platform/macos.rs");
        let apply = macos.split("fn apply_power").nth(1).expect("apply_power");
        assert!(
            apply.contains("already_holding"),
            "ApplyPower must snapshot owns_power before assigning it, so adopt is not treated as a donor retry"
        );
        assert!(
            apply.contains("should_retain_clamshell_power_callback"),
            "ApplyPower must drop CLAMSHELL_OWNED when a successor holds the lock"
        );
        assert!(
            apply.contains("should_fail_apply_if_lock_write_failed")
                && apply.contains("write_lock(self.clamshell_on)"),
            "adopt must not succeed when the successor cannot rewrite session.lock"
        );
        let claim_at = apply
            .find("should_claim_session_lock")
            .expect("ApplyPower must consult successor ownership before write_lock");
        let clamshell_at = apply
            .find("should_clear_unclaimed_clamshell")
            .expect("clamshell selector follows ownership");
        let write_at = apply
            .rfind("write_lock")
            .expect("success path still records the lock when this process owns it");
        assert!(
            claim_at < clamshell_at,
            "a live successor's clamshell flag must not be restored by a timed-out donor Tick"
        );
        assert!(
            claim_at < write_at,
            "a live menu lock must survive a timed-out donor's later ApplyPower"
        );
    }

    #[test]
    fn lock_write_failure_reasserts_donor_clamshell() {
        assert!(
            should_reassert_donor_clamshell_after_lock_write_failure(true, true, true, true),
            "clearing the flag then failing to take session.lock must put clamshell disable back"
        );
        assert!(
            !should_reassert_donor_clamshell_after_lock_write_failure(true, true, true, false),
            "a dead leftover clamshell=1 lock must not re-enable the override with no owner"
        );
        assert!(!should_reassert_donor_clamshell_after_lock_write_failure(
            false, true, true, true
        ));
        assert!(!should_reassert_donor_clamshell_after_lock_write_failure(
            true, false, true, true
        ));
        assert!(!should_reassert_donor_clamshell_after_lock_write_failure(
            true, true, false, true
        ));
        let macos = include_str!("platform/macos.rs");
        let apply = macos.split("fn apply_power").nth(1).expect("apply_power");
        assert!(
            apply.contains("should_reassert_donor_clamshell_after_lock_write_failure")
                && apply.contains("set_clamshell_sleep_disabled(true)"),
            "ApplyPower must restore the donor's clamshell disable after a failed lock write"
        );
        let reassert = apply
            .split("should_reassert_donor_clamshell_after_lock_write_failure")
            .nth(1)
            .expect("reassert call");
        let args = reassert.split(')').next().expect("reassert args");
        assert!(
            args.contains("holder_alive"),
            "reassert only while the donor that recorded clamshell=1 is still live"
        );
        let clear_at = apply
            .find("should_clear_unclaimed_clamshell")
            .expect("clear unclaimed");
        let write_at = apply.rfind("write_lock").expect("write lock");
        assert!(
            clear_at < write_at,
            "do not write session.lock before restoring an inherited clamshell flag"
        );
        let reassert = apply
            .split("should_reassert_donor_clamshell_after_lock_write_failure")
            .nth(1)
            .expect("reassert block");
        assert!(
            reassert.contains("let iokit_ok = set_clamshell_sleep_disabled(true)")
                && reassert.contains("should_request_donor_clamshell_reapply")
                && reassert.contains("request_donor_clamshell_reapply"),
            "a failed IOKit reassert must ask the still-active donor to ApplyPower again"
        );
    }

    #[test]
    fn local_toggle_keeps_escape_intent_after_cloud_off() {
        assert!(
            local_toggle_wants_stop(true),
            "⌥⌘P during an active session means Stop"
        );
        assert!(!local_toggle_wants_stop(false));
        assert_eq!(local_toggle_after_cloud_drain(true, true), Some(true));
        assert_eq!(
            local_toggle_after_cloud_drain(true, false),
            None,
            "a queued phone Off must not turn the local escape into Start"
        );
        assert_eq!(local_toggle_after_cloud_drain(false, false), Some(false));
        assert_eq!(
            local_toggle_after_cloud_drain(false, true),
            None,
            "a queued phone On already matches a local Start"
        );
        let gui = include_str!("gui.rs");
        for (marker, before) in [
            ("UserEvent::Hotkey", "refresh_ui"),
            ("UserEvent::Menu(id)", "handle_menu_event"),
            ("UserEvent::Ui(command)", "handle_ui_command"),
        ] {
            let block = gui.split(marker).nth(1).expect(marker);
            let action = block.split(before).next().unwrap();
            let intent_at = action
                .find("local_toggle_wants_stop")
                .unwrap_or_else(|| panic!("{marker} must capture Start/Stop before drain"));
            let drain_at = action
                .find("apply_polled_commands")
                .unwrap_or_else(|| panic!("{marker} must still drain cloud commands"));
            assert!(
                intent_at < drain_at,
                "{marker} must capture the local toggle intent before applying a queued phone Off"
            );
        }
        let ipc = gui
            .split("while let Ok(incoming) = ipc_rx.try_recv()")
            .nth(1)
            .expect("ipc loop")
            .split("match event")
            .next()
            .unwrap();
        let intent_at = ipc
            .find("local_toggle_wants_stop")
            .expect("IPC Toggle must capture intent before drain");
        let drain_at = ipc
            .find("apply_polled_commands")
            .expect("IPC still drains before handle_ipc");
        assert!(
            intent_at < drain_at,
            "IPC Toggle must not invert after a queued phone Off"
        );
        assert!(
            gui.contains("local_toggle_after_cloud_drain"),
            "dispatch the captured Start/Stop, not a fresh Toggle against the post-drain engine"
        );
    }

    #[test]
    fn macos_process_starttime_uses_supported_libproc_api() {
        let source = include_str!("session_lock.rs");
        let implementation = source
            .split("fn macos_proc_starttime")
            .nth(1)
            .expect("macOS process starttime implementation")
            .split("/// Hold phone On/Off")
            .next()
            .expect("end of macOS process starttime implementation");

        assert!(
            implementation.contains("proc_pidinfo") && implementation.contains("PROC_PIDTBSDINFO"),
            "macOS process starttime must use the supported libproc API"
        );
        assert!(
            !implementation.contains("libc::kinfo_proc"),
            "the libc crate does not expose macOS kinfo_proc"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reads_current_process_starttime() {
        assert!(
            process_starttime(std::process::id()).is_some(),
            "libproc must return a start time for the current process"
        );
    }

    #[test]
    fn failed_iokit_reassert_leaves_a_donor_reapply_request() {
        assert!(should_request_donor_clamshell_reapply(true, false));
        assert!(!should_request_donor_clamshell_reapply(true, true));
        assert!(!should_request_donor_clamshell_reapply(false, false));
        let _dir = crate::paths::TestDataDir::install();
        assert!(
            !take_pending_donor_clamshell_reapply(false),
            "an idle menu must not consume the donor's reapply request"
        );
        request_donor_clamshell_reapply();
        assert!(
            !take_pending_donor_clamshell_reapply(false),
            "idle callers leave the marker for the live donor"
        );
        assert!(take_pending_donor_clamshell_reapply(true));
        assert!(
            !take_pending_donor_clamshell_reapply(true),
            "the reapply request is one-shot"
        );
        let gui = include_str!("gui.rs");
        assert!(
            gui.contains("take_pending_donor_clamshell_reapply")
                && gui.contains("apply_power(plan)"),
            "menu Tick must reapply clamshell when the successor's IOKit reassert missed"
        );
        let fg = include_str!("foreground.rs");
        assert!(
            fg.contains("take_pending_donor_clamshell_reapply") && fg.contains("apply_power(plan)"),
            "the still-active donor Tick must reapply clamshell after a failed successor reassert"
        );
        assert!(
            should_reapply_donor_clamshell(true, true)
                && !should_reapply_donor_clamshell(false, true)
                && !should_reapply_donor_clamshell(true, false),
            "a live donor must ApplyPower when the handoff reply says reapply"
        );
        let _ = take_ipc_donor_clamshell_reapply();
        request_donor_clamshell_reapply();
        assert!(
            peek_ipc_donor_clamshell_reapply() && peek_ipc_donor_clamshell_reapply(),
            "peeking the IPC bit must leave it for a retried handoff after a lost reply"
        );
        assert!(
            should_signal_ipc_donor_clamshell_reapply(true, true)
                && !should_signal_ipc_donor_clamshell_reapply(false, true)
                && !should_signal_ipc_donor_clamshell_reapply(true, false),
            "only a handoff reply may carry the bit; Ping must not drop a signal the donor ignores"
        );
        assert!(
            take_ipc_donor_clamshell_reapply(),
            "the IPC bit must not depend on writing clamshell.reapply"
        );
        assert!(
            !take_ipc_donor_clamshell_reapply(),
            "take still clears the bit when delivery is known"
        );
        let handle = gui
            .split("fn handle_ipc")
            .nth(1)
            .expect("handle_ipc")
            .split("fn local_controls_deferred")
            .next()
            .unwrap();
        assert!(
            handle.contains("peek_ipc_donor_clamshell_reapply")
                && handle.contains("should_signal_ipc_donor_clamshell_reapply")
                && handle.contains("clamshell_reapply")
                && !handle.contains("take_ipc_donor_clamshell_reapply"),
            "handoff replies must copy the reapply bit without dropping it before send"
        );
        assert!(
            fg.contains("donor_should_reapply_clamshell")
                && fg.contains("should_reapply_donor_clamshell"),
            "the donor must ApplyPower from the live IPC reply, not only the marker file"
        );
    }

    #[test]
    fn menu_reporter_stays_paused_while_a_donor_holds_reporter_lock() {
        assert!(should_pause_menu_reporter(true));
        assert!(!should_pause_menu_reporter(false));
        assert_eq!(
            should_pause_menu_reporter(peer_reporter_lock_is_live(std::process::id())),
            peer_reporter_lock_is_live(std::process::id()),
            "pause follows a live foreign reporter.lock"
        );
        assert!(
            should_resume_paused_menu_reporter(true, false, false),
            "resume once the donor releases reporter.lock"
        );
        assert!(
            should_resume_paused_menu_reporter(true, true, true),
            "resume as soon as this menu owns the live session"
        );
        assert!(
            !should_resume_paused_menu_reporter(true, true, false),
            "stay paused while a donor still publishes active=true"
        );
        assert!(!should_resume_paused_menu_reporter(false, false, true));
        let gui = include_str!("gui.rs");
        let spawn = include_str!("cloud.rs")
            .split("pub fn reconcile_remote")
            .nth(1)
            .expect("menu cloud spawn")
            .split("/// Phone-board cards")
            .next()
            .unwrap();
        assert!(
            spawn.contains("should_pause_menu_reporter")
                && spawn.contains("peer_reporter_lock_is_live")
                && spawn.contains("spawn_reporter_paused"),
            "the menu reporter must start paused while a foreground donor still holds reporter.lock"
        );
        let refresh = gui
            .split("fn refresh_ui")
            .nth(1)
            .expect("refresh_ui")
            .split("\nfn ")
            .next()
            .unwrap();
        let resume_at = refresh
            .find("handle.resume()")
            .expect("resume paused menu reporter");
        let sync_at = refresh.find("sync_cloud").expect("sync_cloud");
        assert!(
            sync_at < resume_at,
            "queue the adopted snapshot before unpausing so the first POST is not stale idle"
        );
    }

    #[test]
    fn session_lock_write_is_atomic_rename() {
        let src = include_str!("session_lock.rs");
        let write_fn = src
            .split("pub fn write_session_lock")
            .nth(1)
            .expect("write_session_lock")
            .split("pub fn restored_donor_clamshell_bit")
            .next()
            .unwrap();
        assert!(
            write_fn.contains("lock.tmp")
                && write_fn.contains("sync_all")
                && write_fn.contains("rename")
                && write_fn.contains("create_new")
                && !write_fn.contains("set_extension(\"lock.tmp\")"),
            "each writer must use a private tmp in the data dir, then sync+rename"
        );
        assert!(
            write_fn.contains("process::id"),
            "tmp names must include the writer pid so handoff cannot share one inode"
        );
        let restore = src
            .split("pub fn restore_donor_session_lock")
            .nth(1)
            .expect("restore")
            .split("pub fn should_claim_foreground_reporter_lock")
            .next()
            .unwrap();
        assert!(
            restore.contains("write_session_lock"),
            "donor rollback must use the same atomic lock write"
        );
    }

    #[test]
    fn adopt_fails_when_session_lock_cannot_be_rewritten() {
        let macos = include_str!("platform/macos.rs");
        let write_fn = macos
            .split("fn write_lock")
            .nth(1)
            .expect("write_lock")
            .split("fn pid_alive")
            .next()
            .unwrap();
        assert!(
            write_fn.contains("write_session_lock") && write_fn.contains("Result<(), String>"),
            "write_lock must surface a failed session.lock rewrite"
        );
        let _dir = crate::paths::TestDataDir::install();
        crate::paths::ensure_data_dir().unwrap();
        let path = crate::paths::session_lock_path();
        std::fs::write(&path, "pid=1\nclamshell=1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::paths::data_dir();
        let orig = std::fs::metadata(&dir).unwrap().permissions();
        let mut perms = orig.clone();
        perms.set_mode(0o555);
        std::fs::set_permissions(&dir, perms).unwrap();
        let wrote = write_session_lock(std::process::id(), false, Some(1));
        std::fs::set_permissions(&dir, orig).unwrap();
        assert!(
            !wrote,
            "a read-only data dir must not be treated as a successful successor claim"
        );
    }

    #[test]
    fn loop_destroyed_flushes_inactive_cloud() {
        let gui = include_str!("gui.rs");
        let arm = gui
            .split("Event::LoopDestroyed")
            .nth(1)
            .expect("LoopDestroyed")
            .split("_ => {}")
            .next()
            .unwrap();
        let stop_at = arm.find("stop_for_quit").expect("stop_for_quit");
        let flush_at = arm
            .find("flush_cloud_on_quit")
            .expect("flush_cloud_on_quit");
        assert!(
            stop_at < flush_at,
            "LoopDestroyed must flush an inactive snapshot after restoring power policy"
        );
    }

    #[test]
    fn live_menu_lock_survives_foreground_release() {
        assert!(
            !should_release_clamshell_lock(11, Some(22), true),
            "the handing-off process must not drop a live menu's clamshell ownership"
        );
        assert!(should_release_clamshell_lock(11, Some(11), true));
        assert!(should_release_clamshell_lock(11, Some(22), false));
        assert!(should_release_clamshell_lock(11, None, false));
        assert!(
            should_defer_local_controls(false, 11) == should_hold_cloud_commands(false, 11),
            "⌥⌘P before adopt must follow the same live-donor lock as held phone commands"
        );
        let gui = include_str!("gui.rs");
        assert!(
            gui.contains("should_defer_local_controls") && gui.contains("ok_adopted"),
            "handoff IPC must confirm this process adopted, and Toggle must not start over a live donor"
        );
        assert!(
            gui.contains("ipc_owned") && gui.contains("reconcile_remote"),
            "menu reporter starts only after this process owns the IPC socket"
        );
        let _dir = crate::paths::TestDataDir::install();
        if let Some(st) = process_starttime(1) {
            std::fs::write(
                crate::paths::reporter_lock_path(),
                format_lock_text(1, false, Some(st)),
            )
            .unwrap();
            assert!(
                should_hold_cloud_commands(false, std::process::id()),
                "an idle menu must hold phone On while another process owns reporter.lock"
            );
            assert!(
                !should_hold_cloud_commands(true, std::process::id()),
                "an active menu still applies commands locally"
            );
        }
    }

    #[test]
    fn macos_release_power_defers_to_live_peer_lock() {
        let src = include_str!("platform/macos.rs");
        assert!(
            src.contains("should_release_clamshell_lock"),
            "MacPlatform::release_power must not blindly restore clamshell sleep"
        );
        assert!(
            src.contains("should_restore_clamshell"),
            "a live peer that did not claim clamshell still needs the flag restored"
        );
        assert!(
            src.contains("pid_alive"),
            "ownership follows the pid recorded in session.lock"
        );
        let panic_hook = src
            .split("fn install_panic_cleanup")
            .nth(1)
            .expect("panic cleanup");
        assert!(
            panic_hook.contains("should_restore_clamshell")
                && panic_hook.contains("should_release_clamshell_lock"),
            "panic while OWNS_POWER must not wipe a successor's session.lock"
        );
    }

    #[test]
    fn restores_clamshell_when_successor_did_not_claim_it() {
        assert!(
            should_restore_clamshell(10, Some((20, false)), true),
            "menu adopted without lid-awake: the handing-off process must clear the global flag"
        );
        assert!(
            !should_release_clamshell_lock(10, Some(20), true),
            "keep the menu's session.lock even when it did not take clamshell"
        );
    }

    #[test]
    fn leaves_clamshell_when_successor_claimed_it() {
        assert!(!should_restore_clamshell(10, Some((20, true)), true));
        assert!(!should_release_clamshell_lock(10, Some(20), true));
    }

    #[test]
    fn restores_clamshell_for_own_or_dead_lock() {
        assert!(should_restore_clamshell(10, Some((10, true)), true));
        assert!(should_restore_clamshell(10, Some((20, true)), false));
        assert!(should_restore_clamshell(10, None, false));
    }

    #[test]
    fn successor_clears_unclaimed_inherited_clamshell_before_ack() {
        assert!(
            should_clear_unclaimed_clamshell(false),
            "menu adopted without lid-awake must restore the inherited flag itself"
        );
        assert!(
            !should_clear_unclaimed_clamshell(true),
            "menu that claims clamshell keeps the flag disabled"
        );
        let macos = include_str!("platform/macos.rs");
        assert!(
            macos.contains("should_clear_unclaimed_clamshell"),
            "apply_power must restore an inherited flag when this process is not claiming it"
        );
        let apply = macos.split("fn apply_power").nth(1).expect("apply_power");
        let fail_at = apply
            .find("idle_assertion_failed")
            .expect("assertion failure path");
        let clear_at = apply
            .find("should_clear_unclaimed_clamshell")
            .expect("inherited clear");
        assert!(
            fail_at < clear_at,
            "do not restore inherited clamshell before PreventUserIdleSystemSleep succeeds"
        );
        let ipc = include_str!("gui.rs");
        let on_arm = ipc
            .split("IpcRequest::On")
            .nth(1)
            .expect("On arm")
            .split("IpcRequest::Off")
            .next()
            .unwrap();
        let dispatch_at = on_arm.find("dispatch(").expect("handoff dispatch");
        let reply_at = on_arm.find("ok_status").expect("handoff ack");
        assert!(
            dispatch_at < reply_at,
            "restore via ApplyPower must happen before the IPC ok that lets the donor detach"
        );
    }

    #[test]
    fn unclaimed_clamshell_restore_failure_does_not_write_lock() {
        let macos = include_str!("platform/macos.rs");
        let apply = macos.split("fn apply_power").nth(1).expect("apply_power");
        let clear_at = apply
            .find("should_clear_unclaimed_clamshell")
            .expect("inherited clear");
        let clear_arm = &apply[clear_at..];
        let restore_fail = clear_arm
            .find("if !set_clamshell_sleep_disabled(false)")
            .expect("restore failure must be checked");
        let fail_arm = &clear_arm[restore_fail..];
        let err_at = fail_arm
            .find("return Err")
            .expect("adopt fails if restore fails");
        let write_at = apply
            .find("write_lock")
            .expect("success path still records the lock");
        let fail_abs = clear_at + restore_fail + err_at;
        assert!(
            fail_abs < write_at,
            "a failed IOKit restore must not write clamshell=0 before returning Err"
        );
        assert!(
            fail_arm.contains("owns_power = true") && fail_arm.contains("release_power"),
            "keep ownership so release_power sees a live donor lock and does not clear the flag"
        );
        assert!(
            fail_arm.contains("should_fail_unclaimed_clamshell_restore"),
            "restore failure is fatal when a foreign lock recorded clamshell=1"
        );
        assert!(
            fail_arm.contains("should_keep_inherited_clamshell_lock")
                || fail_arm.contains("keep_inherited_lock"),
            "failed restore must mark the inherited lock so release_power does not delete it"
        );
        assert!(
            should_keep_inherited_clamshell_lock(true, Some((20, true)), 10),
            "a dead donor clamshell=1 lock must survive a failed restore"
        );
        assert!(!should_keep_inherited_clamshell_lock(
            false,
            Some((20, true)),
            10
        ));
        assert!(!should_keep_inherited_clamshell_lock(
            true,
            Some((10, true)),
            10
        ));
        assert!(!should_keep_inherited_clamshell_lock(
            true,
            Some((20, false)),
            10
        ));
        let release = macos
            .split("fn release_power")
            .nth(1)
            .expect("release_power");
        assert!(
            release.contains("should_keep_inherited_clamshell_lock"),
            "release_power must not delete the recovery lock after a failed inherited restore"
        );
    }

    #[test]
    fn restore_failure_is_fatal_only_with_live_inherited_clamshell() {
        assert!(
            !should_fail_unclaimed_clamshell_restore(false, None, false, 10),
            "lid-open display-off with no donor must still start if IOKit restore is unavailable"
        );
        assert!(!should_fail_unclaimed_clamshell_restore(
            false,
            Some((10, true)),
            true,
            10
        ));
        assert!(!should_fail_unclaimed_clamshell_restore(
            false,
            Some((20, false)),
            true,
            10
        ));
        assert!(!should_fail_unclaimed_clamshell_restore(
            true,
            Some((20, true)),
            true,
            10
        ));
        assert!(
            should_fail_unclaimed_clamshell_restore(false, Some((20, true)), true, 10),
            "handoff must not ack if inherited clamshell=1 cannot be restored"
        );
        assert!(
            should_fail_unclaimed_clamshell_restore(false, Some((20, true)), false, 10),
            "a dead donor lock that recorded clamshell=1 still needs a successful restore"
        );
    }

    #[test]
    fn deferred_escape_stops_after_adopt() {
        let mut pending = false;
        note_deferred_escape(false, &mut pending);
        assert!(
            !pending,
            "Toggle while this process owns standby is not an overlapping escape"
        );
        note_deferred_escape(true, &mut pending);
        assert!(pending, "⌥⌘P during the donor overlap must not be dropped");
        note_deferred_escape(false, &mut pending);
        assert!(
            pending,
            "a later non-deferred action must not forget the escape"
        );
        assert!(
            !take_pending_stop_on_adopt(false, &mut pending),
            "failed adopt must keep the escape for a later handoff"
        );
        assert!(pending);
        assert!(
            take_pending_stop_after_handoff(false, true, &mut pending),
            "stop_donor after a zero-remaining handoff must still apply the remembered Off"
        );
        assert!(!pending);
        note_deferred_escape(true, &mut pending);
        assert!(take_pending_stop_on_adopt(true, &mut pending));
        assert!(!pending);
        assert!(
            !take_pending_stop_on_adopt(true, &mut pending),
            "the remembered escape is one-shot"
        );
        let gui = include_str!("gui.rs");
        assert!(
            gui.contains("note_deferred_escape") && gui.contains("take_pending_stop_after_handoff"),
            "menu Toggle / ⌥⌘P must record a pending stop and apply it after adopt or stop_donor"
        );
        let loop_src = gui
            .split("while let Ok(incoming) = ipc_rx.try_recv()")
            .nth(1)
            .expect("ipc loop")
            .split("match event")
            .next()
            .unwrap();
        let drain_at = loop_src
            .rfind("apply_polled_commands")
            .expect("post-handoff drain");
        let stop_at = loop_src
            .find("take_pending_stop_after_handoff")
            .expect("escape after held commands");
        assert!(
            drain_at < stop_at,
            "held phone On must not restart standby after the remembered escape"
        );
        let handle = gui
            .split("fn handle_ipc")
            .nth(1)
            .expect("handle_ipc")
            .split("fn local_controls_deferred")
            .next()
            .unwrap();
        let send_at = handle.find("reply.send").expect("IPC reply");
        assert!(
            !handle.contains("take_pending_stop_after_handoff")
                && !handle.contains("take_pending_stop_on_adopt"),
            "do not Stop inside handle_ipc before the handoff_first drain"
        );
        assert!(
            loop_src.find("handle_ipc").expect("handle_ipc call") < stop_at && send_at > 0,
            "donor must see adopted+active before the menu applies the remembered escape"
        );
        let off = gui
            .split("IpcRequest::Off")
            .nth(1)
            .expect("Off")
            .split("IpcRequest::Toggle")
            .next()
            .unwrap();
        assert!(
            off.contains("note_deferred_escape")
                && (off.contains("should_record_deferred_off")
                    || off.contains("local_controls_deferred")),
            "never-sleep off during the donor overlap must record the same pending stop"
        );
        assert!(
            should_record_deferred_off(false, true),
            "idle menu + live donor: Off is an escape, not a no-op"
        );
        assert!(!should_record_deferred_off(true, false));
        assert!(!should_record_deferred_off(false, false));
        let flush = gui
            .split("fn flush_cloud_on_quit")
            .nth(1)
            .expect("flush_cloud_on_quit")
            .split("fn handle_menu_event")
            .next()
            .unwrap();
        assert!(
            flush.contains("should_detach_cloud_on_quit") && flush.contains("detach"),
            "quit before adopt must detach so the donor can resume without an offline heartbeat"
        );
        assert!(
            should_detach_cloud_on_quit(false, 11) == should_defer_local_controls(false, 11),
            "pre-adopt quit follows the same live-donor lock as deferred Toggle"
        );
        assert!(
            should_stop_donor_on_failed_handoff(true, false, true),
            "failed adopt must still deliver a deferred Off to the live donor"
        );
        assert!(!should_stop_donor_on_failed_handoff(true, true, true));
        assert!(!should_stop_donor_on_failed_handoff(true, false, false));
        assert!(!should_stop_donor_on_failed_handoff(false, false, true));
        assert!(
            handle.contains("should_stop_donor_on_failed_handoff")
                && (handle.contains("stop_donor") || gui.contains("stop_donor")),
            "handoff IPC must tell the donor to stop when adopt cannot apply the deferred Off"
        );
        let fg = include_str!("foreground.rs");
        assert!(
            fg.contains("donor_should_stop"),
            "foreground must honor stop_donor instead of resuming standby after a failed adopt"
        );
        assert!(
            gui.contains("menu_confirms_prior_handoff"),
            "a lost handoff reply must still confirm this donor's already-adopted session"
        );
        assert!(
            gui.contains("menu_already_processed_handoff")
                && handle.contains("should_stop_donor_after_ended_prior_handoff"),
            "matching handoff id after the menu stopped must stop the donor, not dispatch again"
        );
        let persist_at = handle
            .find("write_handoff_ack")
            .expect("persist the adopt/stop decision before the IPC reply");
        let send_at = handle.rfind("reply.send").expect("final IPC reply");
        assert!(
            persist_at < send_at,
            "a timed-out donor must be able to read the ack after the menu has already quit"
        );
        assert!(
            handle.contains("should_reject_adopt_if_ack_unpersisted")
                && handle.contains("handoff_ack_failed"),
            "adopt must roll back when handoff.ack cannot be written"
        );
        assert!(
            handle.contains("should_reject_stop_if_ack_unpersisted"),
            "a deferred Off must persist Stop before the donor can rely on the IPC reply"
        );
        assert!(
            loop_src.contains("should_skip_handoff_drain_after_ack_failure")
                && loop_src.contains("skip_drain"),
            "held phone On must not restart standby after an unpersisted adopt rollback"
        );
        assert!(
            flush.contains("mark_handoff_ack_reporter_gone")
                && flush.contains("should_flush_offline_if_ack_reporter_clear_failed")
                && flush.contains("should_flush_offline_after_abandoning_successor_reporter")
                && flush.contains("successor_reporter_live_on_matching_ack")
                && flush.contains("publish_and_flush"),
            "Quit must clear ack reporter ownership, flush if that clear fails, and flush after abandoning this session's reporter=1"
        );
        assert!(
            handle.contains("handoff_ack_reporter")
                && handle.contains("reporter_running")
                && gui.contains("reporter_is_running")
                && !handle.contains("resp.reporter = Some(identity.is_some())")
                && handle.contains("resp.reporter"),
            "adopt IPC must report a running reporter thread, not only a loaded identity"
        );
        assert!(
            handle.contains("should_keep_unpersisted_stop_donor_after_kept_adopt"),
            "an unpersisted Stop after a kept adopt must still reach the live donor"
        );
        assert!(
            should_restore_donor_lock_on_adopt_rollback(true)
                && !should_restore_donor_lock_on_adopt_rollback(false)
                && handle.contains("restore_donor_session_lock")
                && handle.contains("parse_handoff_owner"),
            "adopt rollback must return session.lock to the donor before ReleasePower"
        );
        assert!(
            should_rollback_adopt_after_donor_lock_restore(true, true)
                && !should_rollback_adopt_after_donor_lock_restore(true, false)
                && !should_rollback_adopt_after_donor_lock_restore(false, true)
                && handle.contains("should_rollback_adopt_after_donor_lock_restore"),
            "do not Stop the adopted session unless the donor lock was actually restored"
        );
        let restore_at = handle
            .find("restore_donor_session_lock")
            .expect("restore donor lock");
        let rollback_stop = handle[restore_at..]
            .find("StopReason::AppQuit")
            .expect("rollback Stop");
        assert!(
            rollback_stop > 0,
            "restore the donor lock before the adopted-session Stop"
        );
        let _dir = crate::paths::TestDataDir::install();
        std::fs::write(
            crate::paths::session_lock_path(),
            format_lock_text(99, true, Some(1)),
        )
        .unwrap();
        assert!(
            !restored_donor_clamshell_bit(false, true),
            "rollback must keep the donor's original clamshell bit, not the successor's"
        );
        assert!(restored_donor_clamshell_bit(true, false));
        assert!(restore_donor_session_lock(11, 100, false));
        let rec = read_lock_record().expect("donor lock");
        assert_eq!(rec.pid, 11);
        assert_eq!(rec.starttime, Some(100));
        assert!(
            !rec.clamshell,
            "a donor that never claimed clamshell must not inherit the menu's clamshell=1"
        );
        assert!(
            loop_src.contains("stop_donor"),
            "apply the remembered escape after drain when the response stops the donor"
        );
    }

    #[test]
    fn reused_pid_is_not_a_live_lock_holder() {
        assert!(
            lock_holder_is_live(true, None, Some(99)),
            "locks written before starttime still treat a live pid as the owner"
        );
        assert!(lock_holder_is_live(true, Some(10), Some(10)));
        assert!(
            !lock_holder_is_live(true, Some(10), Some(99)),
            "SIGKILL leftover must not follow a later process that reused the pid"
        );
        assert!(!lock_holder_is_live(true, Some(10), None));
        assert!(!lock_holder_is_live(false, Some(10), Some(10)));
        assert!(
            lock_holder_is_live(true, Some(UNVERIFIED_INSTANCE_TOKEN), None),
            "a peer-visible unverified token keeps a live pid as owner when sysctl is unreadable"
        );
        assert!(
            lock_holder_is_live(true, Some(UNVERIFIED_INSTANCE_TOKEN), Some(99)),
            "a recovered starttime lookup must not evict a still-live unverified owner"
        );
        assert!(!lock_holder_is_live(
            false,
            Some(UNVERIFIED_INSTANCE_TOKEN),
            None
        ));
        let rec = parse_lock_record("pid=22\nclamshell=1\nstarttime=4242\n");
        assert_eq!(rec.pid, 22);
        assert!(rec.clamshell);
        assert_eq!(rec.starttime, Some(4242));
        let body = format_lock_text(22, true, Some(4242));
        assert_eq!(parse_lock_text(&body), (22, true));
        assert!(
            body.contains("starttime=4242"),
            "session.lock must record a start token, not only pid"
        );
        let macos = include_str!("platform/macos.rs");
        assert!(
            macos.contains("write_session_lock") && macos.contains("process_instance_token"),
            "apply_power must write pid+instance token so orphan cleanup can reject pid reuse"
        );
        assert!(
            macos.contains("lock_holder_is_live") && macos.contains("observed_instance_token"),
            "cleanup / release_power must validate starttime, not only kill(pid, 0)"
        );
        let own = process_instance_token(std::process::id());
        assert_ne!(
            own, 0,
            "new locks must always record a process instance token"
        );
        let token_src = include_str!("session_lock.rs")
            .split("pub fn process_instance_token")
            .nth(1)
            .expect("process_instance_token")
            .split("pub fn observed_instance_token")
            .next()
            .unwrap();
        assert!(
            token_src.contains("UNVERIFIED_INSTANCE_TOKEN")
                && !token_src.contains("local_instance_token"),
            "sysctl-miss fallback must be a peer-readable sentinel, not a process-local random"
        );
        assert_eq!(observed_instance_token(std::process::id()), Some(own));
        assert!(lock_holder_is_live(
            pid_is_alive(std::process::id()),
            Some(own),
            observed_instance_token(std::process::id())
        ));
        assert_eq!(
            parse_proc_stat_starttime(
                "1 (init) S 0 1 1 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 42 0 0 0 0 0 0 0 0 0 0 0"
            ),
            Some(42)
        );
    }

    #[test]
    fn second_foreground_does_not_claim_a_live_reporter_lock() {
        assert!(
            !should_claim_reporter_lock(11, Some(22), true),
            "two never-sleep on processes must not both poll the same identity"
        );
        assert!(should_claim_reporter_lock(11, Some(22), false));
        assert!(should_claim_reporter_lock(11, Some(11), true));
        assert!(should_claim_reporter_lock(11, None, false));
        assert!(
            should_abort_foreground_without_reporter_lock(true, false),
            "a second never-sleep on must not Start when reporter.lock is denied"
        );
        assert!(!should_abort_foreground_without_reporter_lock(true, true));
        assert!(
            !should_abort_foreground_without_reporter_lock(false, false),
            "a live menu still accepts handoff; lock denial must not reject that path"
        );
        assert!(should_claim_foreground_reporter_lock(true, true));
        assert!(!should_claim_foreground_reporter_lock(true, false));
        assert!(
            should_claim_foreground_reporter_lock(false, true),
            "foreground admission is reporter.lock even when cloud is disabled"
        );
        let claim_src = include_str!("session_lock.rs")
            .split("pub fn try_claim_reporter_lock")
            .nth(1)
            .expect("try_claim_reporter_lock")
            .split("pub fn release_reporter_lock")
            .next()
            .unwrap();
        assert!(
            !claim_src.contains("remove_file"),
            "replacing a leftover reporter.lock must not unlink then create_new"
        );
        let _dir = crate::paths::TestDataDir::install();
        let ours = std::process::id();
        assert!(
            try_claim_reporter_lock(ours),
            "the first foreground reporter may start cloud polling"
        );
        assert!(
            try_claim_reporter_lock(ours),
            "the owning process may re-enter the lock"
        );
        release_reporter_lock(ours);
        std::fs::write(
            crate::paths::reporter_lock_path(),
            format_lock_text(1, false, process_starttime(1)),
        )
        .unwrap();
        assert!(
            try_claim_reporter_lock(ours),
            "a leftover reporter.lock without a held flock must be taken over atomically"
        );
        release_reporter_lock(ours);
        let foreign = hold_reporter_lock_for_test();
        assert!(
            !try_claim_reporter_lock(ours),
            "a second foreground must not steal a held reporter lock"
        );
        drop(foreign);
        assert!(
            try_claim_reporter_lock(ours),
            "releasing the held lock must allow the next foreground to poll"
        );
        release_reporter_lock(ours);
        assert!(
            read_reporter_lock().is_some(),
            "release must drop the fd without unlinking, so a racer cannot lock a new inode"
        );
        let release_src = include_str!("session_lock.rs")
            .split("pub fn release_reporter_lock")
            .nth(1)
            .expect("release_reporter_lock")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            !release_src.contains("remove_file"),
            "ownership is the held fd; unlinking during release splits the inode"
        );
        let fg = include_str!("foreground.rs");
        assert!(
            fg.contains("try_claim_reporter_lock") && fg.contains("release_reporter_lock"),
            "foreground must take exclusive reporter ownership before heartbeats"
        );
        let start = fg.find("pub fn run_foreground").expect("run_foreground");
        let before_loop = fg[start..]
            .split("while running")
            .next()
            .expect("foreground loop");
        let claim_at = before_loop
            .find("try_claim_reporter_lock")
            .expect("claim reporter.lock before Start");
        let refuse_at = before_loop
            .find("should_refuse_foreground_while_menu_live")
            .expect("refuse Start while ipc.sock is live");
        let dispatch_at = before_loop
            .find("dispatch(")
            .expect("Start dispatch after the reporter claim");
        let second_refuse_at = claim_at
            + before_loop[claim_at..]
                .find("should_refuse_foreground_while_menu_live")
                .expect("re-check ipc.sock after claiming reporter.lock");
        assert!(
            refuse_at < claim_at
                && claim_at < second_refuse_at
                && second_refuse_at < dispatch_at
                && before_loop.contains("should_abort_foreground_without_reporter_lock")
                && before_loop.contains("should_claim_foreground_reporter_lock"),
            "foreground admission must claim reporter.lock then re-check the menu before Start"
        );
        let after_second_refuse = &before_loop[second_refuse_at..dispatch_at];
        assert!(
            after_second_refuse.contains("release_reporter_lock")
                && after_second_refuse.contains("menu_ipc_timed_out"),
            "a menu that binds after the claim must drop the lock and not dispatch Start"
        );
        let take = fg
            .split("fn take_foreground_reporter")
            .nth(1)
            .expect("take_foreground_reporter")
            .split("fn spawn_foreground_reporter")
            .next()
            .unwrap();
        assert!(
            !take.contains("release_reporter_lock"),
            "the lock must stay held until detach or publish_and_flush has joined"
        );
        for marker in ["handle.detach();", "publish_and_flush("] {
            let at = fg.find(marker).unwrap_or_else(|| panic!("{marker}"));
            let after = &fg[at + marker.len()..];
            let release_at = after
                .find("release_reporter_lock")
                .unwrap_or_else(|| panic!("release after {marker}"));
            let next_fn = after.find("\nfn ").unwrap_or(after.len());
            assert!(
                release_at < next_fn,
                "{marker} must drop the lock only after the reporter has stopped"
            );
        }
    }
}
