//! English-first bilingual strings (en + zh-Hans).
//!
//! English is the default and the fallback. Chinese is used when the UI language
//! is `zh` (system Chinese, `--lang zh`, or `NEVER_SLEEP_LANG=zh`).

use crate::DurationPref;

pub const APP_NAME: &str = "Never Sleep";
pub const BUNDLE_ID: &str = "com.seasonxue.never-sleep";
pub const DEFAULT_HOTKEY_LABEL: &str = "⌥⌘P";
pub const LANG_ENV: &str = "NEVER_SLEEP_LANG";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    #[default]
    En,
    Zh,
}

impl Lang {
    pub fn parse_opt(raw: &str) -> Option<Self> {
        let s = raw.trim().to_ascii_lowercase().replace('_', "-");
        let primary = s.split(['-', '.', '@']).next().unwrap_or(&s);
        match primary {
            "zh" | "chi" | "chinese" | "cn" => Some(Self::Zh),
            "en" | "eng" | "english" => Some(Self::En),
            _ => None,
        }
    }

    /// `NEVER_SLEEP_LANG=en|zh` process override.
    pub fn from_override_env() -> Option<Self> {
        std::env::var(LANG_ENV)
            .ok()
            .as_deref()
            .and_then(Self::parse_opt)
    }

    /// Unix locale (`LANG` / `LC_*`). Unknown locales fall back to English.
    pub fn from_unix_locale() -> Self {
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(v) = std::env::var(key) {
                if let Some(lang) = Self::parse_opt(&v) {
                    return lang;
                }
                if !v.is_empty() {
                    return Self::En;
                }
            }
        }
        Self::En
    }

    /// First preferred language tag wins; non-Chinese tags resolve to English.
    pub fn from_preferred_tags<S: AsRef<str>>(tags: &[S]) -> Option<Self> {
        let first = tags.first()?.as_ref();
        Some(if Self::parse_opt(first) == Some(Self::Zh) {
            Self::Zh
        } else {
            Self::En
        })
    }

    pub fn override_or(self) -> Self {
        Self::from_override_env().unwrap_or(self)
    }

    pub fn is_chinese(self) -> bool {
        matches!(self, Self::Zh)
    }

    /// Stable cloud/heartbeat tag (`en` / `zh`). JSON stays English.
    pub fn cloud_tag(self) -> &'static str {
        if self.is_chinese() {
            "zh"
        } else {
            "en"
        }
    }
}

/// Translator for a concrete UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tr {
    pub lang: Lang,
}

impl Tr {
    pub fn new(lang: Lang) -> Self {
        Self { lang }
    }

