//! Menu-bar panel copy and navigation, kept free of AppKit so Linux CI can lock it.

use std::time::{Duration, Instant};

use never_sleep_core::{
    format_clock, format_countdown, AppConfig, DurationPref, Lang, Tr, ViewModel,
    DEFAULT_HOTKEY_LABEL, HEARTBEAT_MS,
};

/// Ignore a second Start/End click that AppKit queued from the same press.
pub const TOGGLE_COOLDOWN_MS: u64 = 400;
/// Status-item mouse-down can hide the panel (focus loss) before mouse-up toggles it.
pub const TRAY_REOPEN_GUARD_MS: u64 = 400;

/// Compact menu-bar popover. Height hugs the packed main/settings column.
pub const PANEL_WIDTH: f64 = 320.0;
pub const PANEL_HEIGHT: f64 = 423.0;
/// Two 12pt lid-on-battery lines plus the 3pt status-stack gap after the summary.
pub const WARNING_SLOT: f64 = 35.0;
pub const HERO_SIZE: f64 = 124.0;
pub const HERO_IMAGE: f64 = 104.0;
pub const CARD_RADIUS: f64 = 8.0;
pub const CONTENT_INSET: f64 = 16.0;
/// Screenshot `.row`: 32pt cell, 11pt left/right. Vertical padding lives inside the 32pt.
pub const CARD_ROW_HEIGHT: f64 = 32.0;
pub const CARD_ROW_INSET_X: f64 = 11.0;
/// Hairline between grouped-card rows. NSBox separators must be this tall, not the default 11pt.
pub const CARD_HAIRLINE: f64 = 1.0;
/// Space between the last menu block and 更多设置 / 退出. Not a stretch void.
pub const FOOTER_GAP: f64 = 8.0;
pub const PRIMARY_HEIGHT: f64 = 28.0;
/// Elapsed clock under the summary while standby is on.
pub const ELAPSED_HEIGHT: f64 = 22.0;
/// Gap between the Start/End pill and the reserved Sleep Display Now row.
pub const PRIMARY_CLUSTER_GAP: f64 = 8.0;
/// Settings chevron + title row.
pub const SHEET_HEAD_HEIGHT: f64 = 36.0;
/// Language segmented control; same height as the main Start/End push.
pub const LANGUAGE_HEIGHT: f64 = 28.0;
pub const FOOTER_HEIGHT: f64 = 36.0;
/// Regular `NSSwitch` column so captions wrap before the knob, not through it.
pub const SWITCH_COL: f64 = 51.0;
/// Gap between a settings/session caption and its control.
pub const CONTROL_ROW_GAP: f64 = 10.0;
/// Rounded panel chrome; matches the HTML-era `.panel` radius.
pub const PANEL_CORNER: f64 = 10.0;
/// Sides and bottom: room for the downward drop shadow to fade.
pub const SHADOW_INSET: f64 = 40.0;
/// Top gutter must stay small. `NSWindow` constrains the frame below the menu
/// bar, so a 40pt top inset becomes a 40pt hole under the status item.
pub const SHADOW_INSET_TOP: f64 = 8.0;
/// Logical gap between the status item and the visible card (not the shadow gutter).
pub const MENU_BAR_GAP: f64 = 4.0;
pub const SHADOW_RADIUS: f64 = 18.0;
/// Downward layer-shadow offset (Core Animation y-up, so the layer value is negative).
pub const SHADOW_OFFSET_Y: f64 = 6.0;
pub const SHADOW_OPACITY: f32 = 0.28;
/// Coin face swap duration (HTML-era `520ms` flip).
pub const HERO_FLIP_SECS: f64 = 0.52;
/// Idle `#f5f5f7` ↔ active `#1c1c1e` wash (HTML `420ms`).
pub const PANEL_COLOR_SECS: f64 = 0.42;
/// UI clock tick while a session is running. Idle keeps `HEARTBEAT_MS`.
/// The GUI should use `panel_clock_delay_ms` so wakes land on second boundaries.
pub const PANEL_TICK_ACTIVE_MS: u64 = 1_000;
pub const IDLE_FILL_RGB: [u8; 3] = [0xf5, 0xf5, 0xf7];
pub const ACTIVE_FILL_RGB: [u8; 3] = [0x1c, 0x1c, 0x1e];
/// Numbered badge / SF Symbol in How-to and Keep-in-mind rows.
pub const HELP_ROW_GLYPH: f64 = 22.0;
pub const HELP_ROW_GAP: f64 = 10.0;
pub const HELP_ROW_INSET: f64 = 12.0;
/// Vertical padding inside How-to / Keep-in-mind rows. Keeps copy off the hairline.
pub const HELP_ROW_PAD_Y: f64 = 16.0;
/// Title-to-detail gap inside a How-to step.
pub const HELP_COPY_GAP: f64 = 4.0;
/// Gap from a How-to / Keep-in-mind header to its grouped card.
pub const HELP_SECTION_GAP: f64 = 8.0;
/// Kicker to lead; lead to “Get started”.
pub const HELP_LEAD_GAP: f64 = 8.0;
/// How-to card to Keep-in-mind header.
pub const HELP_BLOCK_GAP: f64 = 16.0;
/// Scroll-document bottom inset so the last note clears the rounded clip.
pub const HELP_BODY_PAD_BOTTOM: f64 = 20.0;
/// Extra stack space around a grouped-card hairline. Screenshot rows sit on the line.
pub const CARD_SEPARATOR_GAP: f64 = 0.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelPlacement {
    /// Anchored under the status item; hide when the panel loses key focus.
    MenuBar,
}

pub fn panel_placement() -> PanelPlacement {
    PanelPlacement::MenuBar
}

pub fn dismiss_on_focus_loss() -> bool {
    matches!(panel_placement(), PanelPlacement::MenuBar)
}

pub fn window_width() -> f64 {
    PANEL_WIDTH + SHADOW_INSET * 2.0
}

pub fn window_height() -> f64 {
    panel_hug_height() + SHADOW_INSET_TOP + SHADOW_INSET
}

