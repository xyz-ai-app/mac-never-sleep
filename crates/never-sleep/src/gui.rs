use std::sync::mpsc;
use std::time::{Duration, Instant};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use never_sleep_core::{
    AppConfig, DurationPref, Engine, Input, Lang, PowerPlan, StopReason, Tr, DEFAULT_BATTERY_FLOOR,
    DEFAULT_HOTKEY_LABEL, HEARTBEAT_MS,
};
use tao::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use tao::event::{ElementState, Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::keyboard::Key;
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS, WindowBuilderExtMacOS};
use tao::window::{Window, WindowBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{
    MouseButton, MouseButtonState, Rect as TrayRect, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use crate::apply::{dispatch, stop_for_quit};
use crate::cloud::{
    cloud_enabled, default_display_name, load_or_create_identity, spawn_reporter_paused,
    CloudHandle,
};
use crate::icon::tray_icon;
use crate::ipc::{self, IpcIncoming};
use crate::panel::{
    dismiss_on_focus_loss, panel_clock_delay_ms, panel_clock_only_changed, panel_placement,
    panel_state, panel_window_y, physical_to_logical, suppress_tray_reopen, window_height,
    window_width, PanelPlacement, PanelState, ToggleGate, PANEL_HEIGHT, PANEL_WIDTH,
};
use crate::persist::{load_config, save_config};
use crate::platform::{default_platform, Platform};
use crate::protocol::{IpcRequest, IpcResponse};

use crate::native_panel;

pub(crate) enum UserEvent {
    Hotkey,
    Menu(tray_icon::menu::MenuId),
    Tray(TrayRect),
    TrayAnchor(TrayRect),
    Ui(UiCommand),
}

#[derive(Debug)]
pub(crate) enum UiCommand {
    Toggle,
    SleepDisplayNow,
    SetDuration { value: String },
    SetOption { key: String, enabled: bool },
    SetLanguage { language: String },
    Help,
    More,
    PhoneBoard,
    Back,
    Quit,
}

struct Popover {
    window: Window,
    ui: native_panel::NativePanel,
    visible: bool,
    last_tray: Option<TrayRect>,
    last: Option<PanelState>,
    focus_loss_hide_at: Option<Instant>,
}

impl Popover {
    fn build(
        event_loop: &EventLoop<UserEvent>,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, String> {
        let window = WindowBuilder::new()
            .with_title("Never Sleep")
            .with_inner_size(LogicalSize::new(window_width(), window_height()))
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_visible(false)
            .with_always_on_top(true)
            .with_has_shadow(false)
            .with_movable_by_window_background(false)
            .build(event_loop)
            .map_err(|e| format!("panel window: {e}"))?;

        let ui = native_panel::NativePanel::attach(&window, proxy)?;

        Ok(Self {
            window,
            ui,
            visible: false,
            last_tray: None,
            last: None,
            focus_loss_hide_at: None,
        })
    }

    fn toggle_at(&mut self, rect: TrayRect) {
        self.last_tray = Some(rect);
        if self.visible {
            self.hide();
            return;
        }
        if let Some(at) = self.focus_loss_hide_at.take() {
            let ms = Instant::now().saturating_duration_since(at).as_millis() as u64;
            if suppress_tray_reopen(ms) {
                return;
            }
        }
        self.place_at(rect);
        self.window.set_visible(true);
        self.window.set_focus();
        self.visible = true;
    }

    fn show(&mut self) {
        match panel_placement() {
            PanelPlacement::MenuBar => {
                if let Some(rect) = self.last_tray {
                    self.place_at(rect);
                } else {
                    self.center_on_screen();
                }
            }
        }
        self.window.set_visible(true);
        self.window.set_focus();
        self.visible = true;
    }

    fn place_at(&self, rect: TrayRect) {
        let monitors: Vec<_> = self.window.available_monitors().collect();
        let monitor = monitors.iter().find(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            rect.position.x >= f64::from(position.x)
                && rect.position.x <= f64::from(position.x) + f64::from(size.width)
                && rect.position.y >= f64::from(position.y)
                && rect.position.y <= f64::from(position.y) + f64::from(size.height)
        });
        let scale = monitor
            .map(|m| m.scale_factor())
            .filter(|s| *s > 0.0)
            .unwrap_or_else(|| self.window.scale_factor().max(1.0));
        let tray_x = physical_to_logical(rect.position.x, scale);
        let tray_y = physical_to_logical(rect.position.y, scale);
        let tray_w = physical_to_logical(f64::from(rect.size.width), scale);
        let tray_h = physical_to_logical(f64::from(rect.size.height), scale);
        let width = window_width();
        let anchor_x = tray_x + tray_w / 2.0;
        let desired_x = anchor_x - width / 2.0;
        let x = monitor
            .map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                let min_x = physical_to_logical(f64::from(position.x), scale) + 8.0;
                let max_x = physical_to_logical(f64::from(position.x), scale)
                    + physical_to_logical(f64::from(size.width), scale)
                    - width
                    - 8.0;
                desired_x.clamp(min_x, max_x.max(min_x))
            })
            .unwrap_or(desired_x);
        let y = panel_window_y(tray_y, tray_h);
        self.window.set_outer_position(LogicalPosition::new(x, y));
    }

    fn center_on_screen(&self) {
        let Some(monitor) = self
            .window
            .primary_monitor()
            .or_else(|| self.window.current_monitor())
        else {
            return;
        };
        let scale = self.window.scale_factor();
        let width = PANEL_WIDTH * scale;
        let height = PANEL_HEIGHT * scale;
        let origin = monitor.position();
        let size = monitor.size();
        let x = f64::from(origin.x) + (f64::from(size.width) - width) / 2.0;
        let y = f64::from(origin.y) + (f64::from(size.height) - height) / 3.0;
        self.window
            .set_outer_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    }

    fn hide(&mut self) {
        self.window.set_visible(false);
        self.visible = false;
    }

    fn hide_from_focus_loss(&mut self) {
        if self.visible {
            self.focus_loss_hide_at = Some(Instant::now());
        }
        self.hide();
    }

    fn update(&mut self, state: PanelState) {
        if self.last.as_ref() == Some(&state) {
            return;
        }
        if self
            .last
            .as_ref()
            .is_some_and(|prev| panel_clock_only_changed(prev, &state))
        {
            self.ui.set_elapsed_clock(&state.elapsed_clock);
            self.last = Some(state);
            return;
        }
        self.ui.apply(&state);
        self.last = Some(state);
    }
}

