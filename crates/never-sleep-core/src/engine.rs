use crate::config::AppConfig;
use crate::duration::{
    countdown_secs, deadline_unix_secs, format_duration, hours_elapsed_ms, session_remaining_ms,
};
use crate::i18n::{Lang, Tr};
use crate::status::{build_view_model, HostSnapshot, JsonStatus, ViewModel};
use crate::DurationPref;
use crate::SLEEP_DISPLAY_DEBOUNCE_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerPlan {
    pub prevent_idle_sleep: bool,
    pub prevent_system_sleep: bool,
    pub prevent_disk_idle: bool,
    pub network_client: bool,
    pub disable_clamshell_sleep: bool,
}

impl PowerPlan {
    pub fn off() -> Self {
        Self {
            prevent_idle_sleep: false,
            prevent_system_sleep: false,
            prevent_disk_idle: false,
            network_client: false,
            disable_clamshell_sleep: false,
        }
    }

    pub fn for_session(cfg: &AppConfig, on_ac: bool) -> Self {
        Self {
            prevent_idle_sleep: true,
            // Apple 文档：PreventSystemSleep 主要在 AC 下有效
            prevent_system_sleep: cfg.keep_awake_on_lid_close && on_ac,
            prevent_disk_idle: true,
            network_client: true,
            disable_clamshell_sleep: cfg.keep_awake_on_lid_close,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    ApplyPower(PowerPlan),
    ReleasePower,
    SleepDisplay,
    LockSession,
    Notify { title: String, body: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Start,
    StartWith(DurationPref),
    Stop {
        reason: StopReason,
    },
    Toggle,
    Tick,
    DisplayWoke,
    DisplaySlept,
    /// Person asked to darken the panel now. Does not end standby.
    SleepDisplayNow,
    /// Phone / cloud start. Standby begins, but the first display-sleep still
    /// respects `user_present` (never fight someone at the keyboard).
    StartRemote,
    StartRemoteWith(DurationPref),
    /// Take over a live session from another process (foreground → menu).
    /// Uses remote display semantics and the leftover duration, not a fresh
    /// local one-click start.
    Handoff {
        pref: DurationPref,
        remaining_secs: Option<u64>,
        elapsed_secs: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    User,
    BatteryFloor,
    ThermalEmergency,
    DurationElapsed,
    AppQuit,
    AssertionFailed,
}

impl StopReason {
    pub fn label(self, lang: Lang) -> &'static str {
        let t = Tr::new(lang);
        match self {
            Self::User => t.stop_user(),
            Self::BatteryFloor => t.stop_battery(),
            Self::ThermalEmergency => t.stop_thermal(),
            Self::DurationElapsed => t.stop_duration(),
            Self::AppQuit => t.stop_quit(),
            Self::AssertionFailed => t.stop_assertion(),
        }
    }

    /// Stable English text for JSON / agents.
    pub fn label_en(self) -> &'static str {
        self.label(Lang::En)
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::BatteryFloor => "battery_floor",
            Self::ThermalEmergency => "thermal_emergency",
            Self::DurationElapsed => "duration_elapsed",
            Self::AppQuit => "app_quit",
            Self::AssertionFailed => "assertion_failed",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "user" => Some(Self::User),
            "battery_floor" => Some(Self::BatteryFloor),
            "thermal_emergency" => Some(Self::ThermalEmergency),
            "duration_elapsed" => Some(Self::DurationElapsed),
            "app_quit" => Some(Self::AppQuit),
            "assertion_failed" => Some(Self::AssertionFailed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct Session {
    started_ms: u64,
    started_continuous_ms: u64,
    started_unix: i64,
    duration: DurationPref,
    deadline_unix: Option<i64>,
    /// Seconds already elapsed in the donor process. Menu `Instant` cannot
    /// represent a multi-hour session that started before this process.
    elapsed_base_secs: u64,
    initial_display_off_sent: bool,
    last_sleep_display_ms: Option<u64>,
    /// Remote start: skip the first forced sleep while a person is at the Mac.
    remote: bool,
}

struct SessionClocks {
    started_ms: u64,
    started_continuous_ms: u64,
    started_unix: i64,
    deadline: Option<i64>,
    elapsed_base_secs: u64,
}

fn session_clocks(pref: DurationPref, host: &HostSnapshot) -> SessionClocks {
    SessionClocks {
        started_ms: host.monotonic_ms,
        started_continuous_ms: host.continuous_ms,
        started_unix: host.unix_secs,
        deadline: deadline_unix_secs(host.unix_secs, host.utc_offset_secs, host.unix_secs, pref),
        elapsed_base_secs: 0,
    }
}

fn clocks_from_remaining(host: &HostSnapshot, remaining: u64) -> SessionClocks {
    SessionClocks {
        started_ms: host.monotonic_ms,
        started_continuous_ms: host.continuous_ms,
        started_unix: host.unix_secs,
        deadline: Some(host.unix_secs + remaining as i64),
        elapsed_base_secs: 0,
    }
}

fn handoff_clocks(
    pref: DurationPref,
    remaining_secs: Option<u64>,
    elapsed_secs: Option<u64>,
    host: &HostSnapshot,
) -> SessionClocks {
    let mut clocks = match (pref, remaining_secs) {
        (DurationPref::Indefinite, _) => session_clocks(pref, host),
        (_, Some(remaining)) => {
            let rem = match pref {
                DurationPref::Hours { hours } => {
                    remaining.min(u64::from(hours).saturating_mul(3600))
                }
                DurationPref::UntilLocal { .. } => remaining.min(2 * 86_400),
                DurationPref::Indefinite => remaining,
            };
            clocks_from_remaining(host, rem)
        }
        (_, None) => session_clocks(pref, host),
    };
    clocks.elapsed_base_secs = elapsed_secs.unwrap_or(0);
    clocks
}

#[derive(Debug, Clone)]
pub struct Engine {
    pub config: AppConfig,
    session: Option<Session>,
    optimistic_display_asleep: bool,
    last_stop_reason: Option<StopReason>,
    last_plan: PowerPlan,
}

impl Engine {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            session: None,
            optimistic_display_asleep: false,
            last_stop_reason: None,
            last_plan: PowerPlan::off(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    pub fn handle(&mut self, input: Input, host: &HostSnapshot) -> Vec<Effect> {
        let mut effects = Vec::new();
        match input {
            Input::Start => self.start(self.config.duration, host, false, &mut effects),
            Input::StartWith(pref) => self.start(pref, host, false, &mut effects),
            Input::StartRemote => self.start(self.config.duration, host, true, &mut effects),
            Input::StartRemoteWith(pref) => self.start(pref, host, true, &mut effects),
            Input::Handoff {
                pref,
                remaining_secs,
                elapsed_secs,
            } => self.handoff(pref, remaining_secs, elapsed_secs, host, &mut effects),
            Input::Toggle => {
                if self.session.is_some() {
                    self.stop(StopReason::User, host, &mut effects);
                } else {
                    self.start(self.config.duration, host, false, &mut effects);
                }
            }
            Input::Stop { reason } => self.stop(reason, host, &mut effects),
            Input::DisplayWoke => {
                self.optimistic_display_asleep = false;
                self.tick(host, &mut effects);
            }
            Input::DisplaySlept => {
                self.optimistic_display_asleep = true;
            }
            Input::Tick => self.tick(host, &mut effects),
            Input::SleepDisplayNow => self.sleep_display_now(host, &mut effects),
        }
        effects
    }

    fn start(
        &mut self,
        pref: DurationPref,
        host: &HostSnapshot,
        remote: bool,
        effects: &mut Vec<Effect>,
    ) {
        if self.session.is_some() {
            return;
        }
        self.begin_session(pref, session_clocks(pref, host), remote, host, effects);
    }

    fn handoff(
        &mut self,
        pref: DurationPref,
        remaining_secs: Option<u64>,
        elapsed_secs: Option<u64>,
        host: &HostSnapshot,
        effects: &mut Vec<Effect>,
    ) {
        if self.session.is_some() {
            return;
        }
        if remaining_secs == Some(0) {
            return;
        }
        self.begin_session(
            pref,
            handoff_clocks(pref, remaining_secs, elapsed_secs, host),
            true,
            host,
            effects,
        );
    }

    fn begin_session(
        &mut self,
        pref: DurationPref,
        clocks: SessionClocks,
        remote: bool,
        host: &HostSnapshot,
        effects: &mut Vec<Effect>,
    ) {
        self.config.duration = pref;
        self.session = Some(Session {
            started_ms: clocks.started_ms,
            started_continuous_ms: clocks.started_continuous_ms,
            started_unix: clocks.started_unix,
            duration: pref,
            deadline_unix: clocks.deadline,
            elapsed_base_secs: clocks.elapsed_base_secs,
            initial_display_off_sent: false,
            last_sleep_display_ms: None,
            remote,
        });
        self.optimistic_display_asleep = host.display_asleep.unwrap_or(false);
        self.last_stop_reason = None;
        let plan = PowerPlan::for_session(&self.config, host.on_ac);
        self.last_plan = plan;
        effects.push(Effect::ApplyPower(plan));

        let t = self.config.tr();
        let deferred_sleep =
            remote && self.config.screen_off && host.user_present(self.config.user_idle_resleep_ms);
        let mut body = if deferred_sleep {
            t.notify_started_body_remote_user_present(crate::DEFAULT_HOTKEY_LABEL)
        } else if self.config.screen_off {
            t.notify_started_body_screen_off(
                self.config.display_off_delay_ms.max(1).div_ceil(1000),
                crate::DEFAULT_HOTKEY_LABEL,
            )
        } else {
            t.notify_started_body_keep_awake(crate::DEFAULT_HOTKEY_LABEL)
        };
        if let Some(d) = clocks.deadline {
            let rem = d.saturating_sub(host.unix_secs).max(0) as u64;
            body.push_str(&t.remaining_clause(&format_duration(self.config.lang(), rem)));
        }
        effects.push(Effect::Notify {
            title: t.notify_started_title().into(),
            body,
        });
        self.tick(host, effects);
    }

    fn stop(&mut self, reason: StopReason, host: &HostSnapshot, effects: &mut Vec<Effect>) {
        if self.session.take().is_none() {
            return;
        }
        self.last_plan = PowerPlan::off();
        self.optimistic_display_asleep = host.display_asleep.unwrap_or(false);
        self.last_stop_reason = Some(reason);
        effects.push(Effect::ReleasePower);
        let t = self.config.tr();
        if !matches!(reason, StopReason::AppQuit | StopReason::User) {
            effects.push(Effect::Notify {
                title: t.notify_ended_title().into(),
                body: reason.label(self.config.lang()).into(),
            });
        } else if matches!(reason, StopReason::User) {
            effects.push(Effect::Notify {
                title: t.notify_ended_title().into(),
                body: t.notify_ended_user_body().into(),
            });
        }
    }

    fn tick(&mut self, host: &HostSnapshot, effects: &mut Vec<Effect>) {
        if self.session.is_none() {
            return;
        }
        if let Some(reason) = self.should_auto_stop(host) {
            self.stop(reason, host, effects);
            return;
        }
        let plan = PowerPlan::for_session(&self.config, host.on_ac);
        if plan != self.last_plan {
            self.last_plan = plan;
            effects.push(Effect::ApplyPower(plan));
        }
        if self.should_sleep_display(host) {
            self.emit_sleep_display(host, effects);
        }
    }

    fn sleep_display_now(&mut self, host: &HostSnapshot, effects: &mut Vec<Effect>) {
        self.emit_sleep_display(host, effects);
    }

    fn emit_sleep_display(&mut self, host: &HostSnapshot, effects: &mut Vec<Effect>) {
        let first = self
            .session
            .as_ref()
            .is_some_and(|s| !s.initial_display_off_sent);
        if let Some(session) = self.session.as_mut() {
            session.initial_display_off_sent = true;
            session.last_sleep_display_ms = Some(host.monotonic_ms);
        }
        self.optimistic_display_asleep = true;
        effects.push(Effect::SleepDisplay);
        if first && self.config.lock_screen {
            effects.push(Effect::LockSession);
        }
    }

    fn should_auto_stop(&self, host: &HostSnapshot) -> Option<StopReason> {
        let session = self.session.as_ref()?;
        if host.thermal.is_emergency() {
            return Some(StopReason::ThermalEmergency);
        }
        if let Some(floor) = self.config.battery_floor_percent {
            if !host.on_ac {
                if let Some(pct) = host.battery_percent {
                    if pct < floor {
                        return Some(StopReason::BatteryFloor);
                    }
                }
            }
        }
        if self.remaining_ms(session, host) == Some(0) {
            return Some(StopReason::DurationElapsed);
        }
        None
    }

    fn display_asleep(&self, host: &HostSnapshot) -> bool {
        host.display_asleep
            .unwrap_or(self.optimistic_display_asleep)
    }

    fn should_sleep_display(&self, host: &HostSnapshot) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        if !self.config.screen_off {
            return false;
        }
        let elapsed = host.monotonic_ms.saturating_sub(session.started_ms);
        if elapsed < self.config.display_off_delay_ms {
            return false;
        }
        if let Some(last) = session.last_sleep_display_ms {
            if host.monotonic_ms.saturating_sub(last) < SLEEP_DISPLAY_DEBOUNCE_MS {
                return false;
            }
        }
        if !session.initial_display_off_sent {
            if session.remote && host.user_present(self.config.user_idle_resleep_ms) {
                return false;
            }
            // Local one-click start: request display sleep even if the panel is already dark.
            return true;
        }
        if !self.config.resleep_display {
            return false;
        }
        // 人在电脑前：把屏幕控制权交还，绝不抢
        if host.user_present(self.config.user_idle_resleep_ms) {
            return false;
        }
        // 人不在就周期性重申关屏。远程代理若把屏幕点亮，几秒内会被盖回去。
        // 屏幕本来就灭时，pmset displaysleepnow 基本是空操作。
        true
    }

    pub fn view(&self, host: &HostSnapshot) -> ViewModel {
        let (elapsed, remaining) = match &self.session {
            Some(s) => (
                Some(self.elapsed_secs(s, host)),
                self.remaining_secs(s, host),
            ),
            None => (None, None),
        };
        let last_stop = self
            .last_stop_reason
            .map(|r| r.label(self.config.lang()).to_string());
        build_view_model(
            &self.config,
            self.is_active(),
            elapsed,
            remaining,
            host,
            last_stop.as_deref(),
            self.display_asleep(host),
        )
    }

    /// Elapsed milliseconds and optional remaining milliseconds for the UI clock.
    pub fn session_times(&self, host: &HostSnapshot) -> Option<(u64, Option<u64>)> {
        let session = self.session.as_ref()?;
        Some((
            self.session_elapsed_ms(session, host),
            self.remaining_ms(session, host),
        ))
    }

    fn remaining_ms(&self, session: &Session, host: &HostSnapshot) -> Option<u64> {
        session.deadline_unix.map(|deadline| {
            session_remaining_ms(
                session.duration,
                deadline,
                session.started_unix,
                session.started_continuous_ms,
                host.continuous_ms,
                host.unix_secs,
            )
        })
    }

    fn remaining_secs(&self, session: &Session, host: &HostSnapshot) -> Option<u64> {
        self.remaining_ms(session, host).map(countdown_secs)
    }

    fn session_elapsed_ms(&self, session: &Session, host: &HostSnapshot) -> u64 {
        let since_adopt = match session.duration {
            DurationPref::Hours { .. } => {
                hours_elapsed_ms(session.started_continuous_ms, host.continuous_ms)
            }
            DurationPref::UntilLocal { .. } | DurationPref::Indefinite => {
                host.monotonic_ms.saturating_sub(session.started_ms)
            }
        };
        session
            .elapsed_base_secs
            .saturating_mul(1_000)
            .saturating_add(since_adopt)
    }

    fn elapsed_secs(&self, session: &Session, host: &HostSnapshot) -> u64 {
        self.session_elapsed_ms(session, host) / 1_000
    }

    pub fn json_status(&self, host: &HostSnapshot) -> JsonStatus {
        let (elapsed, remaining) = match &self.session {
            Some(s) => (
                Some(self.elapsed_secs(s, host)),
                self.remaining_secs(s, host),
            ),
            None => (None, None),
        };
        JsonStatus {
            active: self.is_active(),
            display: if self.display_asleep(host) {
                "asleep".into()
            } else {
                "awake".into()
            },
            lid: if host.lid_closed {
                "closed".into()
            } else {
                "open".into()
            },
            on_ac: host.on_ac,
            battery: host.battery_percent,
            remaining_secs: remaining,
            user_present: host.user_present(self.config.user_idle_resleep_ms),
            elapsed_secs: elapsed,
            stop_reason: self.last_stop_reason.map(|r| r.label_en().to_string()),
            stop_reason_code: self.last_stop_reason.map(|r| r.code().to_string()),
            screen_off_enabled: self.config.screen_off,
            lid_awake_enabled: self.config.keep_awake_on_lid_close,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Thermal;

    fn host(ms: u64) -> HostSnapshot {
        HostSnapshot {
            monotonic_ms: ms,
            continuous_ms: ms,
            unix_secs: 1_700_000_000 + (ms as i64 / 1000),
            utc_offset_secs: 8 * 3600,
            on_ac: true,
            battery_percent: Some(80),
            lid_closed: false,
            display_asleep: Some(false),
            hid_idle_ms: 60_000,
            thermal: Thermal::Nominal,
        }
    }

    fn cfg() -> AppConfig {
        AppConfig {
            display_off_delay_ms: 1_500,
            user_idle_resleep_ms: 45_000,
            ..AppConfig::default()
        }
    }

    fn has_sleep(effects: &[Effect]) -> bool {
        effects.iter().any(|e| matches!(e, Effect::SleepDisplay))
    }

    fn has_release(effects: &[Effect]) -> bool {
        effects.iter().any(|e| matches!(e, Effect::ReleasePower))
    }

    fn apply_plan(effects: &[Effect]) -> Option<PowerPlan> {
        effects.iter().rev().find_map(|e| match e {
            Effect::ApplyPower(p) => Some(*p),
            _ => None,
        })
    }

    #[test]
    fn delay_then_sleep_display() {
        let mut eng = Engine::new(cfg());
        let e = eng.handle(Input::Start, &host(0));
        assert!(apply_plan(&e).unwrap().prevent_idle_sleep);
        assert!(!has_sleep(&e));
        let e = eng.handle(Input::Tick, &host(1_499));
        assert!(!has_sleep(&e));
        let e = eng.handle(Input::Tick, &host(1_500));
        assert!(has_sleep(&e));
        // debounce
        let mut h = host(2_000);
        h.display_asleep = Some(false);
        // optimistic asleep after previous sleep, host still false? engine set optimistic true
        // host Some(false) wins — but debounce 3s
        let e = eng.handle(Input::Tick, &h);
        assert!(!has_sleep(&e));
    }

    fn notify_body(effects: &[Effect]) -> String {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::Notify { body, .. } => Some(body.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn remote_on_while_user_present_does_not_promise_display_sleep() {
        let mut eng = Engine::new(cfg());
        let mut h = host(0);
        h.hid_idle_ms = 500;
        h.lid_closed = false;
        let effects = eng.handle(Input::StartRemote, &h);
        assert!(eng.is_active());
        let body = notify_body(&effects);
        assert!(
            !body.contains("will sleep in about"),
            "must not tell the person at the keyboard the display is about to sleep, got {body}"
        );
        assert!(
            body.contains("local control"),
            "remote start while someone is present should keep display under local control, got {body}"
        );
        assert!(!has_sleep(&effects));
    }

    #[test]
    fn handoff_while_user_present_does_not_sleep_display() {
        let mut eng = Engine::new(cfg());
        let mut h = host(0);
        h.hid_idle_ms = 500;
        h.lid_closed = false;
        let effects = eng.handle(
            Input::Handoff {
                pref: DurationPref::Hours { hours: 8 },
                remaining_secs: Some(3600),
                elapsed_secs: None,
            },
            &h,
        );
        assert!(eng.is_active());
        assert!(
            !has_sleep(&effects),
            "adopting a live session must not fight a person at the keyboard"
        );
        let mut later_host = host(1_500);
        later_host.hid_idle_ms = 500;
        later_host.lid_closed = false;
        let later = eng.handle(Input::Tick, &later_host);
        assert!(
            !has_sleep(&later),
            "handoff uses remote first-sleep rules so HID idle still wins after the delay"
        );
        let body = notify_body(&effects);
        assert!(
            !body.contains("will sleep in about"),
            "must not promise a display sleep while someone is present, got {body}"
        );
    }

    #[test]
    fn handoff_keeps_remaining_hour_not_full_pref() {
        let mut eng = Engine::new(cfg());
        let h0 = host(0);
        eng.handle(
            Input::Handoff {
                pref: DurationPref::Hours { hours: 8 },
                remaining_secs: Some(3600),
                elapsed_secs: None,
            },
            &h0,
        );
        assert_eq!(eng.json_status(&h0).remaining_secs, Some(3600));
        assert_eq!(eng.config.duration, DurationPref::Hours { hours: 8 });
        let mut h = host(10_000);
        h.unix_secs = h0.unix_secs;
        assert_eq!(eng.json_status(&h).remaining_secs, Some(3_590));
        assert!(eng.is_active());
    }

    #[test]
    fn handoff_until_local_with_zero_remaining_does_not_roll_to_tomorrow() {
        let mut eng = Engine::new(cfg());
        let h = host(0);
        let effects = eng.handle(
            Input::Handoff {
                pref: DurationPref::UntilLocal { hour: 8, minute: 0 },
                remaining_secs: Some(0),
                elapsed_secs: None,
            },
            &h,
        );
        assert!(
            !eng.is_active(),
            "an already-elapsed until-local session must not restart as tomorrow"
        );
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::ApplyPower(_))),
            "zero remaining is not a fresh start"
        );
        assert!(!eng.json_status(&h).active);
    }

    #[test]
    fn handoff_until_local_uses_supplied_remaining_not_wall_clock() {
        let mut eng = Engine::new(cfg());
        let h0 = host(0);
        eng.handle(
            Input::Handoff {
                pref: DurationPref::UntilLocal { hour: 8, minute: 0 },
                remaining_secs: Some(90),
                elapsed_secs: None,
            },
            &h0,
        );
        assert!(eng.is_active());
        assert_eq!(eng.json_status(&h0).remaining_secs, Some(90));
        assert_eq!(
            eng.config.duration,
            DurationPref::UntilLocal { hour: 8, minute: 0 }
        );
    }

    #[test]
    fn handoff_until_local_preserves_dst_25h_remaining() {
        let mut eng = Engine::new(cfg());
        let h = host(0);
        eng.handle(
            Input::Handoff {
                pref: DurationPref::UntilLocal { hour: 8, minute: 0 },
                remaining_secs: Some(25 * 3600),
                elapsed_secs: None,
            },
            &h,
        );
        assert_eq!(
            eng.json_status(&h).remaining_secs,
            Some(25 * 3600),
            "a fall-back 25-hour UntilLocal leftover must not be capped at 24h"
        );
        assert!(eng.is_active());
    }

    #[test]
    fn handoff_hours_with_zero_remaining_does_not_start() {
        let mut eng = Engine::new(cfg());
        let effects = eng.handle(
            Input::Handoff {
                pref: DurationPref::Hours { hours: 8 },
                remaining_secs: Some(0),
                elapsed_secs: None,
            },
            &host(0),
        );
        assert!(!eng.is_active());
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::ApplyPower(_))),
            "zero leftover seconds must not re-apply power just to tick-stop"
        );
    }

    #[test]
    fn handoff_preserves_elapsed_with_remaining() {
        let mut eng = Engine::new(cfg());
        let h = host(7 * 3_600_000);
        eng.handle(
            Input::Handoff {
                pref: DurationPref::Hours { hours: 8 },
                remaining_secs: Some(3600),
                elapsed_secs: Some(7 * 3600),
            },
            &h,
        );
        let st = eng.json_status(&h);
        assert_eq!(st.remaining_secs, Some(3600));
        assert_eq!(
            st.elapsed_secs,
            Some(7 * 3600),
            "adopting a 7h-old session must not reset the panel and phone elapsed clock"
        );
        assert!(eng.is_active());
    }

    #[test]
    fn handoff_elapsed_does_not_use_menu_process_clock() {
        let mut eng = Engine::new(cfg());
        // Menu bar just launched: process Instant is ~1.5s, not the 7h session.
        let h = host(1_500);
        eng.handle(
            Input::Handoff {
                pref: DurationPref::Hours { hours: 8 },
                remaining_secs: Some(3600),
                elapsed_secs: Some(7 * 3600),
            },
            &h,
        );
        let st = eng.json_status(&h);
        assert_eq!(st.remaining_secs, Some(3600));
        assert_eq!(
            st.elapsed_secs,
            Some(7 * 3600),
            "elapsed must keep the handed-off 7h, not saturate to menu uptime"
        );
        let times = eng.session_times(&h).expect("adopted session");
        assert_eq!(times.0, 7 * 3_600_000);
        let later = host(11_500);
        let later_st = eng.json_status(&later);
        assert_eq!(later_st.elapsed_secs, Some(7 * 3600 + 10));
        assert_eq!(later_st.remaining_secs, Some(3590));
        assert!(eng.is_active());
    }

    #[test]
    fn handoff_until_local_elapsed_survives_fresh_menu_clock() {
        let mut eng = Engine::new(cfg());
        let h = host(2_000);
        eng.handle(
            Input::Handoff {
                pref: DurationPref::UntilLocal { hour: 8, minute: 0 },
                remaining_secs: Some(90),
                elapsed_secs: Some(120),
            },
            &h,
        );
        let st = eng.json_status(&h);
        assert_eq!(st.elapsed_secs, Some(120));
        assert_eq!(st.remaining_secs, Some(90));
        let later = host(12_000);
        let later_st = eng.json_status(&later);
        assert_eq!(later_st.elapsed_secs, Some(130));
        assert_eq!(later_st.remaining_secs, Some(80));
    }

    #[test]
    fn user_present_does_not_resleep() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        eng.handle(Input::Tick, &host(1_500));
        let mut h = host(10_000);
        h.display_asleep = Some(false);
        h.hid_idle_ms = 1_000; // typing
        h.lid_closed = false;
        let e = eng.handle(Input::DisplayWoke, &h);
        assert!(!has_sleep(&e), "must not fight a person sitting at the Mac");
    }

    #[test]
    fn agent_wake_resleeps_when_user_away() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        eng.handle(Input::Tick, &host(1_500));
        let mut h = host(10_000);
        h.display_asleep = Some(false);
        h.hid_idle_ms = 120_000;
        let e = eng.handle(Input::DisplayWoke, &h);
        assert!(has_sleep(&e));
    }

    #[test]
    fn watchdog_resleeps_on_tick_without_wake_event() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        eng.handle(Input::Tick, &host(1_500));
        let mut h = host(1_500 + SLEEP_DISPLAY_DEBOUNCE_MS);
        h.display_asleep = Some(true);
        h.hid_idle_ms = 80_000;
        let e = eng.handle(Input::Tick, &h);
        assert!(
            has_sleep(&e),
            "away + debounce elapsed => reassert display sleep"
        );
    }

