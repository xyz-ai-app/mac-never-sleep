#[cfg(any(test, target_os = "macos"))]
use std::fs::File;
#[cfg(any(test, target_os = "macos"))]
use std::io::Write;
#[cfg(any(test, target_os = "macos"))]
use std::path::{Path, PathBuf};
#[cfg(any(test, target_os = "macos"))]
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use never_sleep_core::{parse_duration_pref_in, DurationPref, JsonStatus, Lang, Tr};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcRequest {
    On {
        #[serde(default)]
        duration: Option<String>,
        /// Leftover seconds of a timed session. Only set on an internal handoff.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remaining_secs: Option<u64>,
        /// Elapsed seconds of the live session. Only set on an internal handoff.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_secs: Option<u64>,
        /// Adopt a live session with remote display semantics (do not fight HID).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        handoff: bool,
        /// Command ids the donor already applied. Only set on an internal handoff.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        applied_command_ids: Vec<String>,
        /// Stable id so a timed-out donor can confirm a handoff that already adopted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handoff_id: Option<String>,
    },
    Off,
    Toggle,
    Status,
    Quit,
    Ping,
    Pair,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<JsonStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pong: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Set only when this process dispatched a handoff onto an idle engine.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub adopted: bool,
    /// Ask the foreground donor to stop after a deferred Off that this process
    /// could not adopt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stop_donor: bool,
    /// Whether this process still owns the cloud reporter after a handoff.
    /// Internal only; CLI JSON omits the field when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporter: Option<bool>,
    /// Ask the live donor to ApplyPower again after a successor IOKit miss.
    /// Internal only; CLI JSON omits the field when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clamshell_reapply: bool,
}

impl IpcRequest {
    pub fn on(duration: Option<String>) -> Self {
        Self::On {
            duration,
            remaining_secs: None,
            elapsed_secs: None,
            handoff: false,
            applied_command_ids: Vec::new(),
            handoff_id: None,
        }
    }

    pub fn handoff(
        duration: Option<String>,
        remaining_secs: Option<u64>,
        elapsed_secs: Option<u64>,
    ) -> Self {
        Self::On {
            duration,
            remaining_secs,
            elapsed_secs,
            handoff: true,
            applied_command_ids: Vec::new(),
            handoff_id: None,
        }
    }

    pub fn with_applied_command_ids(self, ids: Vec<String>) -> Self {
        match self {
            Self::On {
                duration,
                remaining_secs,
                elapsed_secs,
                handoff,
                handoff_id,
                ..
            } => Self::On {
                duration,
                remaining_secs,
                elapsed_secs,
                handoff,
                applied_command_ids: ids,
                handoff_id,
            },
            other => other,
        }
    }

    pub fn with_handoff_id(self, id: impl Into<String>) -> Self {
        match self {
            Self::On {
                duration,
                remaining_secs,
                elapsed_secs,
                handoff,
                applied_command_ids,
                ..
            } => Self::On {
                duration,
                remaining_secs,
                elapsed_secs,
                handoff,
                applied_command_ids,
                handoff_id: Some(id.into()),
            },
            other => other,
        }
    }

    #[cfg(any(test, target_os = "macos"))]
    pub fn applied_command_ids(&self) -> &[String] {
        match self {
            Self::On {
                applied_command_ids,
                ..
            } => applied_command_ids,
            _ => &[],
        }
    }

    #[cfg(test)]
    pub fn handoff_id(&self) -> Option<&str> {
        match self {
            Self::On { handoff_id, .. } => handoff_id.as_deref(),
            _ => None,
        }
    }

    #[cfg(any(test, target_os = "macos"))]
    pub fn is_handoff(&self) -> bool {
        matches!(self, Self::On { handoff: true, .. })
    }
}

impl IpcResponse {
    pub fn ok_status(status: JsonStatus) -> Self {
        Self {
            ok: true,
            error: None,
            status: Some(status),
            pong: None,
            pairing_code: None,
            pairing_url: None,
            device_id: None,
            adopted: false,
            stop_donor: false,
            reporter: None,
            clamshell_reapply: false,
        }
    }

    #[cfg(any(test, target_os = "macos"))]
    pub fn ok_adopted(status: JsonStatus) -> Self {
        Self {
            adopted: true,
            ..Self::ok_status(status)
        }
    }

    #[cfg(any(test, target_os = "macos"))]
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            status: None,
            pong: None,
            pairing_code: None,
            pairing_url: None,
            device_id: None,
            adopted: false,
            stop_donor: false,
            reporter: None,
            clamshell_reapply: false,
        }
    }

    #[cfg(any(test, target_os = "macos"))]
    pub fn pong() -> Self {
        Self {
            ok: true,
            error: None,
            status: None,
            pong: Some(true),
            pairing_code: None,
            pairing_url: None,
            device_id: None,
            adopted: false,
            stop_donor: false,
            reporter: None,
            clamshell_reapply: false,
        }
    }

    #[cfg(any(test, target_os = "macos"))]
    pub fn ok_pairing(code: String, url: String, device_id: Option<String>) -> Self {
        Self {
            ok: true,
            error: None,
            status: None,
            pong: None,
            pairing_code: Some(code),
            pairing_url: Some(url),
            device_id,
            adopted: false,
            stop_donor: false,
            reporter: None,
            clamshell_reapply: false,
        }
    }
}