    fn pick(self, en: &'static str, zh: &'static str) -> &'static str {
        match self.lang {
            Lang::En => en,
            Lang::Zh => zh,
        }
    }

    pub fn app_display_name(self) -> &'static str {
        self.pick(APP_NAME, APP_NAME)
    }

    pub fn panel_idle_title(self) -> &'static str {
        self.pick("Not Active", "未开启")
    }

    pub fn panel_active_title(self) -> &'static str {
        self.pick("Screen-Off Standby", "关屏待命中")
    }

    pub fn panel_summary_idle(self) -> &'static str {
        self.pick("Display off, Mac stays online", "屏幕将关闭，Mac 保持在线")
    }

    pub fn panel_summary_active(self) -> &'static str {
        self.pick("Display asleep, Mac stays online", "屏幕已休眠，Mac 仍在线")
    }

    pub fn more_settings(self) -> &'static str {
        self.pick("More Settings", "更多设置")
    }

    pub fn settings_title(self) -> &'static str {
        self.pick("Settings", "设置")
    }

    pub fn back(self) -> &'static str {
        self.pick("Back", "返回")
    }

    pub fn panel_section_session(self) -> &'static str {
        self.pick("Session", "待命")
    }

    pub fn panel_section_display(self) -> &'static str {
        self.pick("Display", "屏幕")
    }

    pub fn panel_section_lid(self) -> &'static str {
        self.pick("Lid", "合盖")
    }

    pub fn panel_section_safeguards(self) -> &'static str {
        self.pick("Safeguards", "保护")
    }

    pub fn panel_section_general(self) -> &'static str {
        self.pick("General", "通用")
    }

    pub fn sidebar_group_options(self) -> &'static str {
        self.pick("Options", "选项")
    }

    pub fn sidebar_group_guide(self) -> &'static str {
        self.pick("Guide", "指南")
    }

    pub fn pane_display_lead(self) -> &'static str {
        self.pick(
            "The display sleeps for real — not brightness 0.",
            "真·关闭屏幕，不是把亮度拉到 0。",
        )
    }

    pub fn pane_safeguards_lead(self) -> &'static str {
        self.pick(
            "Lock only if you need it. End standby before the pack dies.",
            "需要时再锁登录。电量过低时结束待命。",
        )
    }

    pub fn pane_general_lead(self) -> &'static str {
        self.pick("Applies to this Mac only.", "只影响这台 Mac。")
    }

    pub fn show_window(self) -> &'static str {
        self.pick("Show Window", "显示窗口")
    }

    pub fn panel_hotkey_hint(self) -> String {
        match self.lang {
            Lang::En => format!("{DEFAULT_HOTKEY_LABEL} works with the display off"),
            Lang::Zh => format!("屏幕关掉时也可按 {DEFAULT_HOTKEY_LABEL}"),
        }
    }

    pub fn onboarding(self) -> &'static str {
        self.pick(ONBOARDING_EN, ONBOARDING_ZH)
    }

    pub fn welcome_title(self) -> &'static str {
        self.pick("Welcome to Never Sleep", "欢迎使用 Never Sleep")
    }

    pub fn help_title(self) -> &'static str {
        self.pick("How to use", "使用说明")
    }

    pub fn help_kicker(self) -> &'static str {
        self.pick("Display off · Mac online", "关屏护屏 · 电脑在线")
    }

    pub fn help_lead(self) -> &'static str {
        self.pick(
            "Remote clients such as ChatGPT and Codex can keep connecting.",
            "ChatGPT / Codex 等远程客户端仍可连上这台电脑。",
        )
    }

    pub fn help_how(self) -> &'static str {
        self.pick("Get started", "怎么用")
    }

    pub fn help_step1_title(self) -> &'static str {
        self.pick("Start standby", "开始待命")
    }

    pub fn help_step1_detail(self) -> &'static str {
        self.pick(
            "Click “Start Screen-Off Standby”. The display sleeps after about 1.5 seconds.",
            "点「开始关屏待命」，约 1.5 秒后屏幕关闭。",
        )
    }

    pub fn help_step2_title(self) -> &'static str {
        self.pick("Stays out of your way", "不抢屏幕")
    }

    pub fn help_step2_detail(self) -> &'static str {
        self.pick(
            "It will not fight you for the screen while you type; it sleeps again after you leave.",
            "人在电脑前绝不强制关屏；走开后再自动关闭。",
        )
    }

    pub fn help_step3_title(self) -> &'static str {
        self.pick("Come back any time", "随时回来")
    }

    pub fn help_step3_before(self) -> &'static str {
        self.pick("Press", "按")
    }

    pub fn help_step3_after(self) -> &'static str {
        self.pick(
            "or choose “End Standby” in the menu.",
            "，或点菜单「结束待命」。",
        )
    }

    pub fn help_notes(self) -> &'static str {
        self.pick("Keep in mind", "请留意")
    }

    pub fn help_note_lid(self) -> &'static str {
        self.pick(
            "Closed-lid stay-awake is best-effort, more reliable on power. Lid open + display asleep is the reliable path.",
            "合盖保活是尽力而为，插电更稳；最可靠仍是开盖熄屏。",
        )
    }

    pub fn help_note_battery(self) -> &'static str {
        self.pick(
            "Standby ends automatically on low battery so the pack is not drained.",
            "电量过低会自动结束，避免把电池耗干。",
        )
    }

    pub fn help_note_quit(self) -> &'static str {
        self.pick(
            "Quitting restores normal sleep. Energy Saver settings are never rewritten.",
            "退出后立即恢复系统睡眠，不会改写节能设置。",
        )
    }

    pub fn dialog_ok(self) -> &'static str {
        self.pick("OK", "好")
    }

    pub fn idle_status(self) -> &'static str {
        self.pick("Idle · click to start", "未待命 · 点击开始")
    }

    pub fn standby_status(self) -> &'static str {
        self.pick("Standby", "待命中")
    }

    pub fn standby_elapsed(self, duration: &str) -> String {
        match self.lang {
            Lang::En => format!("Standby · {duration} elapsed"),
            Lang::Zh => format!("待命中 · 已 {duration}"),
        }
    }

    pub fn will_sleep_display(self) -> &'static str {
        self.pick(
            "Will sleep the display and keep the Mac running",
            "将关闭屏幕、保持系统运行",
        )
    }

    pub fn will_keep_awake_only(self) -> &'static str {
        self.pick(
            "Will keep the Mac running (display stays under system control)",
            "将保持系统运行（不强制关屏）",
        )
    }

    pub fn start_standby(self) -> &'static str {
        self.pick("Start Screen-Off Standby", "开始关屏待命")
    }

    pub fn end_standby(self) -> &'static str {
        self.pick("End Standby", "结束待命")
    }

    pub fn duration_menu(self) -> &'static str {
        self.pick("Duration", "时长")
    }

    pub fn indefinite(self) -> &'static str {
        self.pick("Indefinite", "无限期")
    }

    pub fn hours(self, hours: u32) -> String {
        match self.lang {
            Lang::En if hours == 1 => "1 hour".into(),
            Lang::En => format!("{hours} hours"),
            Lang::Zh => format!("{hours} 小时"),
        }
    }

    pub fn until_clock(self, hour: u8, minute: u8) -> String {
        match self.lang {
            Lang::En => format!("Until {hour:02}:{minute:02}"),
            Lang::Zh => format!("到 {hour:02}:{minute:02}"),
        }
    }

    pub fn duration_pref(self, pref: DurationPref) -> String {
        match pref {
            DurationPref::Indefinite => self.indefinite().into(),
            DurationPref::Hours { hours } => self.hours(hours),
            DurationPref::UntilLocal { hour, minute } => self.until_clock(hour, minute),
        }
    }

    pub fn screen_off_now(self) -> &'static str {
        self.pick("Sleep display immediately", "立即关闭屏幕")
    }

    pub fn sleep_display_now_action(self) -> &'static str {
        self.pick("Sleep Display Now", "立即熄屏")
    }

    pub fn lid_awake(self) -> &'static str {
        self.pick(
            "Keep running when the lid is closed (best effort)",
            "合盖尽量保持运行",
        )
    }

    pub fn resleep_display(self) -> &'static str {
        self.pick("Re-sleep the display after you leave", "人离开后自动再关屏")
    }

    pub fn lock_screen(self) -> &'static str {
        self.pick(
            "Lock the session when the display sleeps (breaks remote GUI)",
            "关屏时锁定登录（远程 GUI 会受影响）",
        )
    }

    pub fn battery_floor_on(self, percent: u8) -> String {
        match self.lang {
            Lang::En => format!("End when battery is below {percent}%"),
            Lang::Zh => format!("电量低于 {percent}% 时结束"),
        }
    }

    pub fn battery_floor_off(self) -> &'static str {
        self.pick(
            "Do not end automatically on low battery",
            "电量过低时不自动结束",
        )
    }

    pub fn launch_at_login(self) -> &'static str {
        self.pick("Launch at login", "登录时启动")
    }

    pub fn phone_board(self) -> &'static str {
        self.pick("Phone board", "手机看板")
    }

    pub fn pairing_code(self) -> &'static str {
        self.pick("Pairing code", "配对码")
    }

    pub fn remote_access(self) -> &'static str {
        self.pick("Remote access", "远程访问")
    }

    pub fn remote_disabled(self) -> &'static str {
        self.pick(
            "Remote access is off. Enable it in Settings to pair your phone.",
            "远程访问已关闭。请在设置中开启后配对手机。",
        )
    }

    pub fn pairing_unavailable(self) -> &'static str {
        self.pick(
            "Pairing is not ready yet. Open Settings, or retry in a moment.",
            "配对尚未就绪。请打开设置，或稍后再试。",
        )
    }

    pub fn language_menu(self) -> &'static str {
        self.pick("Language", "语言")
    }

    pub fn language_english(self) -> &'static str {
        "English"
    }

    pub fn language_chinese(self) -> &'static str {
        "简体中文"
    }

    pub fn quit(self) -> &'static str {
        self.pick("Quit", "退出")
    }

    pub fn display_asleep(self) -> &'static str {
        self.pick("Display asleep", "屏幕已关")
    }

    pub fn user_controls_display(self) -> &'static str {
        self.pick(
            "You are using it · display is yours",
            "你正在用，屏幕由你控制",
        )
    }

    pub fn display_pending(self) -> &'static str {
        self.pick("Display will sleep", "屏幕待关")
    }

    pub fn lid_closed(self) -> &'static str {
        self.pick("Lid closed", "合盖")
    }

    pub fn lid_open(self) -> &'static str {
        self.pick("Lid open", "开盖")
    }

    pub fn power_ac(self) -> &'static str {
        self.pick("Power adapter", "电源适配器")
    }

    pub fn power_battery(self) -> &'static str {
        self.pick("Battery", "电池")
    }

    pub fn battery_percent(self, percent: u8) -> String {
        match self.lang {
            Lang::En => format!("Battery {percent}%"),
            Lang::Zh => format!("电量 {percent}%"),
        }
    }

    pub fn remaining(self, duration: &str) -> String {
        match self.lang {
            Lang::En => format!("{duration} left"),
            Lang::Zh => format!("剩余 {duration}"),
        }
    }

    pub fn tooltip_active(self) -> String {
        format!(
            "{} · {}",
            self.app_display_name(),
            self.pick("running", "运行中")
        )
    }

    pub fn tooltip_idle(self) -> String {
        format!(
            "{} · {}",
            self.app_display_name(),
            self.pick("idle", "已关闭")
        )
    }

    pub fn warn_lid_on_battery(self) -> &'static str {
        self.pick(
            "Closed-lid stay-awake is unreliable on battery; plug in if you can",
            "合盖保活在电池供电下不太可靠，建议插电",
        )
    }

    pub fn warn_lid_best_effort(self) -> &'static str {
        self.pick(
            "Closed-lid standby is best-effort; lid open + display asleep is the reliable path",
            "合盖待命是尽力而为；最稳妥是开盖熄屏",
        )
    }

    pub fn notify_started_title(self) -> &'static str {
        self.pick("Screen-off standby is on", "已进入关屏待命")
    }

    pub fn notify_started_body_screen_off(self, seconds: u64, hotkey: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Display will sleep in about {seconds} seconds; the Mac stays awake. Press {hotkey} to end."
            ),
            Lang::Zh => format!("约 {seconds} 秒后关闭屏幕，电脑保持运行。按 {hotkey} 结束。"),
        }
    }

    pub fn notify_started_body_keep_awake(self, hotkey: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "The Mac will stay awake (display is not forced off). Press {hotkey} to end."
            ),
            Lang::Zh => format!("电脑将保持运行（不强制关屏）。按 {hotkey} 结束。"),
        }
    }

    pub fn notify_started_body_remote_user_present(self, hotkey: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Standby is on; the display stays under local control until you step away. Press {hotkey} to end."
            ),
            Lang::Zh => {
                format!("待命已开启；有人在用时屏幕仍由本机控制，离开后才会关屏。按 {hotkey} 结束。")
            }
        }
    }

    pub fn remaining_clause(self, duration: &str) -> String {
        match self.lang {
            Lang::En => format!(" {duration} left."),
            Lang::Zh => format!(" 剩余 {duration}。"),
        }
    }

    pub fn notify_ended_title(self) -> &'static str {
        self.pick("Standby ended", "关屏待命已结束")
    }

    pub fn notify_ended_user_body(self) -> &'static str {
        self.pick("Normal sleep policy restored.", "系统恢复正常睡眠策略。")
    }

    pub fn stop_user(self) -> &'static str {
        self.pick("Ended by you", "已由你结束")
    }

    pub fn stop_battery(self) -> &'static str {
        self.pick(
            "Battery too low; standby ended to protect remaining charge",
            "电量过低，已结束待命以免耗干电池",
        )
    }

    pub fn stop_thermal(self) -> &'static str {
        self.pick("System overheating; standby ended", "系统过热，已结束待命")
    }

    pub fn stop_duration(self) -> &'static str {
        self.pick(
            "The set duration elapsed; standby ended",
            "到达设定时长，已结束待命",
        )
    }

    pub fn stop_quit(self) -> &'static str {
        self.pick(
            "App quit; normal sleep restored",
            "应用退出，已恢复正常睡眠",
        )
    }

    pub fn stop_assertion(self) -> &'static str {
        self.pick(
            "Could not prevent system sleep; standby cancelled",
            "无法阻止系统睡眠，已取消待命",
        )
    }

    pub fn already_running(self) -> &'static str {
        self.pick(
            "Never Sleep is already running in the menu bar.",
            "Never Sleep 已在菜单栏运行。",
        )
    }

    pub fn menu_ipc_timed_out(self) -> &'static str {
        self.pick(
            "The menu bar did not answer in time. Standby was not started in this process.",
            "菜单栏未及时响应，本进程未开启待命。",
        )
    }

    pub fn ipc_not_started(self, err: &str) -> String {
        match self.lang {
            Lang::En => format!("IPC did not start: {err} (CLI will run in the foreground)"),
            Lang::Zh => format!("IPC 未启动：{err}（命令行将以前台模式工作）"),
        }
    }

    pub fn hotkey_failed(self, hotkey: &str) -> String {
        match self.lang {
            Lang::En => format!("Could not register {hotkey}; use the menu instead."),
            Lang::Zh => format!("快捷键 {hotkey} 注册失败，仍可通过菜单操作。"),
        }
    }

    pub fn login_item_title(self) -> &'static str {
        self.pick("Login item", "登录项")
    }

    pub fn menubar_macos_only(self) -> &'static str {
        self.pick(
            "The menu bar is only available on macOS.",
            "菜单栏仅支持 macOS。",
        )
    }

    pub fn cleanup_done(self) -> &'static str {
        self.pick(
            "Tried to restore clamshell sleep and clear leftover locks.",
            "已尝试还原合盖睡眠标志并清除残留锁。",
        )
    }

    pub fn failed(self) -> &'static str {
        self.pick("Failed", "失败")
    }

    pub fn not_in_standby(self) -> &'static str {
        self.pick("Not in standby.", "未待命。")
    }

    pub fn menubar_missing_foreground_json(self) -> &'static str {
        self.pick(
            "Menu bar is not running; starting in the foreground (query JSON status from another terminal).",
            "菜单栏未运行，以前台模式启动（JSON 状态请另开终端查询）。",
        )
    }

    pub fn menubar_not_running(self) -> String {
        match self.lang {
            Lang::En => format!(
                "Menu bar is not running. Open {}, or run `never-sleep on` in the foreground.",
                self.app_display_name()
            ),
            Lang::Zh => format!(
                "菜单栏未运行。请先打开 {}，或使用 never-sleep on 以前台方式启动。",
                self.app_display_name()
            ),
        }
    }

    pub fn cli_status_line(
        self,
        display: &str,
        lid: &str,
        power: &str,
        battery: Option<u8>,
    ) -> String {
        let batt = battery
            .map(|b| match self.lang {
                Lang::En => format!(" · battery {b}%"),
                Lang::Zh => format!(" · 电量 {b}%"),
            })
            .unwrap_or_default();
        match self.lang {
            Lang::En => format!("Standby · display {display} · {lid} · {power}{batt}"),
            Lang::Zh => format!("待命中 · 屏幕 {display} · {lid} · {power}{batt}"),
        }
    }

    pub fn foreground_failed(self) -> &'static str {
        self.pick("Could not enter standby", "未能进入待命")
    }

    pub fn foreground_already_running(self) -> &'static str {
        self.pick(
            "Standby is already running in another Never Sleep process.",
            "另一 Never Sleep 进程已在待命。",
        )
    }

    pub fn foreground_started(self) -> &'static str {
        self.pick(
            "Standby is on. The display will sleep; the Mac stays awake. Press Ctrl-C to end.",
            "关屏待命已开启。屏幕将关闭，电脑保持运行。按 Ctrl-C 结束。",
        )
    }

    pub fn foreground_status_hint(self) -> &'static str {
        self.pick(
            "Query status with `never-sleep status --json` if the menu bar is running.",
            "状态可用 `never-sleep status --json` 查询（若菜单栏正在运行）。",
        )
    }

    pub fn foreground_ended(self) -> &'static str {
        self.pick("Standby ended.", "已结束待命。")
    }

    pub fn power_assertion_failed(self, err: &str) -> String {
        match self.lang {
            Lang::En => format!("Power assertion failed: {err}"),
            Lang::Zh => format!("电源断言失败：{err}"),
        }
    }

    pub fn sleep_display_failed(self, err: &str) -> String {
        match self.lang {
            Lang::En => format!("Could not sleep the display: {err}"),
            Lang::Zh => format!("关屏失败：{err}"),
        }
    }

    pub fn assertion_reason(self) -> &'static str {
        self.pick(
            "Never Sleep: keep the Mac awake for remote clients",
            "Never Sleep：保持系统运行供远程客户端连接",
        )
    }

    pub fn idle_assertion_failed(self) -> &'static str {
        self.pick(
            "Could not create PreventUserIdleSystemSleep assertion",
            "无法创建 PreventUserIdleSystemSleep 断言",
        )
    }

    pub fn clamshell_restore_failed(self) -> &'static str {
        self.pick(
            "Could not restore clamshell sleep after adopting without lid-awake",
            "接管后未能恢复合盖睡眠（本次未占用合盖标志）",
        )
    }

    pub fn displaysleep_and_wrangler_failed(self) -> &'static str {
        self.pick(
            "pmset displaysleepnow failed, and IODisplayWrangler was not found",
            "pmset displaysleepnow 失败，且找不到 IODisplayWrangler",
        )
    }

    pub fn displaysleep_both_failed(self, ret: i32) -> String {
        match self.lang {
            Lang::En => format!(
                "Could not sleep the display: pmset and IORequestIdle both failed (IOReturn {ret})"
            ),
            Lang::Zh => format!("关屏失败：pmset 与 IORequestIdle 均未成功 (IOReturn {ret})"),
        }
    }

    pub fn launchctl_load_failed(self) -> &'static str {
        self.pick("launchctl load failed", "launchctl load 失败")
    }

    pub fn doctor_title(self) -> &'static str {
        self.pick("Never Sleep diagnostics", "Never Sleep 诊断")
    }

    pub fn doctor_snapshot(
        self,
        on_ac: bool,
        battery: Option<u8>,
        lid_closed: bool,
        display_asleep: Option<bool>,
        hid_idle_ms: u64,
        thermal: &str,
    ) -> String {
        let power = if on_ac { "AC" } else { self.power_battery() };
        match self.lang {
            Lang::En => format!(
                "Power: {power}\nBattery: {battery:?}\nLid closed: {lid_closed}\nDisplay asleep: {display_asleep:?}\nHID idle: {hid_idle_ms} ms\nThermal: {thermal}\n"
            ),
            Lang::Zh => format!(
                "电源: {power}\n电量: {battery:?}\n合盖: {lid_closed}\n屏幕休眠: {display_asleep:?}\nHID空闲: {hid_idle_ms} ms\n过热: {thermal}\n"
            ),
        }
    }

    pub fn stub_not_macos(self) -> &'static str {
        self.pick(
            "This platform is not macOS. Run `never-sleep doctor` on a Mac.",
            "当前平台不是 macOS。请在 Mac 上运行 `never-sleep doctor`。",
        )
    }

    pub fn ipc_timeout(self) -> &'static str {
        self.pick("Timed out", "超时")
    }

    pub fn parse_duration_error(self, raw: &str) -> String {
        match self.lang {
            Lang::En => format!("Could not parse duration: {raw}"),
            Lang::Zh => format!("无法解析时长: {raw}"),
        }
    }

    pub fn parse_duration_min_hour(self) -> &'static str {
        self.pick(
            "Duration must be at least 1 hour, or use indefinite",
            "时长至少 1 小时，或使用 indefinite",
        )
    }

    pub fn parse_time_format(self, raw: &str) -> String {
        match self.lang {
            Lang::En => format!("Time must be HH:MM, got {raw}"),
            Lang::Zh => format!("时间格式应为 HH:MM，收到 {raw}"),
        }
    }

    pub fn parse_invalid_hour(self, raw: &str) -> String {
        match self.lang {
            Lang::En => format!("Invalid hour: {raw}"),
            Lang::Zh => format!("无效小时: {raw}"),
        }
    }

    pub fn parse_invalid_minute(self, raw: &str) -> String {
        match self.lang {
            Lang::En => format!("Invalid minute: {raw}"),
            Lang::Zh => format!("无效分钟: {raw}"),
        }
    }

    pub fn parse_invalid_time(self, raw: &str) -> String {
        match self.lang {
            Lang::En => format!("Invalid time: {raw}"),
            Lang::Zh => format!("无效时间: {raw}"),
        }
    }

    pub fn cli_about(self) -> &'static str {
        self.pick(
            "Never Sleep: turn the Mac display off and keep the machine awake for ChatGPT / Codex remote sessions",
            "Never Sleep：关掉 Mac 屏幕、不让电脑睡眠，方便 ChatGPT / Codex 远程连接",
        )
    }
}