struct MenuHandles {
    status: MenuItem,
    detail: MenuItem,
    warn: MenuItem,
    toggle: MenuItem,
    dur_inf: CheckMenuItem,
    dur_1h: CheckMenuItem,
    dur_3h: CheckMenuItem,
    dur_8h: CheckMenuItem,
    dur_until: CheckMenuItem,
    screen_off: CheckMenuItem,
    lid_awake: CheckMenuItem,
    resleep: CheckMenuItem,
    lock_screen: CheckMenuItem,
    battery_floor: CheckMenuItem,
    login: CheckMenuItem,
    lang_en: CheckMenuItem,
    lang_zh: CheckMenuItem,
    show_window: MenuItem,
    settings: MenuItem,
    help: MenuItem,
    quit: MenuItem,
    dur_root: Submenu,
    lang_root: Submenu,
}

pub fn run() {
    let mut platform = default_platform();
    platform.cleanup_orphans();
    let mut engine = Engine::new(load_config());

    let (ipc_tx, ipc_rx) = mpsc::channel::<IpcIncoming>();
    let ipc_owned = match ipc::spawn_server(ipc_tx) {
        Err(e) if e == "already_running" => {
            eprintln!("{}", load_config().tr().already_running());
            return;
        }
        Err(e) => {
            eprintln!("{}", load_config().tr().ipc_not_started(&e));
            false
        }
        Ok(()) => {
            crate::ipc::mark_ipc_server_owned(true);
            true
        }
    };

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let proxy_menu = event_loop.create_proxy();
    let proxy_hk = proxy_menu.clone();
    let proxy_tray = proxy_menu.clone();

    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = proxy_menu.send_event(UserEvent::Menu(event.id));
    }));
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state() == HotKeyState::Pressed {
            let _ = proxy_hk.send_event(UserEvent::Hotkey);
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            rect,
            button,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            if button == MouseButton::Left {
                let _ = proxy_tray.send_event(UserEvent::Tray(rect));
            } else {
                let _ = proxy_tray.send_event(UserEvent::TrayAnchor(rect));
            }
        }
    }));

    let hotkeys = GlobalHotKeyManager::new().ok();
    if let Some(ref mgr) = hotkeys {
        let hk = HotKey::new(Some(Modifiers::ALT | Modifiers::SUPER), Code::KeyP);
        if mgr.register(hk).is_err() {
            eprintln!("{}", load_config().tr().hotkey_failed(DEFAULT_HOTKEY_LABEL));
        }
    }
    let _hotkeys = hotkeys;

    let menu = Menu::new();
    let handles = build_menu(&menu, &engine.config);
    let mut popover = match Popover::build(&event_loop, event_loop.create_proxy()) {
        Ok(popover) => Some(popover),
        Err(error) => {
            eprintln!("{error}");
            None
        }
    };
    let mut tray: Option<TrayIcon> = None;
    let mut tray_active: Option<bool> = None;
    let mut next_wake = Instant::now() + Duration::from_millis(HEARTBEAT_MS);
    let mut shown_onboarding = engine.config.onboarding_done;
    let mut toggle_gate = ToggleGate::default();
    let mut pending_stop = false;
    let mut last_handoff_id: Option<String> = None;
    let mut pairing: Option<(String, String, u64)> = None;
    let cloud_identity = if cloud_enabled() {
        match load_or_create_identity() {
            Ok(id) => Some(id),
            Err(err) => {
                eprintln!("never-sleep cloud identity: {err}");
                None
            }
        }
    } else {
        None
    };
    let mut cloud = if ipc_owned {
        cloud_identity.as_ref().map(|identity| {
            spawn_reporter_paused(
                identity.clone(),
                default_display_name(),
                engine.config.lang(),
                crate::session_lock::should_pause_menu_reporter(
                    crate::session_lock::peer_reporter_lock_is_live(std::process::id()),
                ),
            )
        })
    } else {
        None
    };

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(next_wake);

        while let Ok(incoming) = ipc_rx.try_recv() {
            let IpcIncoming::Request { req, .. } = &incoming;
            let handoff_first = req.is_handoff() && !engine.is_active();
            let toggle_want_stop = crate::session_lock::local_toggle_wants_stop(engine.is_active());
            if req.is_handoff() {
                if let Some(handle) = cloud.as_ref() {
                    handle.skip_applied(req.applied_command_ids().to_vec());
                }
            }
            if !handoff_first {
                if let Some(handle) = cloud.as_ref() {
                    crate::cloud::apply_polled_commands(
                        &mut engine,
                        platform.as_mut(),
                        handle,
                        &mut pairing,
                    );
                }
            }
            let (quitting, adopted, stop_donor, skip_drain) = handle_ipc(
                &mut engine,
                platform.as_mut(),
                incoming,
                &mut pairing,
                cloud_identity.as_ref(),
                cloud.as_ref().is_some_and(CloudHandle::reporter_is_running),
                &mut pending_stop,
                &mut last_handoff_id,
                toggle_want_stop,
            );
            if quitting {
                flush_cloud_on_quit(
                    &engine,
                    platform.as_mut(),
                    &mut cloud,
                    last_handoff_id.as_deref(),
                );
                *control_flow = ControlFlow::Exit;
                break;
            } else {
                if handoff_first
                    && !crate::protocol::should_skip_handoff_drain_after_ack_failure(skip_drain)
                {
                    if let Some(handle) = cloud.as_ref() {
                        crate::cloud::apply_polled_commands(
                            &mut engine,
                            platform.as_mut(),
                            handle,
                            &mut pairing,
                        );
                    }
                }
                if crate::session_lock::take_pending_stop_after_handoff(
                    adopted,
                    stop_donor,
                    &mut pending_stop,
                ) {
                    dispatch(
                        &mut engine,
                        platform.as_mut(),
                        Input::Stop {
                            reason: StopReason::User,
                        },
                    );
                }
                refresh_ui(
                    &handles,
                    &mut tray,
                    &mut tray_active,
                    &mut popover,
                    &mut engine,
                    platform.as_mut(),
                    &mut next_wake,
                    cloud.as_ref(),
                    &mut pairing,
                );
            }
        }
        if matches!(*control_flow, ControlFlow::Exit) {
            return;
        }

        match event {
            Event::NewEvents(StartCause::Init) => {
                let icon = tray_icon(false);
                tray = TrayIconBuilder::new()
                    .with_menu(Box::new(menu.clone()))
                    .with_menu_on_left_click(popover.is_none())
                    .with_tooltip(engine.config.tr().app_display_name())
                    .with_icon(icon)
                    .with_icon_as_template(true)
                    .build()
                    .ok();
                if !shown_onboarding {
                    shown_onboarding = true;
                    engine.config.onboarding_done = true;
                    save_config(&engine.config);
                    let t = engine.config.tr();
                    show_dialog(&t, t.welcome_title(), t.onboarding());
                }
                refresh_ui(
                    &handles,
                    &mut tray,
                    &mut tray_active,
                    &mut popover,
                    &mut engine,
                    platform.as_mut(),
                    &mut next_wake,
                    cloud.as_ref(),
                    &mut pairing,
                );
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                dispatch(&mut engine, platform.as_mut(), Input::Tick);
                if crate::session_lock::take_pending_donor_clamshell_reapply(engine.is_active()) {
                    let host = platform.snapshot();
                    let plan = PowerPlan::for_session(&engine.config, host.on_ac);
                    let _ = platform.apply_power(plan);
                }
                refresh_ui(
                    &handles,
                    &mut tray,
                    &mut tray_active,
                    &mut popover,
                    &mut engine,
                    platform.as_mut(),
                    &mut next_wake,
                    cloud.as_ref(),
                    &mut pairing,
                );
            }
            Event::UserEvent(UserEvent::Menu(id)) => {
                let toggle_want_stop =
                    crate::session_lock::local_toggle_wants_stop(engine.is_active());
                if let Some(handle) = cloud.as_ref() {
                    crate::cloud::apply_polled_commands(
                        &mut engine,
                        platform.as_mut(),
                        handle,
                        &mut pairing,
                    );
                }
                handle_menu_event(
                    &mut engine,
                    platform.as_mut(),
                    &handles,
                    control_flow,
                    id,
                    popover.as_mut(),
                    &mut pending_stop,
                    toggle_want_stop,
                );
                if matches!(*control_flow, ControlFlow::Exit) {
                    flush_cloud_on_quit(
                        &engine,
                        platform.as_mut(),
                        &mut cloud,
                        last_handoff_id.as_deref(),
                    );
                } else {
                    refresh_ui(
                        &handles,
                        &mut tray,
                        &mut tray_active,
                        &mut popover,
                        &mut engine,
                        platform.as_mut(),
                        &mut next_wake,
                        cloud.as_ref(),
                        &mut pairing,
                    );
                }
            }
            Event::UserEvent(UserEvent::Hotkey) => {
                let toggle_want_stop =
                    crate::session_lock::local_toggle_wants_stop(engine.is_active());
                if let Some(handle) = cloud.as_ref() {
                    crate::cloud::apply_polled_commands(
                        &mut engine,
                        platform.as_mut(),
                        handle,
                        &mut pairing,
                    );
                }
                dispatch_local_toggle(
                    &mut engine,
                    platform.as_mut(),
                    &mut pending_stop,
                    toggle_want_stop,
                );
                refresh_ui(
                    &handles,
                    &mut tray,
                    &mut tray_active,
                    &mut popover,
                    &mut engine,
                    platform.as_mut(),
                    &mut next_wake,
                    cloud.as_ref(),
                    &mut pairing,
                );
            }
            Event::UserEvent(UserEvent::Tray(rect)) => {
                refresh_ui(
                    &handles,
                    &mut tray,
                    &mut tray_active,
                    &mut popover,
                    &mut engine,
                    platform.as_mut(),
                    &mut next_wake,
                    cloud.as_ref(),
                    &mut pairing,
                );
                if let Some(panel) = popover.as_mut() {
                    panel.toggle_at(rect);
                }
            }
            Event::UserEvent(UserEvent::TrayAnchor(rect)) => {
                if let Some(panel) = popover.as_mut() {
                    panel.last_tray = Some(rect);
                }
            }
            Event::UserEvent(UserEvent::Ui(command)) => {
                let toggle_want_stop =
                    crate::session_lock::local_toggle_wants_stop(engine.is_active());
                if let Some(handle) = cloud.as_ref() {
                    crate::cloud::apply_polled_commands(
                        &mut engine,
                        platform.as_mut(),
                        handle,
                        &mut pairing,
                    );
                }
                handle_ui_command(
                    command,
                    &mut engine,
                    platform.as_mut(),
                    &handles,
                    popover.as_mut(),
                    &mut toggle_gate,
                    control_flow,
                    &mut pending_stop,
                    toggle_want_stop,
                );
                if matches!(*control_flow, ControlFlow::Exit) {
                    flush_cloud_on_quit(
                        &engine,
                        platform.as_mut(),
                        &mut cloud,
                        last_handoff_id.as_deref(),
                    );
                } else {
                    refresh_ui(
                        &handles,
                        &mut tray,
                        &mut tray_active,
                        &mut popover,
                        &mut engine,
                        platform.as_mut(),
                        &mut next_wake,
                        cloud.as_ref(),
                        &mut pairing,
                    );
                }
            }
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if let Some(popover) = popover.as_mut() {
                    if popover.window.id() == window_id {
                        popover.hide();
                    }
                }
            }
            Event::WindowEvent {
                window_id,
                event: WindowEvent::Focused(false),
                ..
            } => {
                if dismiss_on_focus_loss() {
                    if let Some(popover) = popover.as_mut() {
                        if popover.window.id() == window_id {
                            popover.hide_from_focus_loss();
                        }
                    }
                }
            }
            Event::WindowEvent {
                window_id,
                event: WindowEvent::KeyboardInput { event, .. },
                ..
            } => {
                if event.state == ElementState::Pressed {
                    if let Some(panel) = popover.as_mut() {
                        if panel.window.id() == window_id && event.logical_key == Key::Escape {
                            panel.hide();
                        }
                    }
                }
            }
            Event::LoopDestroyed => {
                stop_for_quit(&mut engine, platform.as_mut());
                flush_cloud_on_quit(
                    &engine,
                    platform.as_mut(),
                    &mut cloud,
                    last_handoff_id.as_deref(),
                );
            }
            _ => {}
        }
    });
}