pub fn parse_on_duration_in(raw: Option<&str>, lang: Lang) -> Result<Option<DurationPref>, String> {
    match raw {
        None => Ok(None),
        Some(s) => parse_duration_pref_in(s, lang).map(Some),
    }
}

/// Stable CLI/IPC spelling for a duration so a foreground fallback can hand
/// the live session to the menu with `IpcRequest::On`.
pub fn duration_pref_to_ipc(pref: DurationPref) -> String {
    match pref {
        DurationPref::Indefinite => "indefinite".into(),
        DurationPref::Hours { hours } => format!("{hours}h"),
        DurationPref::UntilLocal { hour, minute } => {
            format!("until={hour:02}:{minute:02}")
        }
    }
}

pub fn menu_accepted_handoff(resp: &IpcResponse) -> bool {
    resp.ok && resp.adopted && resp.status.as_ref().is_some_and(|status| status.active)
}

pub fn donor_should_stop(resp: &IpcResponse) -> bool {
    resp.stop_donor
}

pub fn donor_should_reapply_clamshell(resp: &IpcResponse) -> bool {
    resp.clamshell_reapply
}

/// This donor already completed (or tried) this handoff id, even if standby ended.
#[cfg(any(test, target_os = "macos"))]
pub fn menu_already_processed_handoff(
    handoff: bool,
    request_id: Option<&str>,
    last_id: Option<&str>,
) -> bool {
    handoff && request_id.filter(|id| !id.is_empty()).is_some() && request_id == last_id
}

/// A lost handoff reply must still count as this donor's adopt on retry.
#[cfg(any(test, target_os = "macos"))]
pub fn menu_confirms_prior_handoff(
    handoff: bool,
    engine_active: bool,
    request_id: Option<&str>,
    last_id: Option<&str>,
) -> bool {
    menu_already_processed_handoff(handoff, request_id, last_id) && engine_active
}

/// Matching id after the adopted session ended: stop the donor, do not re-dispatch.
#[cfg(any(test, target_os = "macos"))]
pub fn should_stop_donor_after_ended_prior_handoff(
    already_processed: bool,
    engine_active: bool,
) -> bool {
    already_processed && !engine_active
}

/// `{pid}-{starttime}-{seq}` so a reused PID cannot collide with a prior adopt.
/// Optional trailing `-0`/`-1` carries the donor's original clamshell bit.
pub fn format_handoff_id(
    pid: u32,
    starttime: Option<u64>,
    seq: u64,
    clamshell: Option<bool>,
) -> String {
    let base = match starttime {
        Some(start) => format!("{pid}-{start}-{seq}"),
        None => format!("{pid}-{seq}-{seq}"),
    };
    match clamshell {
        Some(claimed) => format!("{base}-{}", u8::from(claimed)),
        None => base,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, target_os = "macos"))]
pub struct HandoffOwner {
    pub pid: u32,
    pub starttime: u64,
    pub clamshell: Option<bool>,
}

#[cfg(any(test, target_os = "macos"))]
pub fn parse_handoff_owner(id: &str) -> Option<HandoffOwner> {
    let mut parts = id.split('-');
    let pid = parts.next()?.parse().ok()?;
    let start = parts.next()?.parse().ok()?;
    let _seq: u64 = parts.next()?.parse().ok()?;
    let clamshell = match parts.next() {
        None => None,
        Some("0") => Some(false),
        Some("1") => Some(true),
        Some(_) => return None,
    };
    if parts.next().is_some() || pid == 0 {
        return None;
    }
    Some(HandoffOwner {
        pid,
        starttime: start,
        clamshell,
    })
}

/// Ctrl-C during handoff must stop a successor that may already have adopted.
pub fn should_stop_successor_on_cancel(
    cancelled: bool,
    handoff_attempted: bool,
    menu_reachable: bool,
) -> bool {
    cancelled && handoff_attempted && menu_reachable
}

/// Persisted so a timed-out donor can still stop after the menu adopts and quits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffAckOutcome {
    Adopted,
    Stop,
}

/// Matching persisted ack, plus whether the successor still owns a reporter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffAck {
    pub id: String,
    pub outcome: HandoffAckOutcome,
    pub reporter: bool,
}

#[cfg(any(test, target_os = "macos"))]
pub fn format_handoff_ack(id: &str, outcome: HandoffAckOutcome, reporter: bool) -> String {
    let outcome = match outcome {
        HandoffAckOutcome::Adopted => "adopted",
        HandoffAckOutcome::Stop => "stop",
    };
    format!(
        "handoff_id={id}\noutcome={outcome}\nreporter={}\n",
        u8::from(reporter)
    )
}

pub fn parse_handoff_ack(s: &str) -> Option<HandoffAck> {
    let mut id = None;
    let mut outcome = None;
    let mut reporter = false;
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("handoff_id=") {
            let v = v.trim();
            if !v.is_empty() {
                id = Some(v.to_string());
            }
        }
        if let Some(v) = line.strip_prefix("outcome=") {
            outcome = match v.trim() {
                "adopted" => Some(HandoffAckOutcome::Adopted),
                "stop" => Some(HandoffAckOutcome::Stop),
                _ => None,
            };
        }
        if let Some(v) = line.strip_prefix("reporter=") {
            reporter = v.trim() == "1";
        }
    }
    Some(HandoffAck {
        id: id?,
        outcome: outcome?,
        reporter,
    })
}

