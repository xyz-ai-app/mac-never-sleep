use serde::{Deserialize, Serialize};

use crate::i18n::{Lang, Tr};
use crate::{DEFAULT_BATTERY_FLOOR, DEFAULT_DISPLAY_OFF_DELAY_MS, DEFAULT_USER_IDLE_RESLEEP_MS};

/// 用户偏好。全部有安全默认值，首次启动即可「一键关屏待命」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    /// 默认时长（菜单里改，下次启动沿用）
    pub duration: DurationPref,
    /// 进入待命后立刻关屏（本产品的主功能）
    pub screen_off: bool,
    /// 合盖时尽量不让系统睡眠（尽力而为；开盖熄屏才是可靠主路径）
    pub keep_awake_on_lid_close: bool,
    /// 屏幕被远程/系统唤醒后，若判定人已离开则再关一次
    pub resleep_display: bool,
    /// 电量低于该百分比则结束待命，让系统可以睡眠。`None` 表示不限制。
    pub battery_floor_percent: Option<u8>,
    /// 登录时自动打开菜单栏
    pub launch_at_login: bool,
    /// Remote access is explicitly opt-in, including configurations from older versions.
    #[serde(default)]
    pub remote_enabled: bool,
    /// 关屏同时锁屏幕。默认关：GUI 远程操控（ChatGPT/Codex）需要解锁会话。
    pub lock_screen: bool,
    /// 首次点击「开始」后延迟关屏，好让通知/菜单能被看见
    pub display_off_delay_ms: u64,
    /// HID 空闲超过该时间才视为「人已离开」，避免跟正在用电脑的人抢屏幕
    pub user_idle_resleep_ms: u64,
    /// 已看过使用说明
    pub onboarding_done: bool,
    /// Saved UI language. `None` means “not chosen yet” (legacy configs, first run).
    /// Process overrides (`--lang` / `NEVER_SLEEP_LANG`) are applied in `lang()` and are not stored here.
    #[serde(default)]
    pub language: Option<Lang>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            duration: DurationPref::Indefinite,
            screen_off: true,
            keep_awake_on_lid_close: true,
            resleep_display: true,
            battery_floor_percent: Some(DEFAULT_BATTERY_FLOOR),
            launch_at_login: false,
            remote_enabled: false,
            lock_screen: false,
            display_off_delay_ms: DEFAULT_DISPLAY_OFF_DELAY_MS,
            user_idle_resleep_ms: DEFAULT_USER_IDLE_RESLEEP_MS,
            onboarding_done: false,
            language: Some(Lang::En),
        }
    }
}

impl AppConfig {
    pub fn lang(&self) -> Lang {
        Lang::from_override_env()
            .or(self.language)
            .unwrap_or(Lang::En)
    }

    pub fn tr(&self) -> Tr {
        Tr::new(self.lang())
    }

    pub fn battery_floor_label(&self) -> String {
        let t = self.tr();
        match self.battery_floor_percent {
            Some(n) => t.battery_floor_on(n),
            None => t.battery_floor_off().into(),
        }
    }
}

/// 待命持续多久。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DurationPref {
    Indefinite,
    /// 从开始时刻起算
    Hours {
        hours: u32,
    },
    /// 本地时钟的下一个 hour:minute（已过则明天）
    UntilLocal {
        hour: u8,
        minute: u8,
    },
}

impl DurationPref {
    pub fn label(self, lang: Lang) -> String {
        Tr::new(lang).duration_pref(self)
    }
}

impl Default for DurationPref {
    fn default() -> Self {
        Self::Indefinite
    }
}

/// Parse a CLI duration: `indefinite` / `3h` / `until=08:00`. Errors are English.
pub fn parse_duration_pref(raw: &str) -> Result<DurationPref, String> {
    parse_duration_pref_in(raw, Lang::En)
}

pub fn parse_duration_pref_in(raw: &str, lang: Lang) -> Result<DurationPref, String> {
    let t = Tr::new(lang);
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() || s == "indefinite" || s == "inf" || s == "forever" || s == "无限" {
        return Ok(DurationPref::Indefinite);
    }
    if let Some(rest) = s.strip_prefix("until=") {
        return parse_until(rest, t);
    }
    if let Some(rest) = s.strip_prefix("until:") {
        return parse_until(rest, t);
    }
    if let Some(num) = s.strip_suffix('h') {
        let hours: u32 = num.parse().map_err(|_| t.parse_duration_error(raw))?;
        if hours == 0 {
            return Err(t.parse_duration_min_hour().into());
        }
        return Ok(DurationPref::Hours { hours });
    }
    if let Some(num) = s.strip_suffix("小时") {
        let hours: u32 = num
            .trim()
            .parse()
            .map_err(|_| t.parse_duration_error(raw))?;
        if hours == 0 {
            return Err(t.parse_duration_min_hour().into());
        }
        return Ok(DurationPref::Hours { hours });
    }
    parse_until(&s, t)
}