    #[test]
    fn lid_closed_always_resleeps() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        eng.handle(Input::Tick, &host(1_500));
        let mut h = host(10_000);
        h.display_asleep = Some(false);
        h.lid_closed = true;
        h.hid_idle_ms = 0; // even if HID looks busy
        let e = eng.handle(Input::DisplayWoke, &h);
        assert!(has_sleep(&e));
    }

    #[test]
    fn battery_floor_stops_on_battery() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        let mut h = host(5_000);
        h.on_ac = false;
        h.battery_percent = Some(19);
        let e = eng.handle(Input::Tick, &h);
        assert!(has_release(&e));
        assert!(!eng.is_active());
        let st = eng.json_status(&h);
        assert_eq!(st.stop_reason_code.as_deref(), Some("battery_floor"));
        assert_eq!(
            st.stop_reason.as_deref(),
            Some(StopReason::BatteryFloor.label_en())
        );
    }

    #[test]
    fn battery_floor_ignored_on_ac() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        let mut h = host(5_000);
        h.on_ac = true;
        h.battery_percent = Some(5);
        let e = eng.handle(Input::Tick, &h);
        assert!(!has_release(&e));
        assert!(eng.is_active());
    }

    #[test]
    fn first_sleep_can_lock_session() {
        let mut cfg = cfg();
        cfg.lock_screen = true;
        let mut eng = Engine::new(cfg);
        eng.handle(Input::Start, &host(0));
        let e = eng.handle(Input::Tick, &host(1_500));
        assert!(has_sleep(&e));
        assert!(e.iter().any(|x| matches!(x, Effect::LockSession)));
        let e = eng.handle(Input::Tick, &host(1_500 + SLEEP_DISPLAY_DEBOUNCE_MS));
        assert!(has_sleep(&e));
        assert!(!e.iter().any(|x| matches!(x, Effect::LockSession)));
    }

    #[test]
    fn duration_hours_elapses() {
        let mut cfg = cfg();
        cfg.duration = DurationPref::Hours { hours: 1 };
        let mut eng = Engine::new(cfg);
        let h0 = host(0);
        eng.handle(Input::Start, &h0);
        let mut h = host(3_600_000);
        h.unix_secs = h0.unix_secs + 3600;
        let e = eng.handle(Input::Tick, &h);
        assert!(has_release(&e));
    }

    #[test]
    fn duration_hours_ignores_wall_clock_jump() {
        let mut cfg = cfg();
        cfg.duration = DurationPref::Hours { hours: 1 };
        let mut eng = Engine::new(cfg);
        let h0 = host(0);
        eng.handle(Input::Start, &h0);
        let mut h = host(2_000);
        h.unix_secs = h0.unix_secs + 5;
        let e = eng.handle(Input::Tick, &h);
        assert!(
            !has_release(&e),
            "a truncated or stepped wall clock must not end a 1h session after 2s"
        );
        assert!(eng.is_active());
        assert_eq!(eng.json_status(&h).remaining_secs, Some(3_598));
        assert_eq!(eng.json_status(&h).elapsed_secs, Some(2));
    }

    #[test]
    fn duration_hours_ignores_large_ntp_forward_correction() {
        let mut cfg = cfg();
        cfg.duration = DurationPref::Hours { hours: 1 };
        let mut eng = Engine::new(cfg);
        let h0 = host(0);
        eng.handle(Input::Start, &h0);
        let mut h = host(60_000);
        h.unix_secs = h0.unix_secs + 3_600;
        let e = eng.handle(Input::Tick, &h);
        assert!(
            !has_release(&e),
            "a forward NTP/manual correction while the Mac stays awake must not end Hours"
        );
        assert!(eng.is_active());
        assert_eq!(eng.json_status(&h).remaining_secs, Some(3_540));
        assert_eq!(eng.json_status(&h).elapsed_secs, Some(60));
    }

    #[test]
    fn duration_hours_stops_when_monotonic_elapses() {
        let mut cfg = cfg();
        cfg.duration = DurationPref::Hours { hours: 1 };
        let mut eng = Engine::new(cfg);
        let h0 = host(0);
        eng.handle(Input::Start, &h0);
        let mut h = host(3_600_000);
        h.unix_secs = h0.unix_secs;
        let e = eng.handle(Input::Tick, &h);
        assert!(
            has_release(&e),
            "Hours sessions end when the suspend-aware clock elapses, even if unix_secs is stuck"
        );
    }

    #[test]
    fn duration_hours_stops_after_suspend_when_continuous_clock_elapses() {
        let mut cfg = cfg();
        cfg.duration = DurationPref::Hours { hours: 1 };
        let mut eng = Engine::new(cfg);
        let h0 = host(0);
        eng.handle(Input::Start, &h0);
        let mut h = host(2_000);
        h.monotonic_ms = 2_000;
        h.continuous_ms = 3_600_000;
        h.unix_secs = h0.unix_secs;
        let e = eng.handle(Input::Tick, &h);
        assert!(
            has_release(&e),
            "Instant does not run during system sleep; mach_continuous_time must still end Hours"
        );
    }

    #[test]
    fn hours_countdown_matches_suspend_aware_deadline() {
        let mut cfg = cfg();
        cfg.duration = DurationPref::Hours { hours: 1 };
        let mut eng = Engine::new(cfg);
        let h0 = host(0);
        eng.handle(Input::Start, &h0);
        let mut h = host(2_000);
        h.monotonic_ms = 2_000;
        h.continuous_ms = 1_800_000;
        h.unix_secs = h0.unix_secs + 1_800;
        let e = eng.handle(Input::Tick, &h);
        assert!(!has_release(&e));
        assert!(eng.is_active());
        assert_eq!(
            eng.json_status(&h).remaining_secs,
            Some(1_800),
            "30 minutes of suspend must leave 30 minutes on the countdown, not ~1h"
        );
        assert_eq!(eng.json_status(&h).elapsed_secs, Some(1_800));
        assert_eq!(eng.view(&h).remaining_secs, Some(1_800));
        assert_eq!(eng.view(&h).elapsed_secs, Some(1_800));
    }

    #[test]
    fn until_local_remaining_follows_unix_secs() {
        let mut cfg = cfg();
        cfg.duration = DurationPref::UntilLocal { hour: 8, minute: 0 };
        let mut eng = Engine::new(cfg);
        let h0 = host(0);
        eng.handle(Input::Start, &h0);
        let first = eng.json_status(&h0).remaining_secs.unwrap();
        let mut later = host(5_000);
        later.unix_secs = h0.unix_secs;
        assert_eq!(
            eng.json_status(&later).remaining_secs,
            Some(first),
            "UntilLocal remaining must not drop with monotonic time alone"
        );
        later.unix_secs = h0.unix_secs + 10;
        assert_eq!(eng.json_status(&later).remaining_secs, Some(first - 10));
    }

    #[test]
    fn no_screen_off_never_sleeps_display() {
        let mut cfg = cfg();
        cfg.screen_off = false;
        let mut eng = Engine::new(cfg);
        eng.handle(Input::Start, &host(0));
        let e = eng.handle(Input::Tick, &host(5_000));
        assert!(!has_sleep(&e));
    }

    #[test]
    fn clamshell_and_system_assertion_policy() {
        let mut eng = Engine::new(cfg());
        let e = eng.handle(Input::Start, &host(0));
        let plan = apply_plan(&e).unwrap();
        assert!(plan.disable_clamshell_sleep);
        assert!(plan.prevent_system_sleep);
        assert!(plan.network_client);

        let mut h = host(2_000);
        h.on_ac = false;
        h.display_asleep = Some(true);
        let e = eng.handle(Input::Tick, &h);
        let plan = apply_plan(&e).unwrap();
        assert!(plan.disable_clamshell_sleep);
        assert!(!plan.prevent_system_sleep);
    }

    #[test]
    fn thermal_critical_stops() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        let mut h = host(2_000);
        h.thermal = Thermal::Critical;
        let e = eng.handle(Input::Tick, &h);
        assert!(has_release(&e));
    }

    #[test]
    fn toggle_and_json() {
        let mut eng = Engine::new(cfg());
        let h = host(0);
        eng.handle(Input::Toggle, &h);
        assert!(eng.is_active());
        let st = eng.json_status(&h);
        assert!(st.active);
        eng.handle(Input::Toggle, &host(200));
        assert!(!eng.is_active());
    }

    #[test]
    fn view_model_primary_action() {
        let mut eng = Engine::new(cfg());
        let h = host(0);
        assert_eq!(eng.view(&h).primary_action, "Start Screen-Off Standby");
        eng.handle(Input::Start, &h);
        assert_eq!(eng.view(&h).primary_action, "End Standby");
    }

    #[test]
    fn view_model_chinese() {
        let mut cfg = cfg();
        cfg.language = Some(Lang::Zh);
        let mut eng = Engine::new(cfg);
        let h = host(0);
        assert_eq!(eng.view(&h).primary_action, "开始关屏待命");
        eng.handle(Input::Start, &h);
        assert_eq!(eng.view(&h).primary_action, "结束待命");
    }

    #[test]
    fn start_while_active_is_noop() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        let e = eng.handle(Input::Start, &host(100));
        assert!(e.is_empty());
        assert!(eng.is_active());
    }

    #[test]
    fn stop_while_idle_is_noop() {
        let mut eng = Engine::new(cfg());
        let e = eng.handle(
            Input::Stop {
                reason: StopReason::User,
            },
            &host(0),
        );
        assert!(e.is_empty());
    }

    #[test]
    fn start_with_overrides_duration() {
        let mut eng = Engine::new(cfg());
        let h = host(0);
        eng.handle(Input::StartWith(DurationPref::Hours { hours: 3 }), &h);
        let st = eng.json_status(&h);
        assert_eq!(st.remaining_secs, Some(3 * 3600));
        assert_eq!(eng.config.duration, DurationPref::Hours { hours: 3 });
    }

    #[test]
    fn resleep_disabled_does_not_reassert() {
        let mut cfg = cfg();
        cfg.resleep_display = false;
        let mut eng = Engine::new(cfg);
        eng.handle(Input::Start, &host(0));
        eng.handle(Input::Tick, &host(1_500));
        let mut h = host(1_500 + SLEEP_DISPLAY_DEBOUNCE_MS);
        h.display_asleep = Some(false);
        h.hid_idle_ms = 120_000;
        let e = eng.handle(Input::DisplayWoke, &h);
        assert!(!has_sleep(&e));
    }

    #[test]
    fn battery_floor_none_never_stops() {
        let mut cfg = cfg();
        cfg.battery_floor_percent = None;
        let mut eng = Engine::new(cfg);
        eng.handle(Input::Start, &host(0));
        let mut h = host(5_000);
        h.on_ac = false;
        h.battery_percent = Some(1);
        let e = eng.handle(Input::Tick, &h);
        assert!(!has_release(&e));
        assert!(eng.is_active());
    }

    #[test]
    fn user_stop_notifies_app_quit_does_not() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        let e = eng.handle(
            Input::Stop {
                reason: StopReason::User,
            },
            &host(200),
        );
        assert!(has_release(&e));
        assert!(e.iter().any(|x| matches!(x, Effect::Notify { .. })));

        eng.handle(Input::Start, &host(300));
        let e = eng.handle(
            Input::Stop {
                reason: StopReason::AppQuit,
            },
            &host(400),
        );
        assert!(has_release(&e));
        assert!(!e.iter().any(|x| matches!(x, Effect::Notify { .. })));
    }

    #[test]
    fn lid_awake_disabled_skips_clamshell_plan() {
        let mut cfg = cfg();
        cfg.keep_awake_on_lid_close = false;
        let mut eng = Engine::new(cfg);
        let e = eng.handle(Input::Start, &host(0));
        let plan = apply_plan(&e).unwrap();
        assert!(!plan.disable_clamshell_sleep);
        assert!(!plan.prevent_system_sleep);
        assert!(plan.prevent_idle_sleep);
    }

    #[test]
    fn json_status_fields_match_host() {
        let mut eng = Engine::new(cfg());
        let mut h = host(0);
        h.lid_closed = true;
        h.display_asleep = Some(true);
        eng.handle(Input::Start, &h);
        let st = eng.json_status(&h);
        assert_eq!(st.lid, "closed");
        assert_eq!(st.display, "asleep");
        assert!(!st.user_present);
        assert_eq!(st.elapsed_secs, Some(0));
        assert!(st.screen_off_enabled);
        assert!(st.lid_awake_enabled);
    }

    #[test]
    fn stop_reason_codes_roundtrip() {
        for reason in [
            StopReason::User,
            StopReason::BatteryFloor,
            StopReason::ThermalEmergency,
            StopReason::DurationElapsed,
            StopReason::AppQuit,
            StopReason::AssertionFailed,
        ] {
            assert_eq!(StopReason::from_code(reason.code()), Some(reason));
        }
        assert_eq!(StopReason::from_code("nope"), None);
        assert_eq!(StopReason::User.label_en(), "Ended by you");
    }

    #[test]
    fn view_warns_when_lid_awake_on_battery() {
        let mut eng = Engine::new(cfg());
        let mut h = host(0);
        h.on_ac = false;
        eng.handle(Input::Start, &h);
        let view = eng.view(&h);
        assert!(view
            .warnings
            .iter()
            .any(|w| w.contains("battery") || w.contains("电池")));
    }

    #[test]
    fn display_slept_is_optimistic_until_host_says_awake() {
        let mut cfg = cfg();
        cfg.resleep_display = false;
        let mut eng = Engine::new(cfg);
        let mut h = host(0);
        h.display_asleep = None;
        eng.handle(Input::Start, &h);
        eng.handle(Input::DisplaySlept, &h);
        let st = eng.json_status(&h);
        assert_eq!(st.display, "asleep");
    }

    #[test]
    fn sleep_display_now_emits_while_user_present() {
        let mut eng = Engine::new(cfg());
        eng.handle(Input::Start, &host(0));
        let mut h = host(200);
        h.display_asleep = Some(false);
        h.hid_idle_ms = 500;
        h.lid_closed = false;
        assert!(h.user_present(45_000));
        let e = eng.handle(Input::SleepDisplayNow, &h);
        assert!(has_sleep(&e), "an explicit tap must sleep the display now");
        assert!(eng.is_active());
        assert!(!has_release(&e));
    }

    #[test]
    fn sleep_display_now_works_without_starting_standby() {
        let mut eng = Engine::new(cfg());
        let e = eng.handle(Input::SleepDisplayNow, &host(0));
        assert!(has_sleep(&e));
        assert!(!has_release(&e));
        assert!(!eng.is_active());
    }

    #[test]
    fn sleep_display_now_does_not_end_session() {
        let mut cfg = cfg();
        cfg.screen_off = false;
        let mut eng = Engine::new(cfg);
        eng.handle(Input::Start, &host(0));
        let e = eng.handle(Input::SleepDisplayNow, &host(100));
        assert!(has_sleep(&e));
        assert!(eng.is_active(), "Sleep Display Now is not End Standby");
        assert!(!has_release(&e));
    }

    #[test]
    fn sleep_display_now_locks_on_first_off_when_configured() {
        let mut cfg = cfg();
        cfg.lock_screen = true;
        let mut eng = Engine::new(cfg);
        eng.handle(Input::Start, &host(0));
        let e = eng.handle(Input::SleepDisplayNow, &host(200));
        assert!(has_sleep(&e));
        assert!(e.iter().any(|x| matches!(x, Effect::LockSession)));
        let e = eng.handle(Input::SleepDisplayNow, &host(400));
        assert!(has_sleep(&e), "a second tap still reasserts display sleep");
        assert!(!e.iter().any(|x| matches!(x, Effect::LockSession)));
    }
}