/// Matching persisted ack stops this donor even if another menu already rebound IPC.
pub fn donor_should_stop_after_successor_gone(
    our_handoff_id: Option<&str>,
    _successor_live: bool,
    persisted_id: Option<&str>,
) -> bool {
    let Some(ours) = our_handoff_id.filter(|id| !id.is_empty()) else {
        return false;
    };
    persisted_id == Some(ours)
}

/// After an ack-driven stop, flush offline only when no successor reporter remains.
/// Socket liveness is not enough: a quitting menu can still accept Ping after
/// its reporter has already flushed.
pub fn donor_should_flush_offline_after_ack(successor_reporter: bool) -> bool {
    !successor_reporter
}

/// IPC adopt: flush only when the successor explicitly reports no cloud reporter.
/// A missing field (older menus) keeps detach so a live menu reporter is not marked offline.
#[cfg(test)]
pub fn donor_should_flush_offline_after_ipc_adopt(successor_reporter: Option<bool>) -> bool {
    successor_reporter == Some(false)
}

/// Prefer the IPC reporter bit; fall back to the persisted ack; default live for older menus.
/// A matching ack with reporter=0 is newer than an in-flight adopted reply (Quit rewrites
/// the ack after sending it) and must flush rather than detach.
pub fn successor_reporter_after_adopt(
    ipc_reporter: Option<bool>,
    ack_reporter: Option<bool>,
) -> bool {
    if ipc_reporter == Some(false) || ack_reporter == Some(false) {
        return false;
    }
    ipc_reporter.or(ack_reporter).unwrap_or(true)
}

/// Quit cleared reporter=1 while the donor may still detach from a stale adopted reply.
#[cfg(any(test, target_os = "macos"))]
pub fn should_flush_offline_after_abandoning_successor_reporter(
    had_live_reporter: bool,
    clear_ok: bool,
) -> bool {
    had_live_reporter && clear_ok
}

/// A stale `reporter=1` ack from a crashed prior menu must not count as this session's reporter.
#[cfg(any(test, target_os = "macos"))]
pub fn successor_reporter_live_on_matching_ack(
    our_handoff_id: Option<&str>,
    ack_id: Option<&str>,
    ack_reporter: bool,
) -> bool {
    matching_handoff_ack_reporter(our_handoff_id, ack_id, ack_reporter) == Some(true)
}

/// Re-check a matching ack after the adopted IPC snapshot looked live.
pub fn successor_reporter_after_fresh_ack(
    previous: bool,
    ipc_reporter: Option<bool>,
    fresh_ack_reporter: Option<bool>,
) -> bool {
    if !previous {
        return false;
    }
    successor_reporter_after_adopt(ipc_reporter, fresh_ack_reporter)
}

/// Older menus omit `resp.reporter`; only a matching `handoff.ack` may fill it in.
pub fn matching_handoff_ack_reporter(
    our_handoff_id: Option<&str>,
    ack_id: Option<&str>,
    ack_reporter: bool,
) -> Option<bool> {
    let ours = our_handoff_id.filter(|id| !id.is_empty())?;
    let theirs = ack_id.filter(|id| !id.is_empty())?;
    (ours == theirs).then_some(ack_reporter)
}

/// User stopped a successor that we kept after an unpersisted adopt; tell the
/// still-connected donor even if Stop cannot be written to `handoff.ack`.
#[cfg(any(test, target_os = "macos"))]
pub fn should_keep_unpersisted_stop_donor_after_kept_adopt(
    reject_stop: bool,
    last_handoff_present: bool,
    adopted_now: bool,
) -> bool {
    reject_stop && last_handoff_present && !adopted_now
}

/// A detach Quit that cannot clear this session's `reporter=1` must POST offline.
/// An unmatched stale ack must not force flush while another donor is live.
#[cfg(any(test, target_os = "macos"))]
pub fn should_flush_offline_if_ack_reporter_clear_failed(
    clear_ok: bool,
    would_detach: bool,
    matching_live_reporter: bool,
) -> bool {
    would_detach && !clear_ok && matching_live_reporter
}

/// Persist whether this process still owns the cloud reporter, including Stop
/// acks after a failed adopt: the menu reporter may still be heartbeating.
#[cfg(any(test, target_os = "macos"))]
pub fn handoff_ack_reporter(owns_reporter: bool) -> bool {
    owns_reporter
}

#[cfg(any(test, target_os = "macos"))]
pub fn should_persist_handoff_ack(handoff: bool, adopted: bool, stop_donor: bool) -> bool {
    handoff && (adopted || stop_donor)
}

/// A just-dispatched adopt must not report success if `handoff.ack` was not written.
#[cfg(any(test, target_os = "macos"))]
pub fn should_reject_adopt_if_ack_unpersisted(
    adopted: bool,
    dispatched_now: bool,
    persist_ok: bool,
) -> bool {
    adopted && dispatched_now && !persist_ok
}