/// Convert a tray-icon physical coordinate into logical points.
pub fn physical_to_logical(px: f64, scale: f64) -> f64 {
    if scale <= 0.0 {
        px
    } else {
        px / scale
    }
}

/// Logical Y of the window's top-left so the **card** sits `MENU_BAR_GAP` below the tray.
pub fn panel_window_y(tray_y: f64, tray_height: f64) -> f64 {
    tray_y + tray_height + MENU_BAR_GAP - SHADOW_INSET_TOP
}

/// Packed main column: coin, status, elapsed clock, Start/End, reserved Sleep Now, chrome.
pub fn main_column_height() -> f64 {
    CONTENT_INSET * 2.0
        + HERO_SIZE
        + 12.0
        + 22.0
        + 3.0
        + 16.0
        + 3.0
        + ELAPSED_HEIGHT
        + WARNING_SLOT
        + 14.0
        + PRIMARY_HEIGHT
        + PRIMARY_CLUSTER_GAP
        + PRIMARY_HEIGHT
        + FOOTER_GAP
        + FOOTER_HEIGHT
}

/// Packed settings column: head, 8-row card (duration + six switches + pairing), language, chrome.
pub fn settings_column_height() -> f64 {
    CONTENT_INSET * 2.0
        + SHEET_HEAD_HEIGHT
        + 8.0
        + CARD_ROW_HEIGHT * 8.0
        + CARD_HAIRLINE * 7.0
        + 12.0
        + LANGUAGE_HEIGHT
        + FOOTER_GAP
        + FOOTER_HEIGHT
}

/// Window card height is the taller of main and settings so neither page clips.
pub fn panel_hug_height() -> f64 {
    main_column_height().max(settings_column_height())
}

pub fn panel_fill_rgb(active: bool) -> [u8; 3] {
    if active {
        ACTIVE_FILL_RGB
    } else {
        IDLE_FILL_RGB
    }
}

/// Idle shows the sun; standby shows the moon. Never both at once.
pub fn hero_shows_moon(active: bool) -> bool {
    active
}

/// Container Y-axis angle: sun (front) at 0, moon (back) after a half-turn.
pub fn hero_flip_radians(showing_moon: bool) -> f64 {
    if showing_moon {
        std::f64::consts::PI
    } else {
        0.0
    }
}

pub fn panel_inner_width() -> f64 {
    PANEL_WIDTH - CONTENT_INSET * 2.0
}

/// Wrapping width beside a 22pt glyph inside a grouped card.
pub fn grouped_copy_max_width() -> f64 {
    panel_inner_width() - HELP_ROW_INSET * 2.0 - HELP_ROW_GLYPH - HELP_ROW_GAP
}

/// Wrapping width beside an `NSSwitch` in a session/settings row.
pub fn switch_copy_max_width() -> f64 {
    panel_inner_width() - CARD_ROW_INSET_X * 2.0 - SWITCH_COL - CONTROL_ROW_GAP
}

/// Clip-view Y that shows the How-to-use kicker.
///
/// Flipped clip views have origin at the top. Unflipped views have origin at
/// the bottom, so `scrollToPoint(0, 0)` would land on Keep-in-mind instead.
pub fn help_scroll_y(document_height: f64, clip_height: f64, clip_flipped: bool) -> f64 {
    if clip_flipped {
        0.0
    } else {
        (document_height - clip_height).max(0.0)
    }
}

/// One wrapping How-to sentence: "按 ⌥⌘P，或点菜单…" / "Press ⌥⌘P or choose…".
pub fn join_help_step3(before: &str, hotkey: &str, after: &str) -> String {
    if after.starts_with('，') || after.starts_with(',') {
        format!("{before} {hotkey}{after}")
    } else {
        format!("{before} {hotkey} {after}")
    }
}

pub fn hero_flips(reduce_motion: bool) -> bool {
    !reduce_motion
}

pub fn motion_duration_secs(reduce_motion: bool, full_secs: f64) -> f64 {
    if reduce_motion {
        0.0
    } else {
        full_secs
    }
}

/// Event-loop wait so the session clock changes on a whole second, not a sliding 1s interval.
pub fn panel_clock_delay_ms(active: bool, remaining_ms: Option<u64>, elapsed_ms: u64) -> u64 {
    if !active {
        HEARTBEAT_MS
    } else if let Some(remaining) = remaining_ms {
        next_countdown_delay_ms(remaining)
    } else {
        next_elapsed_delay_ms(elapsed_ms)
    }
}

/// Milliseconds until a ceil-countdown digit should change.
pub fn next_countdown_delay_ms(remaining_ms: u64) -> u64 {
    match remaining_ms % 1_000 {
        0 => PANEL_TICK_ACTIVE_MS,
        phase => phase,
    }
}

/// Milliseconds until a floor-elapsed digit should change.
pub fn next_elapsed_delay_ms(elapsed_ms: u64) -> u64 {
    match elapsed_ms % 1_000 {
        0 => PANEL_TICK_ACTIVE_MS,
        phase => 1_000 - phase,
    }
}

/// True when only the session clock label changed, so AppKit can skip a full relayout.
pub fn panel_clock_only_changed(prev: &PanelState, next: &PanelState) -> bool {
    if prev.elapsed_clock == next.elapsed_clock {
        return false;
    }
    let mut stripped = next.clone();
    stripped.elapsed_clock = prev.elapsed_clock.clone();
    stripped == *prev
}

pub fn session_clock_label(vm: &ViewModel) -> String {
    if !vm.active {
        String::new()
    } else if let Some(remaining) = vm.remaining_secs {
        format_countdown(remaining)
    } else {
        format_clock(vm.elapsed_secs.unwrap_or(0))
    }
}