fn build_menu(menu: &Menu, cfg: &AppConfig) -> MenuHandles {
    let t = cfg.tr();
    let status = MenuItem::new(t.idle_status(), false, None);
    let detail = MenuItem::new(t.will_sleep_display(), false, None);
    let warn = MenuItem::new(" ", false, None);
    let toggle = MenuItem::new(t.start_standby(), true, None);

    let dur_inf = CheckMenuItem::new(
        t.indefinite(),
        true,
        matches!(cfg.duration, DurationPref::Indefinite),
        None,
    );
    let dur_1h = CheckMenuItem::new(
        t.hours(1),
        true,
        matches!(cfg.duration, DurationPref::Hours { hours: 1 }),
        None,
    );
    let dur_3h = CheckMenuItem::new(
        t.hours(3),
        true,
        matches!(cfg.duration, DurationPref::Hours { hours: 3 }),
        None,
    );
    let dur_8h = CheckMenuItem::new(
        t.hours(8),
        true,
        matches!(cfg.duration, DurationPref::Hours { hours: 8 }),
        None,
    );
    let dur_until = CheckMenuItem::new(
        t.until_clock(8, 0),
        true,
        matches!(
            cfg.duration,
            DurationPref::UntilLocal { hour: 8, minute: 0 }
        ),
        None,
    );
    let dur_root = Submenu::with_items(
        t.duration_menu(),
        true,
        &[&dur_inf, &dur_1h, &dur_3h, &dur_8h, &dur_until],
    )
    .expect("duration submenu");

    let screen_off = CheckMenuItem::new(t.screen_off_now(), true, cfg.screen_off, None);
    let lid_awake = CheckMenuItem::new(t.lid_awake(), true, cfg.keep_awake_on_lid_close, None);
    let resleep = CheckMenuItem::new(t.resleep_display(), true, cfg.resleep_display, None);
    let lock_screen = CheckMenuItem::new(t.lock_screen(), true, cfg.lock_screen, None);
    let battery_floor = CheckMenuItem::new(
        t.battery_floor_on(DEFAULT_BATTERY_FLOOR),
        true,
        cfg.battery_floor_percent.is_some(),
        None,
    );
    let login = CheckMenuItem::new(t.launch_at_login(), true, cfg.launch_at_login, None);
    let lang_en = CheckMenuItem::new(t.language_english(), true, cfg.lang() == Lang::En, None);
    let lang_zh = CheckMenuItem::new(t.language_chinese(), true, cfg.lang() == Lang::Zh, None);
    let lang_root = Submenu::with_items(t.language_menu(), true, &[&lang_en, &lang_zh])
        .expect("language submenu");
    let show_window = MenuItem::new(t.show_window(), true, None);
    let settings = MenuItem::new(t.settings_title(), true, None);
    let help = MenuItem::new(t.help_title(), true, None);
    let quit = MenuItem::new(t.quit(), true, None);

    let _ = menu.append_items(&[
        &status,
        &detail,
        &warn,
        &PredefinedMenuItem::separator(),
        &toggle,
        &show_window,
        &PredefinedMenuItem::separator(),
        &dur_root,
        &PredefinedMenuItem::separator(),
        &screen_off,
        &lid_awake,
        &resleep,
        &lock_screen,
        &battery_floor,
        &PredefinedMenuItem::separator(),
        &login,
        &lang_root,
        &settings,
        &help,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);

    MenuHandles {
        status,
        detail,
        warn,
        toggle,
        dur_inf,
        dur_1h,
        dur_3h,
        dur_8h,
        dur_until,
        screen_off,
        lid_awake,
        resleep,
        lock_screen,
        battery_floor,
        login,
        lang_en,
        lang_zh,
        show_window,
        settings,
        help,
        quit,
        dur_root,
        lang_root,
    }
}

fn apply_static_labels(handles: &MenuHandles, lang: Lang) {
    let t = Tr::new(lang);
    handles.dur_inf.set_text(t.indefinite());
    handles.dur_1h.set_text(t.hours(1));
    handles.dur_3h.set_text(t.hours(3));
    handles.dur_8h.set_text(t.hours(8));
    handles.dur_until.set_text(t.until_clock(8, 0));
    handles.dur_root.set_text(t.duration_menu());
    handles.screen_off.set_text(t.screen_off_now());
    handles.lid_awake.set_text(t.lid_awake());
    handles.resleep.set_text(t.resleep_display());
    handles.lock_screen.set_text(t.lock_screen());
    handles
        .battery_floor
        .set_text(t.battery_floor_on(DEFAULT_BATTERY_FLOOR));
    handles.login.set_text(t.launch_at_login());
    handles.lang_root.set_text(t.language_menu());
    handles.show_window.set_text(t.show_window());
    handles.settings.set_text(t.settings_title());
    handles.help.set_text(t.help_title());
    handles.quit.set_text(t.quit());
}

#[allow(clippy::too_many_arguments)]
fn refresh_ui(
    handles: &MenuHandles,
    tray: &mut Option<TrayIcon>,
    tray_active: &mut Option<bool>,
    popover: &mut Option<Popover>,
    engine: &mut Engine,
    platform: &mut dyn Platform,
    next_wake: &mut Instant,
    cloud: Option<&CloudHandle>,
    pairing: &mut Option<(String, String, u64)>,
) {
    if let Some(handle) = cloud {
        crate::cloud::sync_cloud(engine, platform, handle, pairing);
        if crate::session_lock::should_resume_paused_menu_reporter(
            handle.is_paused(),
            crate::session_lock::peer_reporter_lock_is_live(std::process::id()),
            engine.is_active(),
        ) {
            handle.resume();
        }
    }
    crate::cloud::expire_stale_pairing(pairing);
    let host = platform.snapshot();
    let vm = engine.view(&host);
    let next = popover.as_ref().map(|_| {
        let mut state = panel_state(&engine.config, &vm);
        if let Some((code, url, _)) = pairing.as_ref() {
            state = state.with_pairing(code, url);
        }
        state
    });
    handles.status.set_text(vm.status_line);
    handles.detail.set_text(vm.detail_line);
    let warn = vm.warnings.first().cloned().unwrap_or_default();
    handles
        .warn
        .set_text(if warn.is_empty() { " " } else { &warn });
    handles.toggle.set_text(vm.primary_action);
    handles.screen_off.set_checked(vm.screen_off);
    handles.lid_awake.set_checked(vm.keep_awake_on_lid_close);
    handles.resleep.set_checked(vm.resleep_display);
    handles.lock_screen.set_checked(vm.lock_screen);
    handles.login.set_checked(vm.launch_at_login);
    handles
        .battery_floor
        .set_checked(engine.config.battery_floor_percent.is_some());
    handles
        .lang_en
        .set_checked(engine.config.lang() == Lang::En);
    handles
        .lang_zh
        .set_checked(engine.config.lang() == Lang::Zh);
    handles
        .dur_inf
        .set_checked(matches!(vm.duration, DurationPref::Indefinite));
    handles
        .dur_1h
        .set_checked(matches!(vm.duration, DurationPref::Hours { hours: 1 }));
    handles
        .dur_3h
        .set_checked(matches!(vm.duration, DurationPref::Hours { hours: 3 }));
    handles
        .dur_8h
        .set_checked(matches!(vm.duration, DurationPref::Hours { hours: 8 }));
    handles.dur_until.set_checked(matches!(
        vm.duration,
        DurationPref::UntilLocal { hour: 8, minute: 0 }
    ));
    if let Some(t) = tray.as_mut() {
        t.set_tooltip(Some(vm.tooltip)).ok();
        // set_icon() creates a fresh NSImage without isTemplate. Re-apply the
        // template flag so macOS can tint the glyph white on a dark menu bar.
        if *tray_active != Some(vm.active) {
            t.set_icon(Some(tray_icon(vm.active))).ok();
            t.set_icon_as_template(true);
            *tray_active = Some(vm.active);
        }
    }
    if let (Some(panel), Some(state)) = (popover.as_mut(), next) {
        panel.update(state);
    }
    let delay = match engine.session_times(&host) {
        Some((elapsed, remaining)) => panel_clock_delay_ms(true, remaining, elapsed),
        None => HEARTBEAT_MS,
    };
    *next_wake = Instant::now() + Duration::from_millis(delay);
}

fn flush_cloud_on_quit(
    engine: &Engine,
    platform: &mut dyn Platform,
    cloud: &mut Option<CloudHandle>,
    last_handoff_id: Option<&str>,
) {
    let ack = crate::protocol::read_handoff_ack();
    let had_live_reporter = crate::protocol::successor_reporter_live_on_matching_ack(
        last_handoff_id,
        ack.as_ref().map(|ack| ack.id.as_str()),
        ack.as_ref().is_some_and(|ack| ack.reporter),
    );
    let clear_ok = crate::protocol::mark_handoff_ack_reporter_gone();
    if let Some(handle) = cloud.take() {
        let would_detach = crate::session_lock::should_detach_cloud_on_quit(
            engine.is_active(),
            std::process::id(),
        );
        if would_detach
            && !crate::protocol::should_flush_offline_if_ack_reporter_clear_failed(
                clear_ok,
                would_detach,
                had_live_reporter,
            )
            && !crate::protocol::should_flush_offline_after_abandoning_successor_reporter(
                had_live_reporter,
                clear_ok,
            )
        {
            handle.detach();
        } else {
            crate::cloud::publish_and_flush(
                handle,
                engine.json_status(&platform.snapshot()),
                engine.config.lang(),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_menu_event(
    engine: &mut Engine,
    platform: &mut dyn Platform,
    handles: &MenuHandles,
    control_flow: &mut ControlFlow,
    id: tray_icon::menu::MenuId,
    popover: Option<&mut Popover>,
    pending_stop: &mut bool,
    toggle_want_stop: bool,
) {
    if id == handles.toggle.id() {
        dispatch_local_toggle(engine, platform, pending_stop, toggle_want_stop);
    } else if id == handles.quit.id() {
        stop_for_quit(engine, platform);
        *control_flow = ControlFlow::Exit;
    } else if id == handles.show_window.id() {
        if let Some(panel) = popover {
            panel.show();
        }
    } else if id == handles.settings.id() {
        if let Some(panel) = popover {
            panel.show();
            panel.ui.show_settings();
        }
    } else if id == handles.help.id() {
        show_menu_help(engine, popover);
    } else if id == handles.lang_en.id() {
        engine.config.language = Some(Lang::En);
        save_config(&engine.config);
        apply_static_labels(handles, Lang::En);
    } else if id == handles.lang_zh.id() {
        engine.config.language = Some(Lang::Zh);
        save_config(&engine.config);
        apply_static_labels(handles, Lang::Zh);
    } else if id == handles.screen_off.id() {
        engine.config.screen_off = !engine.config.screen_off;
        save_config(&engine.config);
    } else if id == handles.lid_awake.id() {
        engine.config.keep_awake_on_lid_close = !engine.config.keep_awake_on_lid_close;
        save_config(&engine.config);
        if engine.is_active() {
            dispatch(engine, platform, Input::Tick);
        }
    } else if id == handles.resleep.id() {
        engine.config.resleep_display = !engine.config.resleep_display;
        save_config(&engine.config);
    } else if id == handles.lock_screen.id() {
        engine.config.lock_screen = !engine.config.lock_screen;
        save_config(&engine.config);
    } else if id == handles.battery_floor.id() {
        engine.config.battery_floor_percent = if engine.config.battery_floor_percent.is_some() {
            None
        } else {
            Some(DEFAULT_BATTERY_FLOOR)
        };
        save_config(&engine.config);
    } else if id == handles.login.id() {
        engine.config.launch_at_login = !engine.config.launch_at_login;
        if let Err(e) = platform.set_launch_at_login(engine.config.launch_at_login) {
            platform.notify(engine.config.tr().login_item_title(), &e);
            engine.config.launch_at_login = !engine.config.launch_at_login;
        }
        save_config(&engine.config);
    } else if id == handles.dur_inf.id() {
        set_duration(engine, platform, DurationPref::Indefinite);
    } else if id == handles.dur_1h.id() {
        set_duration(engine, platform, DurationPref::Hours { hours: 1 });
    } else if id == handles.dur_3h.id() {
        set_duration(engine, platform, DurationPref::Hours { hours: 3 });
    } else if id == handles.dur_8h.id() {
        set_duration(engine, platform, DurationPref::Hours { hours: 8 });
    } else if id == handles.dur_until.id() {
        set_duration(
            engine,
            platform,
            DurationPref::UntilLocal { hour: 8, minute: 0 },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_ui_command(
    command: UiCommand,
    engine: &mut Engine,
    platform: &mut dyn Platform,
    handles: &MenuHandles,
    popover: Option<&mut Popover>,
    toggle_gate: &mut ToggleGate,
    control_flow: &mut ControlFlow,
    pending_stop: &mut bool,
    toggle_want_stop: bool,
) {
    match command {
        UiCommand::Toggle => {
            if !toggle_gate.take_click() {
                return;
            }
            dispatch_local_toggle(engine, platform, pending_stop, toggle_want_stop);
        }
        UiCommand::SleepDisplayNow => {
            dispatch(engine, platform, Input::SleepDisplayNow);
        }
        UiCommand::SetDuration { value } => {
            let pref = match value.as_str() {
                "indefinite" => Some(DurationPref::Indefinite),
                "1h" => Some(DurationPref::Hours { hours: 1 }),
                "3h" => Some(DurationPref::Hours { hours: 3 }),
                "8h" => Some(DurationPref::Hours { hours: 8 }),
                "until_0800" => Some(DurationPref::UntilLocal { hour: 8, minute: 0 }),
                _ => None,
            };
            if let Some(pref) = pref {
                set_duration(engine, platform, pref);
            }
        }
        UiCommand::SetOption { key, enabled } => {
            match key.as_str() {
                "screen_off" => engine.config.screen_off = enabled,
                "lid_awake" => engine.config.keep_awake_on_lid_close = enabled,
                "resleep_display" => engine.config.resleep_display = enabled,
                "lock_screen" => engine.config.lock_screen = enabled,
                "battery_floor" => {
                    engine.config.battery_floor_percent = enabled.then_some(DEFAULT_BATTERY_FLOOR)
                }
                "launch_at_login" => {
                    if let Err(error) = platform.set_launch_at_login(enabled) {
                        platform.notify(engine.config.tr().login_item_title(), &error);
                    } else {
                        engine.config.launch_at_login = enabled;
                    }
                }
                _ => return,
            }
            save_config(&engine.config);
            if key == "lid_awake" && engine.is_active() {
                dispatch(engine, platform, Input::Tick);
            }
        }
        UiCommand::SetLanguage { language } => {
            let lang = match language.as_str() {
                "en" => Some(Lang::En),
                "zh" => Some(Lang::Zh),
                _ => None,
            };
            if let Some(lang) = lang {
                engine.config.language = Some(lang);
                save_config(&engine.config);
                apply_static_labels(handles, lang);
            }
        }
        UiCommand::Help => {
            if let Some(panel) = popover {
                panel.ui.show_help();
            }
        }
        UiCommand::More => {
            if let Some(panel) = popover {
                panel.ui.show_settings();
            }
        }
        UiCommand::PhoneBoard => {
            if let Some(panel) = popover {
                panel.ui.show_pairing();
            }
        }
        UiCommand::Back => {
            if let Some(panel) = popover {
                panel.ui.go_back();
            }
        }
        UiCommand::Quit => {
            stop_for_quit(engine, platform);
            *control_flow = ControlFlow::Exit;
        }
    }
}

fn set_duration(engine: &mut Engine, platform: &mut dyn Platform, pref: DurationPref) {
    engine.config.duration = pref;
    save_config(&engine.config);
    if engine.is_active() {
        dispatch(
            engine,
            platform,
            Input::Stop {
                reason: StopReason::User,
            },
        );
        dispatch(engine, platform, Input::StartWith(pref));
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_ipc(
    engine: &mut Engine,
    platform: &mut dyn Platform,
    incoming: IpcIncoming,
    pairing: &mut Option<(String, String, u64)>,
    identity: Option<&never_sleep_core::CloudIdentity>,
    reporter_running: bool,
    pending_stop: &mut bool,
    last_handoff_id: &mut Option<String>,
    toggle_want_stop: bool,
) -> (bool, bool, bool, bool) {
    crate::cloud::expire_stale_pairing(pairing);
    let IpcIncoming::Request { req, reply } = incoming;
    let host_status = |engine: &Engine, platform: &mut dyn Platform| {
        let host = platform.snapshot();
        engine.json_status(&host)
    };
    let mut quitting = false;
    let mut adopted = false;
    let mut handoff_attempt = false;
    let mut prior_handoff = false;
    let mut ack_id: Option<String> = None;
    let mut dispatched_now = false;
    let mut resp = match req {
        IpcRequest::Ping => IpcResponse::pong(),
        IpcRequest::Status => IpcResponse::ok_status(host_status(engine, platform)),
        IpcRequest::Pair => match pairing.as_ref() {
            Some((code, url, _)) => IpcResponse::ok_pairing(
                code.clone(),
                url.clone(),
                identity.map(|id| id.device_id.clone()),
            ),
            None => IpcResponse::err("pairing_unavailable"),
        },
        IpcRequest::On {
            duration,
            remaining_secs,
            elapsed_secs,
            handoff,
            handoff_id,
            ..
        } => {
            let parsed = match crate::protocol::parse_on_duration_in(
                duration.as_deref(),
                engine.config.lang(),
            ) {
                Ok(d) => d,
                Err(e) => {
                    let _ = reply.send(IpcResponse::err(e));
                    return (false, false, false, false);
                }
            };
            let input = if handoff {
                Input::Handoff {
                    pref: parsed.unwrap_or(engine.config.duration),
                    remaining_secs,
                    elapsed_secs,
                }
            } else {
                match parsed {
                    None => Input::Start,
                    Some(d) => Input::StartWith(d),
                }
            };
            handoff_attempt = handoff;
            ack_id = handoff_id.clone();
            if handoff {
                prior_handoff = crate::protocol::menu_already_processed_handoff(
                    handoff,
                    handoff_id.as_deref(),
                    last_handoff_id.as_deref(),
                );
                if crate::protocol::menu_confirms_prior_handoff(
                    handoff,
                    engine.is_active(),
                    handoff_id.as_deref(),
                    last_handoff_id.as_deref(),
                ) {
                    adopted = true;
                } else if !prior_handoff && !engine.is_active() {
                    dispatch(engine, platform, input);
                    dispatched_now = true;
                    adopted = engine.is_active();
                    if adopted {
                        *last_handoff_id = handoff_id.clone();
                    }
                }
            } else if local_controls_deferred(engine) {
                // live donor still owns standby; do not start a second session
            } else if engine.is_active() && matches!(input, Input::Start) {
                // already on
            } else if engine.is_active() {
                dispatch(
                    engine,
                    platform,
                    Input::Stop {
                        reason: StopReason::User,
                    },
                );
                dispatch(engine, platform, input);
            } else {
                dispatch(engine, platform, input);
            }
            if adopted {
                let mut resp = IpcResponse::ok_adopted(host_status(engine, platform));
                resp.reporter = Some(reporter_running);
                resp
            } else {
                IpcResponse::ok_status(host_status(engine, platform))
            }
        }
        IpcRequest::Off => {
            if engine.is_active() {
                dispatch(
                    engine,
                    platform,
                    Input::Stop {
                        reason: StopReason::User,
                    },
                );
            } else if crate::session_lock::should_record_deferred_off(
                engine.is_active(),
                local_controls_deferred(engine),
            ) {
                crate::session_lock::note_deferred_escape(true, pending_stop);
            }
            IpcResponse::ok_status(host_status(engine, platform))
        }
        IpcRequest::Toggle => {
            dispatch_local_toggle(engine, platform, pending_stop, toggle_want_stop);
            IpcResponse::ok_status(host_status(engine, platform))
        }
        IpcRequest::Quit => {
            stop_for_quit(engine, platform);
            quitting = true;
            IpcResponse::ok_status(host_status(engine, platform))
        }
    };
    if crate::session_lock::should_stop_donor_on_failed_handoff(
        handoff_attempt,
        adopted,
        *pending_stop,
    ) || crate::protocol::should_stop_donor_after_ended_prior_handoff(
        prior_handoff,
        engine.is_active(),
    ) {
        resp.stop_donor = true;
    }
    let mut stop_donor = resp.stop_donor;
    let mut skip_drain = false;
    if crate::protocol::should_persist_handoff_ack(handoff_attempt, adopted, stop_donor) {
        let outcome = if adopted {
            crate::protocol::HandoffAckOutcome::Adopted
        } else {
            crate::protocol::HandoffAckOutcome::Stop
        };
        let reporter = crate::protocol::handoff_ack_reporter(reporter_running);
        let persist_ok = ack_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(|id| crate::protocol::write_handoff_ack(id, outcome, reporter).is_ok())
            .unwrap_or(false);
        let reject_adopt = crate::protocol::should_reject_adopt_if_ack_unpersisted(
            adopted,
            dispatched_now,
            persist_ok,
        );
        let reject_stop =
            crate::protocol::should_reject_stop_if_ack_unpersisted(stop_donor, persist_ok);
        if crate::protocol::should_skip_handoff_drain_after_ack_failure(reject_adopt || reject_stop)
        {
            if reject_adopt {
                let restore_ok =
                    if crate::session_lock::should_restore_donor_lock_on_adopt_rollback(true) {
                        ack_id
                            .as_deref()
                            .and_then(crate::protocol::parse_handoff_owner)
                            .map(|owner| {
                                crate::session_lock::restore_donor_session_lock(
                                    owner.pid,
                                    owner.starttime,
                                    owner.clamshell.unwrap_or(false),
                                )
                            })
                            .unwrap_or(false)
                    } else {
                        false
                    };
                if crate::session_lock::should_rollback_adopt_after_donor_lock_restore(
                    true, restore_ok,
                ) {
                    dispatch(
                        engine,
                        platform,
                        Input::Stop {
                            reason: StopReason::AppQuit,
                        },
                    );
                    *last_handoff_id = None;
                    adopted = false;
                }
            }
            let keep_stop = crate::protocol::should_keep_unpersisted_stop_donor_after_kept_adopt(
                reject_stop,
                last_handoff_id.is_some(),
                adopted,
            );
            if !keep_stop {
                stop_donor = false;
            }
            skip_drain = true;
            resp = IpcResponse::err("handoff_ack_failed");
            if keep_stop {
                resp.stop_donor = true;
            }
        }
    }
    resp.clamshell_reapply = crate::session_lock::should_signal_ipc_donor_clamshell_reapply(
        handoff_attempt,
        crate::session_lock::peek_ipc_donor_clamshell_reapply(),
    );
    let _ = reply.send(resp);
    (quitting, adopted, stop_donor, skip_drain)
}

fn local_controls_deferred(engine: &Engine) -> bool {
    crate::session_lock::should_defer_local_controls(engine.is_active(), std::process::id())
}

fn dispatch_local_toggle(
    engine: &mut Engine,
    platform: &mut dyn Platform,
    pending_stop: &mut bool,
    want_stop: bool,
) {
    let deferred = local_controls_deferred(engine);
    crate::session_lock::note_deferred_escape(deferred, pending_stop);
    if deferred {
        return;
    }
    match crate::session_lock::local_toggle_after_cloud_drain(want_stop, engine.is_active()) {
        Some(true) => {
            dispatch(
                engine,
                platform,
                Input::Stop {
                    reason: StopReason::User,
                },
            );
        }
        Some(false) => {
            dispatch(engine, platform, Input::Toggle);
        }
        None => {}
    }
}

fn show_menu_help(engine: &Engine, popover: Option<&mut Popover>) {
    if let Some(panel) = popover {
        panel.show();
        panel.ui.show_help_from_menu();
        return;
    }
    let t = engine.config.tr();
    show_dialog(&t, t.help_title(), t.onboarding());
}

fn show_dialog(t: &Tr, title: &str, body: &str) {
    let ok = t.dialog_ok().replace('"', "\\\"");
    let script = format!(
        "display dialog \"{}\" with title \"{}\" with icon note buttons {{\"{ok}\"}} default button 1",
        body.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n"),
        title.replace('"', "\\\"")
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status();
}