/// A deferred Off must not report `stop_donor` unless the Stop ack was written.
#[cfg(any(test, target_os = "macos"))]
pub fn should_reject_stop_if_ack_unpersisted(stop_donor: bool, persist_ok: bool) -> bool {
    stop_donor && !persist_ok
}

/// Held phone On must not restart the menu after an unpersisted adopt/stop.
#[cfg(any(test, target_os = "macos"))]
pub fn should_skip_handoff_drain_after_ack_failure(ack_unpersisted: bool) -> bool {
    ack_unpersisted
}

pub fn read_handoff_ack() -> Option<HandoffAck> {
    let text = std::fs::read_to_string(crate::paths::handoff_ack_path()).ok()?;
    parse_handoff_ack(&text)
}

#[cfg(any(test, target_os = "macos"))]
static HANDOFF_ACK_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(any(test, target_os = "macos"))]
fn create_private_handoff_ack_tmp(path: &Path) -> std::io::Result<(PathBuf, File)> {
    let pid = std::process::id();
    let mut last_err = None;
    for _ in 0..32 {
        let seq = HANDOFF_ACK_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_file_name(format!("handoff.ack.tmp.{pid}.{seq}"));
        match File::create_new(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::AlreadyExists, "handoff.ack tmp")
    }))
}

#[cfg(any(test, target_os = "macos"))]
pub fn write_handoff_ack(
    id: &str,
    outcome: HandoffAckOutcome,
    reporter: bool,
) -> std::io::Result<()> {
    if id.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty handoff id",
        ));
    }
    crate::paths::ensure_data_dir()?;
    let path = crate::paths::handoff_ack_path();
    let body = format_handoff_ack(id, outcome, reporter);
    let (tmp, mut file) = create_private_handoff_ack_tmp(&path)?;
    if let Err(err) = file
        .write_all(body.as_bytes())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    drop(file);
    if let Err(err) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

/// Quit teardown: IPC may still accept Ping after this reporter has flushed.
#[cfg(any(test, target_os = "macos"))]
pub fn mark_handoff_ack_reporter_gone() -> bool {
    let Some(ack) = read_handoff_ack() else {
        return true;
    };
    if !ack.reporter {
        return true;
    }
    write_handoff_ack(&ack.id, ack.outcome, false).is_ok()
}

pub fn clear_handoff_ack() {
    let _ = std::fs::remove_file(crate::paths::handoff_ack_path());
}

