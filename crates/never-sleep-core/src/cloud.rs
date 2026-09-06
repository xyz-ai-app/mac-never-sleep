//! Phone-board / cloud heartbeat contract.
//!
//! Local `never-sleep status --json` field names stay untouched. The cloud
//! payload *adds* `device_id`, `display_name`, `online`, and `last_seen_unix`.
//! Pairing uses a random device token; sequential ids are not used.

use serde::{Deserialize, Serialize};

use crate::status::JsonStatus;
use crate::{parse_duration_pref, Engine, HostSnapshot, Input, StopReason};

/// Public origin after the gateway prefix. Phone pages and Mac heartbeats share it.
pub const PUBLIC_SITE_ORIGIN: &str = "https://xyz-ai.app/never-sleep";
/// A Mac is online only if a heartbeat landed within this many seconds.
pub const HEARTBEAT_TTL_SECS: u64 = 35;
pub const PAIRING_TTL_SECS: u64 = 10 * 60;
pub const PAIRING_CODE_LEN: usize = 8;
pub const DEVICE_ID_HEX_LEN: usize = 32;
pub const DEVICE_TOKEN_HEX_LEN: usize = 64;

/// Crockford base32 without I, L, O, U so a pairing code is easy to read aloud.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudIdentity {
    pub device_id: String,
    pub device_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudDeviceStatus {
    pub device_id: String,
    pub display_name: String,
    pub online: bool,
    pub last_seen_unix: Option<i64>,
    #[serde(flatten)]
    pub status: JsonStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCommand {
    pub id: String,
    pub cmd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
}

impl RemoteCommand {
    pub fn on(id: impl Into<String>, duration: Option<String>) -> Self {
        Self {
            id: id.into(),
            cmd: "on".into(),
            duration,
        }
    }

    pub fn off(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            cmd: "off".into(),
            duration: None,
        }
    }

    pub fn is_allowed_cmd(cmd: &str) -> bool {
        matches!(cmd, "on" | "off" | "sleep_display")
    }

    /// Map a phone command onto Engine input. Unknown cmds are rejected.
    pub fn to_input(&self) -> Result<Input, String> {
        match self.cmd.as_str() {
            "on" => match self.duration.as_deref() {
                None => Ok(Input::StartRemote),
                Some(raw) => parse_duration_pref(raw).map(Input::StartRemoteWith),
            },
            "sleep_display" => Ok(Input::SleepDisplayNow),
            "off" => Ok(Input::Stop {
                reason: StopReason::User,
            }),
            other => Err(format!("unsupported cloud cmd {other}")),
        }
    }
}

pub fn device_is_online(last_seen_unix: i64, now_unix: i64) -> bool {
    if now_unix < last_seen_unix {
        return true;
    }
    (now_unix - last_seen_unix) as u64 <= HEARTBEAT_TTL_SECS
}

pub fn hex_from_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn identity_from_bytes(id_bytes: &[u8; 16], token_bytes: &[u8; 32]) -> CloudIdentity {
    CloudIdentity {
        device_id: hex_from_bytes(id_bytes),
        device_token: hex_from_bytes(token_bytes),
    }
}

/// Same 32/64-hex contract the Worker uses before `idFromName`.
pub fn device_credentials_are_valid(device_id: &str, device_token: &str) -> bool {
    is_hex_len(device_id, DEVICE_ID_HEX_LEN) && is_hex_len(device_token, DEVICE_TOKEN_HEX_LEN)
}

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn pairing_code_from_bytes(bytes: &[u8; 5]) -> String {
    let mut bits: u64 = 0;
    for b in bytes {
        bits = (bits << 8) | u64::from(*b);
    }
    let mut chars = [0u8; PAIRING_CODE_LEN];
    for slot in chars.iter_mut().rev() {
        *slot = CROCKFORD[(bits & 31) as usize];
        bits >>= 5;
    }
    String::from_utf8(chars.to_vec()).expect("crockford is ascii")
}

pub fn normalize_pairing_code(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(PAIRING_CODE_LEN);
    for ch in raw.chars() {
        if ch == '-' || ch.is_whitespace() {
            continue;
        }
        let mapped = match ch.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            c @ ('0'..='9' | 'A'..='Z') => c,
            _ => return None,
        };
        if !CROCKFORD.contains(&(mapped as u8)) {
            return None;
        }
        out.push(mapped);
        if out.len() > PAIRING_CODE_LEN {
            return None;
        }
    }
    if out.len() == PAIRING_CODE_LEN {
        Some(out)
    } else {
        None
    }
}

pub fn format_pairing_code(code: &str) -> String {
    if code.len() == PAIRING_CODE_LEN {
        format!("{}-{}", &code[..4], &code[4..])
    } else {
        code.to_string()
    }
}

pub fn pairing_url(code: &str, chinese: bool) -> String {
    let path = if chinese { "/zh/board/" } else { "/board/" };
    format!(
        "{PUBLIC_SITE_ORIGIN}{path}?code={}",
        format_pairing_code(code)
    )
}