fn parse_until(raw: &str, t: Tr) -> Result<DurationPref, String> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 2 {
        return Err(t.parse_time_format(raw));
    }
    let hour: u8 = parts[0].parse().map_err(|_| t.parse_invalid_hour(raw))?;
    let minute: u8 = parts[1].parse().map_err(|_| t.parse_invalid_minute(raw))?;
    if hour > 23 || minute > 59 {
        return Err(t.parse_invalid_time(raw));
    }
    Ok(DurationPref::UntilLocal { hour, minute })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_access_requires_explicit_persisted_opt_in() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        assert_eq!(value["remote_enabled"], false);
        value.as_object_mut().unwrap().remove("remote_enabled");
        let legacy: AppConfig = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(legacy).unwrap()["remote_enabled"],
            false
        );
        value["remote_enabled"] = true.into();
        let enabled: AppConfig = serde_json::from_value(value).unwrap();
        assert_eq!(
            serde_json::to_value(enabled).unwrap()["remote_enabled"],
            true
        );
    }

    #[test]
    fn parse_duration_variants() {
        assert_eq!(
            parse_duration_pref("indefinite").unwrap(),
            DurationPref::Indefinite
        );
        assert_eq!(
            parse_duration_pref("3h").unwrap(),
            DurationPref::Hours { hours: 3 }
        );
        assert_eq!(
            parse_duration_pref("until=08:00").unwrap(),
            DurationPref::UntilLocal { hour: 8, minute: 0 }
        );
        assert_eq!(
            parse_duration_pref("22:30").unwrap(),
            DurationPref::UntilLocal {
                hour: 22,
                minute: 30
            }
        );
        assert_eq!(
            parse_duration_pref("forever").unwrap(),
            DurationPref::Indefinite
        );
        assert_eq!(
            parse_duration_pref("inf").unwrap(),
            DurationPref::Indefinite
        );
        assert_eq!(
            parse_duration_pref("无限").unwrap(),
            DurationPref::Indefinite
        );
        assert_eq!(
            parse_duration_pref("3小时").unwrap(),
            DurationPref::Hours { hours: 3 }
        );
        assert_eq!(
            parse_duration_pref("until:08:00").unwrap(),
            DurationPref::UntilLocal { hour: 8, minute: 0 }
        );
        assert_eq!(
            parse_duration_pref(" 8H ").unwrap(),
            DurationPref::Hours { hours: 8 }
        );
        assert_eq!(
            parse_duration_pref("240h").unwrap(),
            DurationPref::Hours { hours: 240 }
        );
        assert_eq!(
            parse_duration_pref(&format!("{h}h", h = u32::MAX)).unwrap(),
            DurationPref::Hours { hours: u32::MAX }
        );
        assert!(parse_duration_pref(&format!("{h}h", h = u64::from(u32::MAX) + 1)).is_err());
    }

    #[test]
    fn parse_duration_rejects_zero_hours() {
        let en = parse_duration_pref("0h").unwrap_err();
        assert!(en.contains("1 hour"), "{en}");
        let zh = parse_duration_pref_in("0小时", Lang::Zh).unwrap_err();
        assert!(zh.contains("1 小时") || zh.contains("indefinite"), "{zh}");
    }

    #[test]
    fn parse_duration_rejects_invalid_clock() {
        assert!(parse_duration_pref("24:00").is_err());
        assert!(parse_duration_pref("12:60").is_err());
        assert!(parse_duration_pref("until=ab:cd").is_err());
    }

    #[test]
    fn duration_pref_labels_follow_language() {
        assert_eq!(DurationPref::Indefinite.label(Lang::En), "Indefinite");
        assert_eq!(DurationPref::Hours { hours: 1 }.label(Lang::En), "1 hour");
        assert_eq!(DurationPref::Hours { hours: 3 }.label(Lang::En), "3 hours");
        assert_eq!(
            DurationPref::UntilLocal { hour: 8, minute: 0 }.label(Lang::Zh),
            "到 08:00"
        );
    }

    #[test]
    fn battery_floor_label_matches_config() {
        let cfg = AppConfig::default();
        assert!(cfg.battery_floor_label().contains("20%"));
        let mut off = cfg.clone();
        off.battery_floor_percent = None;
        assert_eq!(off.battery_floor_label(), off.tr().battery_floor_off());
    }

    #[test]
    fn default_config_is_screen_off_standby() {
        let cfg = AppConfig::default();
        assert!(cfg.screen_off);
        assert!(cfg.keep_awake_on_lid_close);
        assert!(cfg.resleep_display);
        assert_eq!(cfg.battery_floor_percent, Some(20));
        assert!(!cfg.lock_screen);
        assert_eq!(cfg.language, Some(Lang::En));
    }

    #[test]
    fn parse_duration_errors_follow_language() {
        let en = parse_duration_pref_in("nope", Lang::En).unwrap_err();
        assert!(en.contains("HH:MM"), "{en}");
        let zh = parse_duration_pref_in("nope", Lang::Zh).unwrap_err();
        assert!(zh.contains("HH:MM"), "{zh}");
        assert_ne!(en, zh);
    }

    #[test]
    fn missing_language_field_deserializes_as_none() {
        let value = serde_json::json!({
            "duration": { "kind": "indefinite" },
            "screen_off": true,
            "keep_awake_on_lid_close": true,
            "resleep_display": true,
            "battery_floor_percent": 20,
            "launch_at_login": false,
            "lock_screen": false,
            "display_off_delay_ms": 1500,
            "user_idle_resleep_ms": 45000,
            "onboarding_done": true
        });
        let cfg: AppConfig = serde_json::from_value(value).unwrap();
        assert_eq!(cfg.language, None);
        assert_eq!(cfg.lang(), Lang::from_override_env().unwrap_or(Lang::En));
    }
}