pub fn app_display_name(lang: Lang) -> &'static str {
    Tr::new(lang).app_display_name()
}

pub fn onboarding(lang: Lang) -> &'static str {
    Tr::new(lang).onboarding()
}

const ONBOARDING_EN: &str = "\
Never Sleep turns the display off while the Mac stays awake, so ChatGPT / Codex can keep connecting.

Get started
1. Start standby — click “Start Screen-Off Standby”. The display sleeps after about 1.5 seconds.
2. Stays out of your way — it will not fight you for the screen while you type; it sleeps again after you leave.
3. Come back — press ⌥⌘P, or choose “End Standby” in the menu.

Keep in mind
• Closed-lid stay-awake is best-effort, more reliable on power. Lid open + display asleep is the reliable path.
• Standby ends automatically on low battery so the pack is not drained.
• Quitting restores normal sleep. Energy Saver settings are never rewritten.\
";

const ONBOARDING_ZH: &str = "\
Never Sleep 会关掉屏幕，同时不让 Mac 进入睡眠。ChatGPT / Codex 等远程客户端仍可连上这台电脑。\n\n\
怎么用\n\
1. 开始待命 — 点「开始关屏待命」，约 1.5 秒后屏幕关闭。\n\
2. 不抢屏幕 — 人在电脑前绝不强制关屏；走开后再自动关闭。\n\
3. 随时回来 — 按 ⌥⌘P，或点菜单「结束待命」。\n\n\
请留意\n\
• 合盖保活是尽力而为，插电更稳；最可靠仍是开盖熄屏。\n\
• 电量过低会自动结束，避免把电池耗干。\n\
• 退出后立即恢复系统睡眠，不会改写节能设置。\
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_language_tags() {
        assert_eq!(Lang::parse_opt("en"), Some(Lang::En));
        assert_eq!(Lang::parse_opt("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::parse_opt("zh-Hans-CN"), Some(Lang::Zh));
        assert_eq!(Lang::parse_opt("zh_CN"), Some(Lang::Zh));
        assert_eq!(Lang::parse_opt("fr_FR"), None);
        assert_eq!(Lang::parse_opt("C"), None);
        assert_eq!(Lang::En.cloud_tag(), "en");
        assert_eq!(Lang::Zh.cloud_tag(), "zh");
    }

    #[test]
    fn preferred_tags_use_first() {
        assert_eq!(
            Lang::from_preferred_tags(&["zh-Hans-CN", "en-US"]),
            Some(Lang::Zh)
        );
        assert_eq!(
            Lang::from_preferred_tags(&["en-US", "zh-Hans"]),
            Some(Lang::En)
        );
        assert_eq!(Lang::from_preferred_tags(&["fr-FR"]), Some(Lang::En));
    }

    #[test]
    fn english_is_default() {
        assert_eq!(Lang::default(), Lang::En);
        assert_eq!(
            Tr::new(Lang::En).start_standby(),
            "Start Screen-Off Standby"
        );
        assert_eq!(Tr::new(Lang::En).app_display_name(), "Never Sleep");
    }

    #[test]
    fn product_name_is_never_sleep_in_both_languages() {
        assert_eq!(Tr::new(Lang::En).app_display_name(), APP_NAME);
        assert_eq!(Tr::new(Lang::Zh).app_display_name(), APP_NAME);
        let zh = Tr::new(Lang::Zh);
        for s in [
            zh.app_display_name(),
            zh.welcome_title(),
            zh.start_standby(),
            zh.end_standby(),
            zh.help_step1_detail(),
            zh.notify_started_title(),
            zh.notify_ended_title(),
            zh.already_running(),
            zh.foreground_started(),
            zh.assertion_reason(),
            zh.doctor_title(),
            zh.cli_about(),
            zh.onboarding(),
            zh.panel_idle_title(),
            zh.panel_active_title(),
            zh.menubar_not_running().as_str(),
        ] {
            assert!(
                !s.contains("熄屏待命"),
                "the product is Never Sleep in both languages, got {s}"
            );
        }
        assert_eq!(zh.start_standby(), "开始关屏待命");
        assert_eq!(zh.panel_active_title(), "关屏待命中");
        assert_eq!(zh.end_standby(), "结束待命");
    }

    #[test]
    fn panel_chrome_strings_are_bilingual() {
        let en = Tr::new(Lang::En);
        let zh = Tr::new(Lang::Zh);
        assert_eq!(en.panel_summary_idle(), "Display off, Mac stays online");
        assert_eq!(zh.panel_summary_idle(), "屏幕将关闭，Mac 保持在线");
        assert_eq!(
            en.panel_summary_active(),
            "Display asleep, Mac stays online"
        );
        assert_eq!(zh.panel_summary_active(), "屏幕已休眠，Mac 仍在线");
        assert_eq!(en.remote_access(), "Remote access");
        assert_eq!(zh.remote_access(), "远程访问");
        assert_eq!(
            en.remote_disabled(),
            "Remote access is off. Enable it in Settings to pair your phone."
        );
        assert_eq!(
            zh.remote_disabled(),
            "远程访问已关闭。请在设置中开启后配对手机。"
        );
        assert_eq!(en.more_settings(), "More Settings");
        assert_eq!(zh.more_settings(), "更多设置");
        assert_eq!(en.sleep_display_now_action(), "Sleep Display Now");
        assert_eq!(zh.sleep_display_now_action(), "立即熄屏");
        assert_ne!(
            en.sleep_display_now_action(),
            en.screen_off_now(),
            "the moon-panel action is a one-shot, not the settings toggle"
        );
        assert_ne!(zh.sleep_display_now_action(), zh.screen_off_now());
        assert_ne!(
            en.more_settings(),
            en.settings_title(),
            "footer is More Settings; the sheet heading stays Settings"
        );
        assert_eq!(en.settings_title(), "Settings");
        assert_eq!(zh.settings_title(), "设置");
        assert_eq!(en.phone_board(), "Phone board");
        assert_eq!(zh.phone_board(), "手机看板");
        assert_eq!(en.pairing_code(), "Pairing code");
        assert_eq!(zh.pairing_code(), "配对码");
        assert_eq!(
            en.pairing_unavailable(),
            "Pairing is not ready yet. Open Settings, or retry in a moment."
        );
        assert_eq!(
            zh.pairing_unavailable(),
            "配对尚未就绪。请打开设置，或稍后再试。"
        );
        assert_ne!(
            zh.pairing_unavailable(),
            "pairing_unavailable",
            "human CLI output must not print the IPC code"
        );
        assert_eq!(en.back(), "Back");
        assert_eq!(zh.back(), "返回");
        assert_eq!(en.panel_section_session(), "Session");
        assert_eq!(zh.panel_section_session(), "待命");
        assert_eq!(en.panel_section_display(), "Display");
        assert_eq!(zh.panel_section_display(), "屏幕");
        assert_eq!(en.panel_section_lid(), "Lid");
        assert_eq!(zh.panel_section_lid(), "合盖");
        assert_eq!(en.panel_section_safeguards(), "Safeguards");
        assert_eq!(zh.panel_section_safeguards(), "保护");
        assert_eq!(en.panel_section_general(), "General");
        assert_eq!(zh.panel_section_general(), "通用");
        assert_eq!(en.sidebar_group_options(), "Options");
        assert_eq!(zh.sidebar_group_options(), "选项");
        assert_eq!(en.sidebar_group_guide(), "Guide");
        assert_eq!(zh.sidebar_group_guide(), "指南");
        assert_eq!(en.show_window(), "Show Window");
        assert_eq!(zh.show_window(), "显示窗口");
        assert!(en.pane_display_lead().contains("brightness"));
        assert!(zh.pane_display_lead().contains("亮度"));
        assert_eq!(en.pane_general_lead(), "Applies to this Mac only.");
        assert!(en.panel_hotkey_hint().contains(DEFAULT_HOTKEY_LABEL));
        assert!(zh.panel_hotkey_hint().contains(DEFAULT_HOTKEY_LABEL));
        assert!(en.panel_hotkey_hint().contains("display off"));
        assert!(zh.panel_hotkey_hint().contains("屏幕"));
    }

    #[test]
    fn help_copy_is_sectioned() {
        let en = Tr::new(Lang::En);
        let zh = Tr::new(Lang::Zh);
        assert_eq!(en.help_how(), "Get started");
        assert_eq!(zh.help_how(), "怎么用");
        assert!(en.onboarding().contains("Get started"));
        assert!(en.onboarding().contains("Keep in mind"));
        assert!(en.onboarding().contains("power"));
        assert!(en.onboarding().contains(DEFAULT_HOTKEY_LABEL));
        assert!(zh.onboarding().contains("怎么用"));
        assert!(zh.onboarding().contains("请留意"));
        assert!(zh.onboarding().contains(DEFAULT_HOTKEY_LABEL));
        assert!(
            en.help_note_lid().contains("power"),
            "{}",
            en.help_note_lid()
        );
        assert!(zh.help_note_lid().contains("插电"));
    }

    #[test]
    fn parse_aliases_and_unknown() {
        assert_eq!(Lang::parse_opt("ENG"), Some(Lang::En));
        assert_eq!(Lang::parse_opt("chinese"), Some(Lang::Zh));
        assert_eq!(Lang::parse_opt("cn"), Some(Lang::Zh));
        assert_eq!(Lang::parse_opt("chi"), Some(Lang::Zh));
        assert_eq!(Lang::parse_opt(""), None);
        assert!(Lang::En.override_or() == Lang::En || Lang::En.override_or() == Lang::Zh);
        assert!(Lang::Zh.is_chinese());
        assert!(!Lang::En.is_chinese());
    }

    #[test]
    fn preferred_tags_empty_is_none() {
        let empty: [&str; 0] = [];
        assert_eq!(Lang::from_preferred_tags(&empty), None);
    }

    #[test]
    fn free_functions_match_translator() {
        assert_eq!(
            app_display_name(Lang::En),
            Tr::new(Lang::En).app_display_name()
        );
        assert_eq!(onboarding(Lang::Zh), Tr::new(Lang::Zh).onboarding());
        assert!(onboarding(Lang::En).contains("Start Screen-Off Standby"));
        assert!(Tr::new(Lang::En).cli_about().contains("Codex"));
        assert!(Tr::new(Lang::Zh).cli_about().contains("Codex"));
    }

    #[test]
    fn stop_labels_differ_by_language() {
        assert_ne!(
            Tr::new(Lang::En).stop_battery(),
            Tr::new(Lang::Zh).stop_battery()
        );
        assert_eq!(
            Tr::new(Lang::En).notify_ended_user_body(),
            "Normal sleep policy restored."
        );
    }

    #[test]
    fn remote_user_present_start_copy_explains_local_display_control() {
        let en = Tr::new(Lang::En).notify_started_body_remote_user_present("⌥⌘P");
        let zh = Tr::new(Lang::Zh).notify_started_body_remote_user_present("⌥⌘P");
        assert!(en.contains("local control"));
        assert!(en.contains("⌥⌘P"));
        assert!(!en.contains("will sleep in about"));
        assert!(zh.contains("本机控制"));
        assert!(zh.contains("⌥⌘P"));
        assert!(!zh.contains("秒后关闭屏幕"));
        assert_ne!(en, zh);
    }

    #[test]
    fn clamshell_restore_failed_is_bilingual() {
        let en = Tr::new(Lang::En).clamshell_restore_failed();
        let zh = Tr::new(Lang::Zh).clamshell_restore_failed();
        assert!(en.contains("clamshell"));
        assert!(zh.contains("合盖"));
        assert_ne!(en, zh);
        assert!(!zh.contains("熄屏待命"));
    }

    #[test]
    fn foreground_already_running_is_bilingual() {
        assert_eq!(
            Tr::new(Lang::En).foreground_already_running(),
            "Standby is already running in another Never Sleep process."
        );
        assert_eq!(
            Tr::new(Lang::Zh).foreground_already_running(),
            "另一 Never Sleep 进程已在待命。"
        );
        assert!(!Tr::new(Lang::Zh)
            .foreground_already_running()
            .contains("熄屏待命"));
    }

    #[test]
    fn menu_ipc_timed_out_is_bilingual() {
        assert_eq!(
            Tr::new(Lang::En).menu_ipc_timed_out(),
            "The menu bar did not answer in time. Standby was not started in this process."
        );
        assert_eq!(
            Tr::new(Lang::Zh).menu_ipc_timed_out(),
            "菜单栏未及时响应，本进程未开启待命。"
        );
        assert!(!Tr::new(Lang::Zh).menu_ipc_timed_out().contains("熄屏待命"));
    }
}