/// Constant-time compare so a token is not leaked by response timing.
pub fn tokens_match(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut acc = 0u8;
    for (a, b) in left.as_bytes().iter().zip(right.as_bytes().iter()) {
        acc |= a ^ b;
    }
    acc == 0
}

pub fn apply_remote_command(
    engine: &mut Engine,
    host: &HostSnapshot,
    cmd: &RemoteCommand,
) -> Result<Vec<crate::Effect>, String> {
    let input = cmd.to_input()?;
    let effects = match input {
        Input::StartRemote if engine.is_active() => Vec::new(),
        Input::StartRemoteWith(_) if engine.is_active() => {
            let mut effects = engine.handle(
                Input::Stop {
                    reason: StopReason::User,
                },
                host,
            );
            effects.extend(engine.handle(input, host));
            effects
        }
        other => engine.handle(other, host),
    };
    Ok(effects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::JsonStatus;
    use crate::{AppConfig, Effect, HostSnapshot, Thermal};

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
            stop_reason: Some("Ended by you".into()),
            stop_reason_code: Some("user".into()),
            screen_off_enabled: true,
            lid_awake_enabled: true,
        }
    }

    fn host(ms: u64) -> HostSnapshot {
        HostSnapshot {
            monotonic_ms: ms,
            continuous_ms: ms,
            unix_secs: 1_700_000_000,
            utc_offset_secs: 0,
            on_ac: true,
            battery_percent: Some(80),
            lid_closed: false,
            display_asleep: Some(false),
            hid_idle_ms: 1_000,
            thermal: Thermal::Nominal,
        }
    }

    fn has_sleep(effects: &[Effect]) -> bool {
        effects.iter().any(|e| matches!(e, Effect::SleepDisplay))
    }

    #[test]
    fn cloud_device_status_keeps_json_status_field_names() {
        let cloud = CloudDeviceStatus {
            device_id: "ab".repeat(16),
            display_name: "Studio".into(),
            online: true,
            last_seen_unix: Some(1_700_000_000),
            status: sample_status(),
        };
        let v = serde_json::to_value(&cloud).unwrap();
        for key in [
            "device_id",
            "display_name",
            "online",
            "last_seen_unix",
            "active",
            "display",
            "lid",
            "on_ac",
            "battery",
            "remaining_secs",
            "user_present",
            "elapsed_secs",
            "stop_reason",
            "stop_reason_code",
            "screen_off_enabled",
            "lid_awake_enabled",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert!(v.get("status").is_none(), "JsonStatus must be flattened");
        let back: CloudDeviceStatus = serde_json::from_value(v).unwrap();
        assert_eq!(back.device_id, cloud.device_id);
        assert!(back.status.active);
    }

    #[test]
    fn identity_from_bytes_is_not_sequential() {
        let a = identity_from_bytes(&[1; 16], &[2; 32]);
        let b = identity_from_bytes(&[3; 16], &[4; 32]);
        assert_eq!(a.device_id.len(), DEVICE_ID_HEX_LEN);
        assert_eq!(a.device_token.len(), DEVICE_TOKEN_HEX_LEN);
        assert_ne!(a.device_id, b.device_id);
        assert_ne!(a.device_token, b.device_token);
        assert_ne!(a.device_id, "1");
        assert_ne!(a.device_id, "device-1");
    }

    #[test]
    fn device_credentials_match_the_worker_hex_contract() {
        assert!(device_credentials_are_valid(
            &"ab".repeat(16),
            &"cd".repeat(32)
        ));
        assert!(device_credentials_are_valid(
            &"AB".repeat(16),
            &"CD".repeat(32)
        ));
        for (id, token) in [
            ("aa".repeat(8), "cd".repeat(32)),
            ("ab".repeat(16), "cd".repeat(8)),
            ("ab".repeat(16), "not-hex-but-long-enough-xxxx".into()),
            ("gg".repeat(16), "cd".repeat(32)),
            ("f".repeat(64), "f".repeat(128)),
        ] {
            assert!(
                !device_credentials_are_valid(&id, &token),
                "id={id:?} token={token:?} must not register"
            );
        }
    }

    #[test]
    fn pairing_code_is_short_crockford_not_a_counter() {
        let code = pairing_code_from_bytes(&[0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(code.len(), PAIRING_CODE_LEN);
        assert!(code.chars().all(|c| CROCKFORD.contains(&(c as u8))));
        assert_ne!(code, "00000001");
        assert_eq!(
            normalize_pairing_code(&format_pairing_code(&code)).as_deref(),
            Some(code.as_str())
        );
        assert_eq!(
            normalize_pairing_code("ab7k-2q9m").as_deref(),
            Some("AB7K2Q9M")
        );
        assert!(normalize_pairing_code("short").is_none());
        assert!(normalize_pairing_code("!!!!!!!!").is_none());
    }

    #[test]
    fn pairing_url_uses_public_origin_and_board_path() {
        let url = pairing_url("AB7K2Q9M", false);
        assert!(url.starts_with(PUBLIC_SITE_ORIGIN));
        assert!(url.contains("/board/"));
        assert!(url.contains("code=AB7K-2Q9M"));
        let zh = pairing_url("AB7K2Q9M", true);
        assert!(zh.contains("/zh/board/"));
    }

    #[test]
    fn heartbeat_ttl_marks_stale_devices_offline() {
        assert!(device_is_online(100, 100));
        assert!(device_is_online(100, 115));
        assert!(device_is_online(100, 130));
        assert!(device_is_online(100, 135));
        assert!(!device_is_online(100, 136));
        assert!(!device_is_online(100, 200));
    }

    #[test]
    fn tokens_match_rejects_wrong_secret() {
        let token = "a".repeat(DEVICE_TOKEN_HEX_LEN);
        assert!(tokens_match(&token, &token));
        assert!(!tokens_match(&token, &"b".repeat(DEVICE_TOKEN_HEX_LEN)));
        assert!(!tokens_match(&token, "short"));
    }

    #[test]
    fn remote_command_serde_and_allowed_cmds() {
        let on = RemoteCommand::on("c1", Some("8h".into()));
        let v = serde_json::to_value(&on).unwrap();
        assert_eq!(v["cmd"], "on");
        assert_eq!(v["duration"], "8h");
        assert!(RemoteCommand::is_allowed_cmd("on"));
        assert!(RemoteCommand::is_allowed_cmd("off"));
        assert!(!RemoteCommand::is_allowed_cmd("toggle"));
        assert!(!RemoteCommand::is_allowed_cmd("quit"));
        assert!(RemoteCommand::on("x", None).to_input().is_ok());
        assert!(RemoteCommand {
            id: "x".into(),
            cmd: "toggle".into(),
            duration: None,
        }
        .to_input()
        .is_err());
    }

    #[test]
    fn explicit_remote_sleep_darkens_immediately_without_changing_standby() {
        let command: RemoteCommand =
            serde_json::from_str(r#"{"id":"sleep-1","cmd":"sleep_display"}"#).unwrap();
        assert!(RemoteCommand::is_allowed_cmd(&command.cmd));
        for active in [false, true] {
            let mut eng = Engine::new(AppConfig::default());
            let h = host(0);
            if active {
                eng.handle(Input::StartRemote, &h);
            }
            let effects = apply_remote_command(&mut eng, &h, &command).unwrap();
            assert!(has_sleep(&effects));
            assert_eq!(eng.is_active(), active);
            assert!(!effects
                .iter()
                .any(|e| matches!(e, Effect::ApplyPower(_) | Effect::ReleasePower)));
        }
    }

    #[test]
    fn authorized_on_off_changes_engine_state() {
        let mut eng = Engine::new(AppConfig::default());
        let h = host(0);
        apply_remote_command(&mut eng, &h, &RemoteCommand::on("1", None)).unwrap();
        assert!(eng.is_active());
        apply_remote_command(&mut eng, &host(200), &RemoteCommand::off("2")).unwrap();
        assert!(!eng.is_active());
        let st = eng.json_status(&host(200));
        assert!(!st.active);
        assert_eq!(st.stop_reason_code.as_deref(), Some("user"));
    }

    #[test]
    fn remote_on_sleeps_after_the_same_delay_as_app_start() {
        let mut eng = Engine::new(AppConfig::default());
        let mut h = host(0);
        h.hid_idle_ms = 500;
        h.lid_closed = false;
        let effects = apply_remote_command(&mut eng, &h, &RemoteCommand::on("1", None)).unwrap();
        assert!(eng.is_active());
        assert!(!has_sleep(&effects));
        h.monotonic_ms = 1_500;
        h.continuous_ms = 1_500;
        let later = eng.handle(Input::Tick, &h);
        assert!(
            has_sleep(&later),
            "explicit phone start must sleep the display like app start"
        );
        h.hid_idle_ms = 80_000;
        h.monotonic_ms = 50_000;
        h.continuous_ms = 50_000;
        let away = eng.handle(Input::Tick, &h);
        assert!(has_sleep(&away), "display sleeps after the user leaves");
    }

    #[test]
    fn json_status_agent_fields_unchanged_when_wrapped() {
        let st = sample_status();
        let direct = serde_json::to_value(&st).unwrap();
        let wrapped = serde_json::to_value(&CloudDeviceStatus {
            device_id: "d".repeat(32),
            display_name: "Mac".into(),
            online: false,
            last_seen_unix: None,
            status: st,
        })
        .unwrap();
        for key in [
            "active",
            "display",
            "lid",
            "on_ac",
            "battery",
            "remaining_secs",
            "user_present",
            "elapsed_secs",
            "stop_reason",
            "stop_reason_code",
            "screen_off_enabled",
            "lid_awake_enabled",
        ] {
            assert_eq!(direct[key], wrapped[key], "{key} must stay the Agent name");
        }
    }
}