/// Map stable IPC error codes to bilingual CLI text. JSON still prints the code.
pub fn human_ipc_error(code: &str, lang: Lang) -> String {
    match code {
        "" => Tr::new(lang).failed().to_string(),
        "remote_disabled" => Tr::new(lang).remote_disabled().to_string(),
        "pairing_unavailable" => Tr::new(lang).pairing_unavailable().to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use never_sleep_core::JsonStatus;

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
    fn remote_disabled_error_stays_machine_readable_and_has_localized_guidance() {
        let response = IpcResponse::err("remote_disabled");
        assert_eq!(
            serde_json::to_value(response).unwrap()["error"],
            "remote_disabled"
        );
        assert_eq!(
            human_ipc_error("remote_disabled", Lang::En),
            "Remote access is off. Enable it in Settings to pair your phone."
        );
        assert_eq!(
            human_ipc_error("remote_disabled", Lang::Zh),
            "远程访问已关闭。请在设置中开启后配对手机。"
        );
    }

    #[test]
    fn request_cmd_tags_are_stable() {
        let cases = [
            (
                IpcRequest::On {
                    duration: Some("8h".into()),
                    remaining_secs: None,
                    elapsed_secs: None,
                    handoff: false,
                    applied_command_ids: Vec::new(),
                    handoff_id: None,
                },
                r#"{"cmd":"on","duration":"8h"}"#,
            ),
            (IpcRequest::Off, r#"{"cmd":"off"}"#),
            (IpcRequest::Toggle, r#"{"cmd":"toggle"}"#),
            (IpcRequest::Status, r#"{"cmd":"status"}"#),
            (IpcRequest::Quit, r#"{"cmd":"quit"}"#),
            (IpcRequest::Ping, r#"{"cmd":"ping"}"#),
            (IpcRequest::Pair, r#"{"cmd":"pair"}"#),
        ];
        for (req, expected) in cases {
            let json = serde_json::to_string(&req).unwrap();
            assert_eq!(json, expected);
            let back: IpcRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(back, req);
        }
    }

    #[test]
    fn parse_on_duration_none_and_hours() {
        assert_eq!(parse_on_duration_in(None, Lang::En).unwrap(), None);
        assert_eq!(
            parse_on_duration_in(Some("3h"), Lang::En).unwrap(),
            Some(DurationPref::Hours { hours: 3 })
        );
        let err = parse_on_duration_in(Some("nope"), Lang::Zh).unwrap_err();
        assert!(err.contains("HH:MM"), "{err}");
        assert_ne!(
            err,
            parse_on_duration_in(Some("nope"), Lang::En).unwrap_err()
        );
        assert_eq!(duration_pref_to_ipc(DurationPref::Indefinite), "indefinite");
        assert_eq!(duration_pref_to_ipc(DurationPref::Hours { hours: 8 }), "8h");
        assert_eq!(
            duration_pref_to_ipc(DurationPref::UntilLocal { hour: 8, minute: 0 }),
            "until=08:00"
        );
    }

    #[test]
    fn handoff_on_carries_remaining_secs() {
        let req = IpcRequest::handoff(Some("8h".into()), Some(3600), Some(7 * 3600));
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(
            json,
            r#"{"cmd":"on","duration":"8h","remaining_secs":3600,"elapsed_secs":25200,"handoff":true}"#
        );
        let back: IpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
        let local = IpcRequest::on(Some("8h".into()));
        assert_eq!(
            serde_json::to_string(&local).unwrap(),
            r#"{"cmd":"on","duration":"8h"}"#
        );
        let mut accepted = sample_status();
        accepted.active = true;
        assert!(
            !menu_accepted_handoff(&IpcResponse::ok_status(accepted.clone())),
            "an already-active menu (⌥⌘P before adopt) must not count as this handoff"
        );
        assert!(menu_accepted_handoff(&IpcResponse::ok_adopted(
            accepted.clone()
        )));
        let adopted_json = serde_json::to_value(IpcResponse::ok_adopted(accepted.clone())).unwrap();
        assert_eq!(adopted_json["adopted"], true);
        assert!(
            serde_json::to_value(IpcResponse::ok_status(accepted.clone()))
                .unwrap()
                .get("adopted")
                .is_none(),
            "CLI status must omit the internal handoff-adopted flag"
        );
        assert!(
            serde_json::to_value(IpcResponse::ok_status(accepted.clone()))
                .unwrap()
                .get("reporter")
                .is_none(),
            "CLI status must omit the internal successor-reporter flag"
        );
        let mut adopted_no_reporter = IpcResponse::ok_adopted(accepted.clone());
        adopted_no_reporter.reporter = Some(false);
        let adopted_reporter_json = serde_json::to_value(&adopted_no_reporter).unwrap();
        assert_eq!(adopted_reporter_json["reporter"], false);
        assert!(
            donor_should_flush_offline_after_ipc_adopt(adopted_no_reporter.reporter),
            "adopt with an explicit reporter=false must flush rather than detach"
        );
        assert!(
            !donor_should_flush_offline_after_ipc_adopt(Some(true)),
            "a live successor reporter must still detach"
        );
        assert!(
            !donor_should_flush_offline_after_ipc_adopt(None),
            "older menus that omit reporter must keep detach"
        );
        assert!(
            !successor_reporter_after_adopt(Some(false), Some(true)),
            "the IPC reporter bit wins over a stale ack"
        );
        assert!(
            !successor_reporter_after_adopt(Some(true), Some(false)),
            "Quit rewrote reporter=0 after the adopted reply; the donor must flush, not detach"
        );
        assert!(!successor_reporter_after_adopt(None, Some(false)));
        assert!(
            successor_reporter_after_adopt(None, None),
            "a missing reporter field must not POST offline over an older live menu"
        );
        assert!(
            matching_handoff_ack_reporter(Some("h1"), Some("h2"), false).is_none(),
            "a stale handoff.ack must not mark a live successor reporter offline"
        );
        assert_eq!(
            matching_handoff_ack_reporter(Some("h1"), Some("h1"), false),
            Some(false)
        );
        assert!(
            successor_reporter_after_adopt(
                None,
                matching_handoff_ack_reporter(Some("h1"), Some("h2"), false)
            ),
            "older menus omit reporter; mismatched ack must keep detach"
        );
        assert!(
            should_keep_unpersisted_stop_donor_after_kept_adopt(true, true, false),
            "⌥⌘P after an unpersisted kept-adopt must still stop the live donor"
        );
        assert!(!should_keep_unpersisted_stop_donor_after_kept_adopt(
            true, false, false
        ));
        assert!(!should_keep_unpersisted_stop_donor_after_kept_adopt(
            true, true, true
        ));
        accepted.active = false;
        assert!(!menu_accepted_handoff(&IpcResponse::ok_adopted(
            accepted.clone()
        )));
        assert!(!menu_accepted_handoff(&IpcResponse::err("denied")));
        let ping = IpcResponse::pong();
        assert!(!menu_accepted_handoff(&ping));
        assert!(req.is_handoff());
        assert!(!IpcRequest::on(Some("8h".into())).is_handoff());
        assert!(!IpcRequest::Ping.is_handoff());
        let with_ids = IpcRequest::handoff(Some("8h".into()), Some(3600), Some(7 * 3600))
            .with_applied_command_ids(vec!["phone-on".into()]);
        assert_eq!(with_ids.applied_command_ids(), ["phone-on"]);
        let ids_json = serde_json::to_string(&with_ids).unwrap();
        assert!(
            ids_json.contains("applied_command_ids") && ids_json.contains("phone-on"),
            "internal handoff must name commands the donor already applied, got {ids_json}"
        );
        assert!(
            !serde_json::to_string(&IpcRequest::on(Some("8h".into())))
                .unwrap()
                .contains("applied_command_ids"),
            "CLI On must keep omitting the internal-only field"
        );
        let mut stop = IpcResponse::ok_status(accepted.clone());
        stop.stop_donor = true;
        let stop_json = serde_json::to_value(&stop).unwrap();
        assert_eq!(stop_json["stop_donor"], true);
        assert!(donor_should_stop(&stop));
        assert!(
            serde_json::to_value(IpcResponse::ok_status(accepted.clone()))
                .unwrap()
                .get("stop_donor")
                .is_none(),
            "CLI status must omit the internal stop-donor flag"
        );
        let mut reapply = IpcResponse::ok_status(accepted.clone());
        reapply.clamshell_reapply = true;
        assert!(donor_should_reapply_clamshell(&reapply));
        assert!(
            serde_json::to_value(IpcResponse::ok_status(accepted))
                .unwrap()
                .get("clamshell_reapply")
                .is_none(),
            "CLI status must omit the internal clamshell-reapply flag"
        );
        let mut live = sample_status();
        live.active = true;
        assert!(!donor_should_stop(&IpcResponse::ok_adopted(live)));
        assert!(
            menu_confirms_prior_handoff(true, true, Some("h1"), Some("h1")),
            "retry after a lost reply must confirm the already-adopted handoff"
        );
        assert!(!menu_confirms_prior_handoff(
            true,
            true,
            Some("h1"),
            Some("h2")
        ));
        assert!(!menu_confirms_prior_handoff(
            true,
            false,
            Some("h1"),
            Some("h1")
        ));
        assert!(
            menu_already_processed_handoff(true, Some("h1"), Some("h1")),
            "a matching id is already processed even after the menu stopped the adopted session"
        );
        assert!(
            should_stop_donor_after_ended_prior_handoff(true, false),
            "retry after ⌥⌘P must tell the donor to stop instead of dispatching Handoff again"
        );
        assert!(!should_stop_donor_after_ended_prior_handoff(true, true));
        assert!(!should_stop_donor_after_ended_prior_handoff(false, false));
        assert!(!menu_already_processed_handoff(
            true,
            Some("h1"),
            Some("h2")
        ));
        assert!(!menu_confirms_prior_handoff(true, true, None, Some("h1")));
        let with_id = IpcRequest::handoff(Some("8h".into()), Some(3600), Some(7 * 3600))
            .with_handoff_id("h1");
        assert_eq!(with_id.handoff_id(), Some("h1"));
        let id_json = serde_json::to_string(&with_id).unwrap();
        assert!(
            id_json.contains("handoff_id") && id_json.contains("h1"),
            "internal handoff must name the adopt attempt, got {id_json}"
        );
        assert!(
            !serde_json::to_string(&IpcRequest::on(Some("8h".into())))
                .unwrap()
                .contains("handoff_id"),
            "CLI On must keep omitting the internal-only field"
        );
        assert_eq!(format_handoff_id(11, Some(100), 1, None), "11-100-1");
        assert_eq!(
            parse_handoff_owner("11-100-1"),
            Some(HandoffOwner {
                pid: 11,
                starttime: 100,
                clamshell: None,
            })
        );
        assert_eq!(
            parse_handoff_owner("11-100-1-0"),
            Some(HandoffOwner {
                pid: 11,
                starttime: 100,
                clamshell: Some(false),
            })
        );
        assert_eq!(
            parse_handoff_owner("11-100-1-1"),
            Some(HandoffOwner {
                pid: 11,
                starttime: 100,
                clamshell: Some(true),
            })
        );
        assert!(parse_handoff_owner("h1").is_none());
        assert_eq!(
            format_handoff_id(11, Some(100), 1, Some(false)),
            "11-100-1-0"
        );
        assert!(
            should_stop_successor_on_cancel(true, true, true),
            "Ctrl-C during an in-flight handoff must stop a successor that already adopted"
        );
        assert!(!should_stop_successor_on_cancel(false, true, true));
        assert!(!should_stop_successor_on_cancel(true, false, true));
        assert!(!should_stop_successor_on_cancel(true, true, false));
        assert_ne!(
            format_handoff_id(11, Some(100), 1, None),
            format_handoff_id(11, Some(200), 1, None),
            "a reused PID with a new starttime must not collide with a prior handoff id"
        );
        assert!(
            donor_should_stop_after_successor_gone(Some("h1"), true, Some("h1")),
            "a matching ack must stop this donor even if a replacement menu already bound ipc.sock"
        );
        assert!(
            donor_should_stop_after_successor_gone(Some("h1"), false, Some("h1")),
            "menu adopted then Quit: the donor must stop from the persisted ack"
        );
        assert!(!donor_should_stop_after_successor_gone(
            Some("h1"),
            false,
            Some("h2")
        ));
        assert!(!donor_should_stop_after_successor_gone(
            None,
            false,
            Some("h1")
        ));
        assert!(
            donor_should_flush_offline_after_ack(false),
            "ack stop with no menu reporter must POST offline so the board does not stay live"
        );
        assert!(
            !donor_should_flush_offline_after_ack(true),
            "a live successor reporter must not be marked offline by this donor"
        );
        assert!(
            should_flush_offline_if_ack_reporter_clear_failed(false, true, true),
            "a detach Quit that cannot clear this session's reporter=1 must flush offline instead"
        );
        assert!(!should_flush_offline_if_ack_reporter_clear_failed(
            true, true, true
        ));
        assert!(!should_flush_offline_if_ack_reporter_clear_failed(
            false, false, true
        ));
        assert!(
            !should_flush_offline_if_ack_reporter_clear_failed(false, true, false),
            "a failed rewrite of an unmatched stale reporter=1 ack must not flush over a live donor"
        );
        assert!(
            handoff_ack_reporter(true),
            "a Stop ack must record reporter=1 while the menu reporter is still alive"
        );
        assert!(
            !handoff_ack_reporter(false),
            "Quit-cleared ownership must flush rather than detach into a missing reporter"
        );
        assert!(should_reject_adopt_if_ack_unpersisted(true, true, false));
        assert!(!should_reject_adopt_if_ack_unpersisted(true, false, false));
        assert!(!should_reject_adopt_if_ack_unpersisted(true, true, true));
        assert!(!should_reject_adopt_if_ack_unpersisted(false, true, false));
        assert!(
            should_reject_stop_if_ack_unpersisted(true, false),
            "a deferred Off must not rely on a one-shot IPC reply when Stop cannot be persisted"
        );
        assert!(!should_reject_stop_if_ack_unpersisted(true, true));
        assert!(!should_reject_stop_if_ack_unpersisted(false, false));
        assert!(should_skip_handoff_drain_after_ack_failure(true));
        assert!(!should_skip_handoff_drain_after_ack_failure(false));
        assert!(should_persist_handoff_ack(true, true, false));
        assert!(should_persist_handoff_ack(true, false, true));
        assert!(!should_persist_handoff_ack(true, false, false));
        assert!(!should_persist_handoff_ack(false, true, false));
        let ack = format_handoff_ack("11-100-1", HandoffAckOutcome::Adopted, true);
        assert_eq!(
            parse_handoff_ack(&ack),
            Some(HandoffAck {
                id: "11-100-1".into(),
                outcome: HandoffAckOutcome::Adopted,
                reporter: true,
            })
        );
        assert_eq!(
            parse_handoff_ack(&format_handoff_ack("h1", HandoffAckOutcome::Stop, false))
                .map(|ack| ack.outcome),
            Some(HandoffAckOutcome::Stop)
        );
        assert_eq!(
            parse_handoff_ack("handoff_id=h1\noutcome=adopted\n").map(|ack| ack.reporter),
            Some(false),
            "an older ack without reporter= must flush rather than assume a live successor reporter"
        );
        let _dir = crate::paths::TestDataDir::install();
        write_handoff_ack("h1", HandoffAckOutcome::Adopted, true).unwrap();
        assert_eq!(
            read_handoff_ack(),
            Some(HandoffAck {
                id: "h1".into(),
                outcome: HandoffAckOutcome::Adopted,
                reporter: true,
            })
        );
        assert!(mark_handoff_ack_reporter_gone());
        assert_eq!(
            read_handoff_ack().map(|ack| ack.reporter),
            Some(false),
            "Quit must clear reporter ownership before the socket goes away"
        );
        clear_handoff_ack();
        assert!(read_handoff_ack().is_none());
        std::fs::create_dir(crate::paths::handoff_ack_path()).unwrap();
        assert!(
            write_handoff_ack("h1", HandoffAckOutcome::Adopted, true).is_err(),
            "adopt must not report success when handoff.ack cannot be written"
        );
        let _ = std::fs::remove_dir(crate::paths::handoff_ack_path());
    }

    #[test]
    fn quit_during_adopt_reply_does_not_drop_offline_heartbeat() {
        assert!(
            should_flush_offline_after_abandoning_successor_reporter(true, true),
            "Quit that cleared reporter=1 must POST offline; the donor may still detach from a stale adopted reply"
        );
        assert!(
            !should_flush_offline_after_abandoning_successor_reporter(false, true),
            "Quit before adopt still detaches so the donor can resume"
        );
        assert!(!should_flush_offline_after_abandoning_successor_reporter(
            true, false
        ));
        assert!(
            successor_reporter_live_on_matching_ack(Some("h1"), Some("h1"), true),
            "Quit after adopt must see this session's reporter=1 ack"
        );
        assert!(
            !successor_reporter_live_on_matching_ack(Some("h2"), Some("h1"), true)
                && !successor_reporter_live_on_matching_ack(None, Some("h1"), true),
            "a stale reporter=1 ack from a crashed prior menu must not flush offline over a live donor"
        );
        assert!(
            !successor_reporter_after_fresh_ack(true, Some(true), Some(false)),
            "a later matching ack with reporter=0 must override the first live snapshot before detach"
        );
        assert!(successor_reporter_after_fresh_ack(
            true,
            Some(true),
            Some(true)
        ));
        assert!(
            !successor_reporter_after_fresh_ack(false, Some(true), Some(true)),
            "do not undo an already-decided flush"
        );

        let accept = include_str!("foreground.rs")
            .split("if crate::protocol::menu_accepted_handoff(&resp) {")
            .nth(1)
            .expect("accepted handoff")
            .split("if crate::protocol::donor_should_stop(&resp)")
            .next()
            .unwrap();
        let first = accept
            .find("successor_reporter_after_adopt")
            .expect("first snapshot");
        let fresh = accept
            .find("successor_reporter_after_fresh_ack")
            .expect("reconcile before detach");
        assert!(
            first < fresh,
            "re-read the matching ack after the adopted reply in case Quit already rewrote reporter=0"
        );
        assert!(
            accept.matches("read_handoff_ack").count() >= 2,
            "the donor must not detach from a single ack snapshot"
        );

        let flush = include_str!("gui.rs")
            .split("fn flush_cloud_on_quit")
            .nth(1)
            .expect("flush_cloud_on_quit")
            .split("fn handle_menu_event")
            .next()
            .unwrap();
        assert!(
            flush.contains("should_flush_offline_after_abandoning_successor_reporter")
                && flush.contains("had_live_reporter")
                && flush.contains("successor_reporter_live_on_matching_ack")
                && flush.contains("last_handoff_id"),
            "menu must flush after abandoning this session's reporter=1, not a stale prior ack"
        );
        let clear_failed = flush
            .split("should_flush_offline_if_ack_reporter_clear_failed")
            .nth(1)
            .expect("clear-failed flush gate")
            .split(')')
            .next()
            .unwrap();
        assert!(
            clear_failed.contains("had_live_reporter"),
            "a failed rewrite of an unmatched stale reporter=1 ack must not POST offline over a live donor"
        );
    }

    #[test]
    fn interrupted_reporter_gone_keeps_previous_ack() {
        let src = include_str!("protocol.rs");
        let write_fn = src
            .split("fn create_private_handoff_ack_tmp")
            .nth(1)
            .expect("create_private_handoff_ack_tmp")
            .split("pub fn mark_handoff_ack_reporter_gone")
            .next()
            .unwrap();
        assert!(
            write_fn.contains("handoff.ack.tmp")
                && write_fn.contains("sync_all")
                && write_fn.contains("rename")
                && write_fn.contains("create_new")
                && !write_fn.contains("fs::write"),
            "reporter-gone must replace handoff.ack atomically so a truncated write cannot drop the stop signal"
        );
        let mark_fn = src
            .split("pub fn mark_handoff_ack_reporter_gone")
            .nth(1)
            .expect("mark_handoff_ack_reporter_gone")
            .split("pub fn clear_handoff_ack")
            .next()
            .unwrap();
        assert!(
            mark_fn.contains("write_handoff_ack"),
            "Quit reporter-gone must go through the atomic ack write"
        );

        let _dir = crate::paths::TestDataDir::install();
        write_handoff_ack("h1", HandoffAckOutcome::Adopted, true).unwrap();
        crate::paths::ensure_data_dir().unwrap();
        let dir = crate::paths::data_dir();
        let orig = std::fs::metadata(&dir).unwrap().permissions();
        let mut perms = orig.clone();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o555);
        std::fs::set_permissions(&dir, perms).unwrap();
        let clear_ok = mark_handoff_ack_reporter_gone();
        std::fs::set_permissions(&dir, orig).unwrap();
        assert!(
            !clear_ok,
            "a read-only data dir must surface reporter clear failure"
        );
        assert_eq!(
            read_handoff_ack(),
            Some(HandoffAck {
                id: "h1".into(),
                outcome: HandoffAckOutcome::Adopted,
                reporter: true,
            }),
            "the previous valid ack must survive an interrupted reporter-bit update"
        );
    }

    #[test]
    fn response_helpers_roundtrip() {
        let ok = IpcResponse::ok_status(sample_status());
        assert!(ok.ok);
        assert_eq!(ok.status.as_ref().unwrap().display, "asleep");
        let err = IpcResponse::err("boom");
        assert!(!err.ok);
        assert_eq!(err.error.as_deref(), Some("boom"));
        let pong = IpcResponse::pong();
        assert_eq!(pong.pong, Some(true));
        let pair = IpcResponse::ok_pairing(
            "AB7K-2Q9M".into(),
            "https://xyz-ai.app/never-sleep/board/?code=AB7K-2Q9M".into(),
            Some("ab".repeat(16)),
        );
        let v = serde_json::to_value(&pair).unwrap();
        assert_eq!(v["pairing_code"], "AB7K-2Q9M");
        assert_eq!(
            v["pairing_url"],
            "https://xyz-ai.app/never-sleep/board/?code=AB7K-2Q9M"
        );
        assert_eq!(v["device_id"], "ab".repeat(16));
        assert!(v.get("status").is_none());
        let status_only = IpcResponse::ok_status(sample_status());
        let status_json = serde_json::to_value(&status_only).unwrap();
        assert!(status_json.get("pairing_code").is_none());
        assert!(status_json.get("pairing_url").is_none());
        assert!(status_json.get("device_id").is_none());
        let json_err = IpcResponse::err("pairing_unavailable");
        assert_eq!(
            serde_json::to_value(&json_err).unwrap()["error"],
            "pairing_unavailable"
        );
        assert_ne!(
            human_ipc_error("pairing_unavailable", Lang::Zh),
            "pairing_unavailable"
        );
        assert_eq!(
            human_ipc_error("pairing_unavailable", Lang::En),
            never_sleep_core::Tr::new(Lang::En).pairing_unavailable()
        );
    }
}