/// Event-loop wait while a session is running so the elapsed clock ticks every second.
/// Test helper: the GUI uses `panel_clock_delay_ms` so wakes land on second boundaries.
#[cfg(test)]
pub fn panel_tick_ms(active: bool) -> u64 {
    panel_clock_delay_ms(active, None, 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassKind {
    LiquidGlass,
    Vibrancy,
}

/// macOS 26+ has `NSGlassEffectView`; older releases use `NSVisualEffectView`.
pub fn preferred_glass(ns_glass_effect_view_available: bool) -> GlassKind {
    if ns_glass_effect_view_available {
        GlassKind::LiquidGlass
    } else {
        GlassKind::Vibrancy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelView {
    Main,
    Settings,
    Help,
}

/// Screenshot destinations. Coarse `PanelView` stays for Help / Settings reachability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarItem {
    Standby,
    Display,
    Help,
}

impl SidebarItem {
    #[cfg(test)]
    pub const ALL: [Self; 3] = [Self::Standby, Self::Display, Self::Help];

    #[cfg(test)]
    pub fn index(self) -> isize {
        match self {
            Self::Standby => 0,
            Self::Display => 1,
            Self::Help => 2,
        }
    }

    #[cfg(test)]
    pub fn from_index(index: isize) -> Option<Self> {
        Self::ALL.iter().copied().find(|item| item.index() == index)
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Standby => "moon.zzz",
            Self::Display => "display",
            Self::Help => "questionmark.circle",
        }
    }

    pub fn as_panel_view(self) -> PanelView {
        match self {
            Self::Standby => PanelView::Main,
            Self::Help => PanelView::Help,
            Self::Display => PanelView::Settings,
        }
    }
}

/// Help opened from Settings returns there; Help from Main or the menu returns to Main.
pub fn help_back_target(opened_from: PanelView) -> PanelView {
    match opened_from {
        PanelView::Settings => PanelView::Settings,
        PanelView::Main | PanelView::Help => PanelView::Main,
    }
}

/// Status-item / fallback menu Help always returns to Main, even if Settings was last shown.
pub fn menu_help_origin() -> PanelView {
    PanelView::Main
}

/// Origin stored after Help is opened.
///
/// In-panel How to use keeps the first origin while Help is already showing.
/// Status-item Help always records Main, including when the panel was dismissed
/// with How to use still current.
pub fn help_from_after_open(
    current: PanelView,
    previous: PanelView,
    origin: PanelView,
    from_menu: bool,
) -> PanelView {
    if from_menu || current != PanelView::Help {
        origin
    } else {
        previous
    }
}

/// True when a tray mouse-up should not reopen a panel that just hid from that same click.
pub fn suppress_tray_reopen(ms_since_focus_loss_hide: u64) -> bool {
    ms_since_focus_loss_hide < TRAY_REOPEN_GUARD_MS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationKey {
    Indefinite,
    Hours1,
    Hours3,
    Hours8,
    Until0800,
}

impl DurationKey {
    pub fn from_pref(pref: DurationPref) -> Self {
        match pref {
            DurationPref::Hours { hours: 1 } => Self::Hours1,
            DurationPref::Hours { hours: 3 } => Self::Hours3,
            DurationPref::Hours { hours: 8 } => Self::Hours8,
            DurationPref::UntilLocal { hour: 8, minute: 0 } => Self::Until0800,
            DurationPref::Indefinite
            | DurationPref::Hours { .. }
            | DurationPref::UntilLocal { .. } => Self::Indefinite,
        }
    }

    pub fn index(self) -> isize {
        match self {
            Self::Indefinite => 0,
            Self::Hours1 => 1,
            Self::Hours3 => 2,
            Self::Hours8 => 3,
            Self::Until0800 => 4,
        }
    }

    pub fn from_index(index: isize) -> Option<Self> {
        match index {
            0 => Some(Self::Indefinite),
            1 => Some(Self::Hours1),
            2 => Some(Self::Hours3),
            3 => Some(Self::Hours8),
            4 => Some(Self::Until0800),
            _ => None,
        }
    }

    pub fn as_ipc(self) -> &'static str {
        match self {
            Self::Indefinite => "indefinite",
            Self::Hours1 => "1h",
            Self::Hours3 => "3h",
            Self::Hours8 => "8h",
            Self::Until0800 => "until_0800",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelState {
    pub active: bool,
    pub lang: Lang,
    pub duration: DurationKey,
    pub warning: String,
    pub screen_off: bool,
    pub lid_awake: bool,
    pub resleep_display: bool,
    pub lock_screen: bool,
    pub battery_floor: bool,
    pub launch_at_login: bool,
    pub status_title: String,
    pub summary: String,
    pub primary_action: String,
    pub elapsed_clock: String,
    pub show_elapsed: bool,
    pub sleep_now_label: String,
    pub show_sleep_now: bool,
    pub duration_label: String,
    pub duration_indefinite: String,
    pub duration_1h: String,
    pub duration_3h: String,
    pub duration_8h: String,
    pub duration_until: String,
    pub resleep: String,
    pub battery: String,
    pub more_settings: String,
    pub section_session: String,
    pub section_display: String,
    pub section_lid: String,
    pub section_safeguards: String,
    pub section_general: String,
    pub language_label: String,
    pub hotkey_hint: String,
    pub settings: String,
    pub sidebar_options: String,
    pub sidebar_guide: String,
    pub pane_display_lead: String,
    pub pane_lid_lead: String,
    pub pane_safeguards_lead: String,
    pub pane_general_lead: String,
    pub back: String,
    pub screen_off_label: String,
    pub lid_awake_label: String,
    pub lock_screen_label: String,
    pub launch_at_login_label: String,
    pub help: String,
    pub quit: String,
    pub help_kicker: String,
    pub help_lead: String,
    pub help_how: String,
    pub help_step1_title: String,
    pub help_step1_detail: String,
    pub help_step2_title: String,
    pub help_step2_detail: String,
    pub help_step3_title: String,
    pub help_step3_before: String,
    pub help_hotkey: String,
    pub help_step3_after: String,
    pub help_step3: String,
    pub help_notes: String,
    pub help_note_lid: String,
    pub help_note_battery: String,
    pub help_note_quit: String,
    pub pairing_label: String,
    pub pairing_code: String,
    pub pairing_url: String,
}

impl PanelState {
    pub fn with_pairing(mut self, code: &str, url: &str) -> Self {
        let normalized = never_sleep_core::normalize_pairing_code(code)
            .unwrap_or_else(|| code.replace('-', "").to_ascii_uppercase());
        self.pairing_code = never_sleep_core::format_pairing_code(&normalized);
        self.pairing_url = url.to_string();
        self
    }
}

pub fn panel_state(cfg: &AppConfig, vm: &ViewModel) -> PanelState {
    let t = cfg.tr();
    let help_step3 = join_help_step3(
        t.help_step3_before(),
        DEFAULT_HOTKEY_LABEL,
        t.help_step3_after(),
    );
    PanelState {
        active: vm.active,
        lang: cfg.lang(),
        duration: DurationKey::from_pref(vm.duration),
        warning: vm.warnings.first().cloned().unwrap_or_default(),
        screen_off: vm.screen_off,
        lid_awake: vm.keep_awake_on_lid_close,
        resleep_display: vm.resleep_display,
        lock_screen: vm.lock_screen,
        battery_floor: cfg.battery_floor_percent.is_some(),
        launch_at_login: vm.launch_at_login,
        status_title: if vm.active {
            t.panel_active_title()
        } else {
            t.panel_idle_title()
        }
        .into(),
        summary: panel_summary(vm, t),
        primary_action: vm.primary_action.clone(),
        elapsed_clock: session_clock_label(vm),
        show_elapsed: vm.active,
        sleep_now_label: t.sleep_display_now_action().into(),
        show_sleep_now: vm.active,
        duration_label: t.duration_menu().into(),
        duration_indefinite: t.indefinite().into(),
        duration_1h: t.hours(1),
        duration_3h: t.hours(3),
        duration_8h: t.hours(8),
        duration_until: t.until_clock(8, 0),
        resleep: t.resleep_display().into(),
        battery: vm.battery_floor_label.clone(),
        more_settings: t.more_settings().into(),
        section_session: t.panel_section_session().into(),
        section_display: t.panel_section_display().into(),
        section_lid: t.panel_section_lid().into(),
        section_safeguards: t.panel_section_safeguards().into(),
        section_general: t.panel_section_general().into(),
        language_label: t.language_menu().into(),
        hotkey_hint: t.panel_hotkey_hint(),
        settings: t.settings_title().into(),
        sidebar_options: t.sidebar_group_options().into(),
        sidebar_guide: t.sidebar_group_guide().into(),
        pane_display_lead: t.pane_display_lead().into(),
        pane_lid_lead: t.warn_lid_best_effort().into(),
        pane_safeguards_lead: t.pane_safeguards_lead().into(),
        pane_general_lead: t.pane_general_lead().into(),
        back: t.back().into(),
        screen_off_label: t.screen_off_now().into(),
        lid_awake_label: t.lid_awake().into(),
        lock_screen_label: t.lock_screen().into(),
        launch_at_login_label: t.launch_at_login().into(),
        help: t.help_title().into(),
        quit: t.quit().into(),
        help_kicker: t.help_kicker().into(),
        help_lead: t.help_lead().into(),
        help_how: t.help_how().into(),
        help_step1_title: t.help_step1_title().into(),
        help_step1_detail: t.help_step1_detail().into(),
        help_step2_title: t.help_step2_title().into(),
        help_step2_detail: t.help_step2_detail().into(),
        help_step3_title: t.help_step3_title().into(),
        help_step3_before: t.help_step3_before().into(),
        help_hotkey: DEFAULT_HOTKEY_LABEL.into(),
        help_step3_after: t.help_step3_after().into(),
        help_step3,
        help_notes: t.help_notes().into(),
        help_note_lid: t.help_note_lid().into(),
        help_note_battery: t.help_note_battery().into(),
        help_note_quit: t.help_note_quit().into(),
        pairing_label: t.phone_board().into(),
        pairing_code: String::new(),
        pairing_url: String::new(),
    }
}

/// Panel subtitle follows display state, not merely `vm.active`.
pub fn panel_summary(vm: &ViewModel, t: Tr) -> String {
    if !vm.active {
        if vm.screen_off {
            t.panel_summary_idle().into()
        } else {
            t.will_keep_awake_only().into()
        }
    } else if vm.display_asleep {
        t.panel_summary_active().into()
    } else if vm.user_present {
        t.user_controls_display().into()
    } else if vm.screen_off {
        t.display_pending().into()
    } else {
        t.will_keep_awake_only().into()
    }
}

/// Ignores extra panel toggle clicks for a short cooldown.
///
/// AppKit can queue two `toggle:` actions from a double-click before the first
/// `refresh_ui` paints. The control stays enabled so End Standby is never grayed
/// out waiting for the heartbeat.
#[derive(Debug, Default)]
pub struct ToggleGate {
    locked_until: Option<Instant>,
}

impl ToggleGate {
    pub fn take_click(&mut self) -> bool {
        self.take_click_at(Instant::now())
    }

    pub fn take_click_at(&mut self, now: Instant) -> bool {
        if self.locked_until.is_some_and(|until| now < until) {
            false
        } else {
            self.locked_until = Some(now + Duration::from_millis(TOGGLE_COOLDOWN_MS));
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use never_sleep_core::{Engine, HostSnapshot, Thermal, Tr, DEFAULT_BATTERY_FLOOR};

    fn host() -> HostSnapshot {
        HostSnapshot {
            monotonic_ms: 5_000,
            continuous_ms: 5_000,
            unix_secs: 1_700_000_000,
            utc_offset_secs: 0,
            on_ac: true,
            battery_percent: Some(64),
            lid_closed: false,
            display_asleep: Some(false),
            hid_idle_ms: 1_000,
            thermal: Thermal::Nominal,
        }
    }

    #[test]
    fn shortcuts_and_pairing_keep_the_original_panel_height() {
        assert_eq!(panel_hug_height(), 423.0);
        let native = include_str!("native_panel.rs");
        assert!(native.contains("self.quick_card.setHidden(state.active)"));
        assert!(native.contains("TAG_SCREEN_OFF, mtm"));
        assert!(native.contains("show_pairing"));
        assert!(!native.contains("quick_resleep"));
    }

    #[test]
    fn preferred_glass_uses_liquid_glass_when_class_exists() {
        assert_eq!(preferred_glass(true), GlassKind::LiquidGlass);
        assert_eq!(preferred_glass(false), GlassKind::Vibrancy);
    }

    #[test]
    fn panel_views_are_main_settings_and_help() {
        assert_ne!(PanelView::Main, PanelView::Settings);
        assert_ne!(PanelView::Settings, PanelView::Help);
        assert_ne!(PanelView::Help, PanelView::Main);
    }

    #[test]
    fn sidebar_lists_standby_options_and_help() {
        assert_eq!(
            SidebarItem::ALL.len(),
            3,
            "screenshot panel has Main / Settings / Help, not a six-row sidebar"
        );
        assert_eq!(SidebarItem::Standby.as_panel_view(), PanelView::Main);
        assert_eq!(SidebarItem::Display.as_panel_view(), PanelView::Settings);
        assert_eq!(SidebarItem::Help.as_panel_view(), PanelView::Help);
        assert_eq!(SidebarItem::Standby.symbol(), "moon.zzz");
        assert_eq!(SidebarItem::from_index(2), Some(SidebarItem::Help));
        assert_eq!(SidebarItem::from_index(5), None);
        for item in SidebarItem::ALL {
            assert_eq!(SidebarItem::from_index(item.index()), Some(item));
        }
    }

    #[test]
    fn help_back_returns_to_the_view_that_opened_it() {
        assert_eq!(help_back_target(PanelView::Main), PanelView::Main);
        assert_eq!(help_back_target(PanelView::Settings), PanelView::Settings);
        assert_eq!(
            help_back_target(PanelView::Help),
            PanelView::Main,
            "opening Help while already on Help still backs to Main"
        );
        assert_eq!(
            help_back_target(menu_help_origin()),
            PanelView::Main,
            "status-item Help must not inherit a leftover Settings pane"
        );
    }

    #[test]
    fn menu_help_resets_origin_when_reopening_help() {
        let leftover = PanelView::Settings;
        assert_eq!(
            help_from_after_open(PanelView::Help, leftover, menu_help_origin(), true),
            PanelView::Main,
            "status-item Help after dismissing How to use must Back to Main, not leftover Settings"
        );
        assert_eq!(
            help_from_after_open(PanelView::Help, leftover, PanelView::Settings, false),
            PanelView::Settings,
            "in-panel How to use while already on Help keeps Settings as Back"
        );
        assert_eq!(
            help_from_after_open(PanelView::Settings, leftover, menu_help_origin(), true),
            PanelView::Main
        );
    }

    #[test]
    fn tray_click_after_focus_loss_does_not_reopen() {
        assert!(
            suppress_tray_reopen(0),
            "the mouse-up that dismissed the panel must not toggle it open again"
        );
        assert!(suppress_tray_reopen(TRAY_REOPEN_GUARD_MS - 1));
        assert!(
            !suppress_tray_reopen(TRAY_REOPEN_GUARD_MS),
            "a later status-item click must still open the panel"
        );
    }

    #[test]
    fn screenshot_panel_tokens_match_docs_shots() {
        assert_eq!(PANEL_WIDTH, 320.0);
        assert_eq!(
            PANEL_HEIGHT,
            panel_hug_height(),
            "the popover hugs the taller packed column"
        );
        assert_eq!(HERO_SIZE, 124.0);
        assert_eq!(HERO_IMAGE, 104.0);
        assert_eq!(CARD_RADIUS, 8.0);
        assert_eq!(CONTENT_INSET, 16.0);
        assert_eq!(PANEL_CORNER, 10.0);
        assert_eq!(SHADOW_INSET, 40.0);
        assert_eq!(SHADOW_INSET_TOP, 8.0);
        assert_eq!(MENU_BAR_GAP, 4.0);
        assert_eq!(
            SHADOW_INSET - SHADOW_INSET_TOP,
            32.0,
            "top gutter cannot be the full 40pt side inset; AppKit clamps that under the menu bar"
        );
        assert_eq!(SHADOW_RADIUS, 18.0);
        assert_eq!(SHADOW_OFFSET_Y, 6.0);
        assert_eq!(SHADOW_OPACITY, 0.28);
        assert_eq!(
            SHADOW_INSET - SHADOW_RADIUS - SHADOW_OFFSET_Y,
            16.0,
            "window padding must outlast the blur so the shadow can fade before the window edge"
        );
        assert_eq!(HERO_FLIP_SECS, 0.52);
        assert_eq!(PANEL_COLOR_SECS, 0.42);
        assert_eq!(ELAPSED_HEIGHT, 22.0);
        assert_eq!(PRIMARY_CLUSTER_GAP, 8.0);
        assert_eq!(SHEET_HEAD_HEIGHT, 36.0);
        assert_eq!(PANEL_TICK_ACTIVE_MS, 1_000);
        assert_eq!(panel_fill_rgb(false), [0xf5, 0xf5, 0xf7]);
        assert_eq!(panel_fill_rgb(true), [0x1c, 0x1c, 0x1e]);
        assert!(!hero_shows_moon(false));
        assert!(hero_shows_moon(true));
        assert_eq!(hero_flip_radians(false), 0.0);
        assert_eq!(hero_flip_radians(true), std::f64::consts::PI);
        assert!(
            grouped_copy_max_width() < panel_inner_width(),
            "help/settings row copy sits beside a glyph, not the full inner width"
        );
        assert!(
            grouped_copy_max_width() > 180.0,
            "Chinese How-to sentences must still have room to wrap: {}",
            grouped_copy_max_width()
        );
        assert_eq!(window_width(), PANEL_WIDTH + SHADOW_INSET * 2.0);
        assert_eq!(
            window_height(),
            PANEL_HEIGHT + SHADOW_INSET_TOP + SHADOW_INSET
        );
        assert!(
            window_width() > PANEL_WIDTH,
            "the window is larger than the card so the shadow can fade around the edges"
        );
    }

    #[test]
    fn panel_placement_anchors_under_the_menu_bar() {
        assert_eq!(panel_placement(), PanelPlacement::MenuBar);
        assert!(
            dismiss_on_focus_loss(),
            "a menu-bar popover must hide when it loses key focus"
        );
    }

    #[test]
    fn panel_window_y_puts_the_card_a_menu_gap_below_the_tray() {
        assert_eq!(physical_to_logical(48.0, 2.0), 24.0);
        assert_eq!(physical_to_logical(24.0, 1.0), 24.0);
        assert_eq!(physical_to_logical(24.0, 0.0), 24.0);
        let y = panel_window_y(0.0, 22.0);
        assert_eq!(y, 22.0 + MENU_BAR_GAP - SHADOW_INSET_TOP);
        assert_eq!(
            y + SHADOW_INSET_TOP,
            22.0 + MENU_BAR_GAP,
            "visible card top is the tray bottom plus 4pt, not plus the 40pt side gutter"
        );
        assert_ne!(
            SHADOW_INSET_TOP, SHADOW_INSET,
            "a 40pt top gutter is clamped below the menu bar and reads as a hole under the logo"
        );
    }

    #[test]
    fn grouped_menu_rows_hug_the_hairline() {
        assert_eq!(CARD_ROW_HEIGHT, 32.0);
        assert_eq!(CARD_ROW_INSET_X, 11.0);
        assert_eq!(CARD_HAIRLINE, 1.0);
        assert_eq!(FOOTER_GAP, 8.0);
        assert_eq!(
            CARD_SEPARATOR_GAP, 0.0,
            "screenshot rows sit on the 0.5pt hairline; extra stack gap doubles the menu spacing"
        );
        assert_eq!(
            HELP_ROW_PAD_Y, 16.0,
            "16pt spacers keep How-to copy off the hairline; edgeInsets were collapsing"
        );
        assert_eq!(HELP_COPY_GAP, 4.0);
        assert_eq!(HELP_SECTION_GAP, 8.0);
        assert_eq!(HELP_LEAD_GAP, 8.0);
        assert_eq!(HELP_BLOCK_GAP, 16.0);
        assert_eq!(HELP_BODY_PAD_BOTTOM, 20.0);
        assert_eq!(
            WARNING_SLOT, 35.0,
            "lid-on-battery warning is two 12pt lines plus status stack spacing"
        );
        assert_eq!(
            panel_hug_height(),
            PANEL_HEIGHT,
            "window height is the taller packed column, not a 480pt canvas with a stretch void"
        );
        assert_eq!(
            main_column_height(),
            CONTENT_INSET * 2.0
                + HERO_SIZE
                + 12.0
                + 22.0
                + 3.0
                + 16.0
                + 3.0
                + ELAPSED_HEIGHT
                + WARNING_SLOT
                + 14.0
                + PRIMARY_HEIGHT
                + PRIMARY_CLUSTER_GAP
                + PRIMARY_HEIGHT
                + FOOTER_GAP
                + FOOTER_HEIGHT,
            "main reserves the elapsed clock, End Standby, and Sleep Display Now"
        );
        assert_eq!(
            settings_column_height(),
            CONTENT_INSET * 2.0
                + SHEET_HEAD_HEIGHT
                + 8.0
                + CARD_ROW_HEIGHT * 8.0
                + CARD_HAIRLINE * 7.0
                + 12.0
                + LANGUAGE_HEIGHT
                + FOOTER_GAP
                + FOOTER_HEIGHT,
            "settings holds duration, six switches, and the phone pairing code"
        );
        assert_eq!(
            panel_hug_height(),
            settings_column_height(),
            "the pairing row makes Settings the taller page"
        );
        assert!(
            main_column_height() < panel_hug_height(),
            "main slack fills the extra under Sleep Display Now"
        );
    }

    #[test]
    fn panel_summary_follows_display_state() {
        let t = Tr::new(Lang::En);
        let mut cfg = AppConfig::default();
        let idle = Engine::new(cfg.clone()).view(&host());
        assert_eq!(
            panel_summary(&idle, t),
            t.panel_summary_idle(),
            "idle + screen_off promises the display will go off"
        );
        cfg.screen_off = false;
        let keep = Engine::new(cfg.clone()).view(&host());
        assert_eq!(
            panel_summary(&keep, t),
            t.will_keep_awake_only(),
            "idle without screen_off must not say the display is off"
        );
        let mut engine = Engine::new(AppConfig::default());
        let _ = engine.handle(never_sleep_core::Input::Start, &host());
        let present = engine.view(&host());
        assert_eq!(
            panel_summary(&present, t),
            t.user_controls_display(),
            "a present user keeps the panel; do not claim the display is already asleep"
        );
        let mut asleep = host();
        asleep.display_asleep = Some(true);
        asleep.hid_idle_ms = 80_000;
        let sleeping = engine.view(&asleep);
        assert_eq!(
            panel_summary(&sleeping, t),
            t.panel_summary_active(),
            "only an asleep display uses the asleep summary"
        );
    }

    #[test]
    fn help_copy_wraps_inside_the_grouped_card() {
        assert_eq!(HELP_ROW_GLYPH, 22.0);
        assert_eq!(HELP_ROW_GAP, 10.0);
        assert_eq!(HELP_ROW_INSET, 12.0);
        assert_eq!(HELP_ROW_PAD_Y, 16.0);
        assert_eq!(CARD_SEPARATOR_GAP, 0.0);
        assert_eq!(panel_inner_width(), 288.0);
        assert_eq!(grouped_copy_max_width(), 232.0);
        assert_eq!(
            join_help_step3("按", "⌥⌘P", "，或点菜单「结束待命」。"),
            "按 ⌥⌘P，或点菜单「结束待命」。"
        );
        assert_eq!(
            join_help_step3("Press", "⌥⌘P", "or choose “End Standby” in the menu."),
            "Press ⌥⌘P or choose “End Standby” in the menu."
        );
    }

    #[test]
    fn switch_captions_wrap_beside_the_knob() {
        assert_eq!(SWITCH_COL, 51.0);
        assert_eq!(CONTROL_ROW_GAP, 10.0);
        assert_eq!(LANGUAGE_HEIGHT, PRIMARY_HEIGHT);
        assert_eq!(switch_copy_max_width(), 205.0);
        assert!(
            switch_copy_max_width() < grouped_copy_max_width(),
            "settings rows have a switch column, not the 22pt How-to glyph"
        );
        let t_zh = Tr::new(Lang::Zh);
        let t_en = Tr::new(Lang::En);
        assert!(
            t_zh.lock_screen().contains("远程 GUI"),
            "keep the remote-GUI warning; wrap it instead of truncating"
        );
        assert!(
            t_en.lock_screen().contains("breaks remote GUI"),
            "English lock copy stays the full product sentence"
        );
        assert!(
            t_zh.lock_screen().chars().count() > 14,
            "the parenthetical cannot fit one 205pt line of 13pt Chinese"
        );
    }

    #[test]
    fn help_scroll_y_shows_the_kicker() {
        assert_eq!(help_scroll_y(600.0, 350.0, true), 0.0);
        assert_eq!(help_scroll_y(600.0, 350.0, false), 250.0);
        assert_eq!(
            help_scroll_y(200.0, 350.0, false),
            0.0,
            "a short document stays at the origin instead of scrolling negative"
        );
        assert_eq!(help_scroll_y(200.0, 350.0, true), 0.0);
    }

    #[test]
    fn standby_click_is_one_half_turn() {
        assert_eq!(hero_flip_radians(false), 0.0);
        assert_eq!(hero_flip_radians(true), std::f64::consts::PI);
        assert_eq!(
            hero_flip_radians(true) - hero_flip_radians(false),
            std::f64::consts::PI,
            "Start Screen-Off Standby rotates the coin once, 0→π, not a full spin"
        );
        assert_eq!(HERO_FLIP_SECS, 0.52);
        assert!(hero_flips(false));
    }

    #[test]
    fn hero_flip_and_color_respect_reduce_motion() {
        assert!(hero_flips(false));
        assert!(!hero_flips(true));
        assert_eq!(motion_duration_secs(false, HERO_FLIP_SECS), HERO_FLIP_SECS);
        assert_eq!(motion_duration_secs(true, HERO_FLIP_SECS), 0.0);
        assert_eq!(
            motion_duration_secs(false, PANEL_COLOR_SECS),
            PANEL_COLOR_SECS
        );
        assert_eq!(motion_duration_secs(true, PANEL_COLOR_SECS), 0.0);
    }

    #[test]
    fn panel_tick_is_one_second_while_active() {
        assert_eq!(panel_tick_ms(true), 1_000);
        assert_eq!(panel_tick_ms(false), HEARTBEAT_MS);
        assert!(
            panel_tick_ms(true) < panel_tick_ms(false),
            "the elapsed clock must tick faster than the idle power heartbeat"
        );
        assert_eq!(
            panel_clock_delay_ms(true, Some(3_599_250), 1_000),
            250,
            "countdown wakes when remaining_ms hits the next whole second"
        );
        assert_eq!(panel_clock_delay_ms(true, Some(3_599_000), 1_000), 1_000);
        assert_eq!(
            panel_clock_delay_ms(true, None, 1_250),
            750,
            "elapsed wakes at the next whole second of monotonic time"
        );
        assert_eq!(panel_clock_delay_ms(false, None, 0), HEARTBEAT_MS);
    }

    #[test]
    fn toggle_gate_ignores_clicks_until_cooldown() {
        let mut gate = ToggleGate::default();
        let t0 = Instant::now();
        assert!(
            gate.take_click_at(t0),
            "the first primary click must start standby"
        );
        assert!(
            !gate.take_click_at(t0 + Duration::from_millis(10)),
            "a queued double-click must not toggle standby back off"
        );
        assert!(!gate.take_click_at(t0 + Duration::from_millis(200)));
        assert!(
            gate.take_click_at(t0 + Duration::from_millis(TOGGLE_COOLDOWN_MS)),
            "End Standby must work after a short pause without waiting for heartbeat"
        );
        let mut live = ToggleGate::default();
        assert!(live.take_click());
        assert!(
            !live.take_click(),
            "the production take_click path must share the same cooldown"
        );
    }

    #[test]
    fn duration_key_roundtrip_covers_menu_presets() {
        let cases = [
            (
                DurationPref::Indefinite,
                DurationKey::Indefinite,
                "indefinite",
            ),
            (DurationPref::Hours { hours: 1 }, DurationKey::Hours1, "1h"),
            (DurationPref::Hours { hours: 3 }, DurationKey::Hours3, "3h"),
            (DurationPref::Hours { hours: 8 }, DurationKey::Hours8, "8h"),
            (
                DurationPref::UntilLocal { hour: 8, minute: 0 },
                DurationKey::Until0800,
                "until_0800",
            ),
        ];
        for (pref, key, ipc) in cases {
            assert_eq!(DurationKey::from_pref(pref), key);
            assert_eq!(key.as_ipc(), ipc);
            assert_eq!(DurationKey::from_index(key.index()), Some(key));
        }
        assert_eq!(DurationKey::from_index(9), None);
    }

    #[test]
    fn panel_state_keeps_help_settings_and_more() {
        let cfg = AppConfig::default();
        let engine = Engine::new(cfg.clone());
        let state = panel_state(&cfg, &engine.view(&host()));
        let t = Tr::new(Lang::En);
        assert_eq!(state.more_settings, t.more_settings());
        assert_eq!(state.settings, t.settings_title());
        assert_eq!(state.section_session, t.panel_section_session());
        assert_eq!(state.language_label, t.language_menu());
        assert!(state.hotkey_hint.contains(DEFAULT_HOTKEY_LABEL));
        assert_eq!(state.help, t.help_title());
        assert_eq!(state.back, t.back());
        assert_eq!(state.help_how, t.help_how());
        assert_eq!(state.help_note_lid, t.help_note_lid());
        assert!(state.lid_awake_label.contains("best effort"));
        assert!(state.help_note_lid.contains("power"));
        assert!(state.help_step3.contains(DEFAULT_HOTKEY_LABEL));
        assert_eq!(state.help_hotkey, DEFAULT_HOTKEY_LABEL);
        assert_eq!(state.help_step3_before, t.help_step3_before());
        assert_eq!(state.battery, t.battery_floor_on(DEFAULT_BATTERY_FLOOR));
        assert!(!state.active);
        assert_eq!(state.status_title, t.panel_idle_title());
        assert_eq!(state.summary, t.panel_summary_idle());
        assert_eq!(state.primary_action, t.start_standby());
        assert!(!state.show_elapsed);
        assert!(state.elapsed_clock.is_empty());
        assert_eq!(state.sleep_now_label, t.sleep_display_now_action());
        assert!(!state.show_sleep_now);
        assert_eq!(state.pairing_label, t.phone_board());
        assert!(state.pairing_code.is_empty());
    }

    #[test]
    fn pairing_row_surfaces_code_on_settings() {
        let cfg = AppConfig::default();
        let engine = Engine::new(cfg.clone());
        let state = panel_state(&cfg, &engine.view(&host())).with_pairing(
            "ab7k-2q9m",
            "https://xyz-ai.app/never-sleep/board/?code=AB7K-2Q9M",
        );
        assert_eq!(state.pairing_label, "Phone board");
        assert_eq!(state.pairing_code, "AB7K-2Q9M");
        assert!(state.pairing_url.contains("/board/"));
    }

    #[test]
    fn panel_state_follows_language_and_standby() {
        let cfg = AppConfig {
            language: Some(Lang::Zh),
            ..AppConfig::default()
        };
        let mut engine = Engine::new(cfg.clone());
        let _ = engine.handle(never_sleep_core::Input::Start, &host());
        let vm = engine.view(&host());
        let state = panel_state(&engine.config, &vm);
        let t = Tr::new(Lang::Zh);
        assert!(state.active);
        assert_eq!(state.lang, Lang::Zh);
        assert_eq!(state.status_title, t.panel_active_title());
        assert_eq!(state.summary, t.user_controls_display());
        assert_eq!(state.primary_action, t.end_standby());
        assert!(state.show_elapsed);
        assert_eq!(
            state.elapsed_clock,
            format_clock(vm.elapsed_secs.unwrap_or(0))
        );
        assert!(state.show_sleep_now);
        assert_eq!(state.sleep_now_label, "立即熄屏");
        assert_eq!(state.more_settings, "更多设置");
        assert_eq!(state.section_session, "待命");
        assert_eq!(state.section_display, "屏幕");
        assert_eq!(state.section_lid, "合盖");
        assert_eq!(state.section_safeguards, "保护");
        assert_eq!(state.section_general, "通用");
        assert_eq!(state.language_label, "语言");
        assert!(!state.help_note_lid.contains("熄屏待命"));
        assert!(
            state.help_step3.contains("⌥⌘P，或"),
            "Chinese step 3 must not break after the hotkey: {}",
            state.help_step3
        );
    }

    #[test]
    fn panel_state_uses_grouped_macos_sections() {
        let cfg = AppConfig::default();
        let engine = Engine::new(cfg.clone());
        let state = panel_state(&cfg, &engine.view(&host()));
        let t = Tr::new(Lang::En);
        assert_eq!(state.section_session, "Session");
        assert_eq!(state.section_display, "Display");
        assert_eq!(state.section_lid, "Lid");
        assert_eq!(state.section_safeguards, "Safeguards");
        assert_eq!(state.section_general, "General");
        assert_eq!(state.language_label, "Language");
        assert_eq!(state.more_settings, "More Settings");
        assert_eq!(state.sidebar_options, "Options");
        assert_eq!(state.sidebar_guide, "Guide");
        assert_eq!(state.pane_display_lead, t.pane_display_lead());
        assert_eq!(state.pane_general_lead, t.pane_general_lead());
        assert!(state.pane_lid_lead.contains("best-effort"));
        assert_eq!(state.hotkey_hint, t.panel_hotkey_hint());
        assert_eq!(state.primary_action, t.start_standby());
        assert!(!state.show_elapsed);
        assert!(!state.show_sleep_now);
        assert!(state.hotkey_hint.contains("display off"));
    }

    #[test]
    fn moon_panel_keeps_end_standby_and_shows_elapsed_clock() {
        let cfg = AppConfig::default();
        let mut engine = Engine::new(cfg.clone());
        let mut h = host();
        let _ = engine.handle(never_sleep_core::Input::Start, &h);
        h.monotonic_ms = 70_000;
        let vm = engine.view(&h);
        let state = panel_state(&engine.config, &vm);
        let t = Tr::new(Lang::En);
        assert_eq!(vm.elapsed_secs, Some(65));
        assert_eq!(state.elapsed_clock, "1:05");
        assert!(state.show_elapsed);
        assert_eq!(
            state.primary_action,
            t.end_standby(),
            "the Start/End pill stays End Standby; the clock is a separate label"
        );
        assert!(state.show_sleep_now);
        assert_eq!(state.sleep_now_label, t.sleep_display_now_action());
    }

    #[test]
    fn timed_session_shows_remaining_countdown() {
        let cfg = AppConfig {
            duration: DurationPref::Hours { hours: 1 },
            ..AppConfig::default()
        };
        let mut engine = Engine::new(cfg);
        let mut h = host();
        let _ = engine.handle(never_sleep_core::Input::Start, &h);
        let started = panel_state(&engine.config, &engine.view(&h));
        assert_eq!(started.elapsed_clock, "1:00:00");
        h.monotonic_ms += 5_000;
        h.continuous_ms += 5_000;
        h.unix_secs += 8;
        let later = panel_state(&engine.config, &engine.view(&h));
        assert_eq!(
            later.elapsed_clock, "0:59:55",
            "countdown follows the suspend-aware clock, not a jumped wall clock"
        );
        assert!(panel_clock_only_changed(&started, &later));
        assert!(!panel_clock_only_changed(&started, &started));
    }
}
