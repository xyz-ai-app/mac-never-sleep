//! Native AppKit panel matching `docs/screenshots`: coin, Start pill, three sheets.

use std::cell::{Cell, RefCell};
use std::ops::Deref;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{define_class, msg_send, sel, AllocAnyThread, ClassType, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua,
    NSAppearanceNameDarkAqua, NSAutoresizingMaskOptions, NSBezelStyle, NSBorderType, NSBox,
    NSBoxType, NSButton, NSCellImagePosition, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSGlassEffectView, NSGlassEffectViewStyle, NSImage, NSImageView,
    NSLayoutAttribute, NSLayoutConstraintOrientation, NSPopUpButton, NSScrollView,
    NSSegmentSwitchTracking, NSSegmentedControl, NSStackView, NSStackViewDistribution, NSSwitch,
    NSTextAlignment, NSTextField, NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow,
};
use objc2_core_graphics::CGColor;
use objc2_foundation::{
    MainThreadMarker, NSData, NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSString,
    NSUserDefaults,
};
use objc2_quartz_core::{CALayer, CATransaction, CATransform3D};
use tao::event_loop::EventLoopProxy;
use tao::platform::macos::WindowExtMacOS;
use tao::window::Window;

use crate::gui::{UiCommand, UserEvent};
use crate::panel::{
    grouped_copy_max_width, help_back_target, help_from_after_open, help_scroll_y,
    hero_flip_radians, hero_flips, hero_shows_moon, menu_help_origin, motion_duration_secs,
    panel_fill_rgb, panel_inner_width, preferred_glass, switch_copy_max_width, DurationKey,
    GlassKind, PanelState, PanelView, SidebarItem, CARD_HAIRLINE, CARD_RADIUS, CARD_ROW_HEIGHT,
    CARD_ROW_INSET_X, CARD_SEPARATOR_GAP, CONTENT_INSET, CONTROL_ROW_GAP, ELAPSED_HEIGHT,
    FOOTER_GAP, FOOTER_HEIGHT, HELP_BLOCK_GAP, HELP_BODY_PAD_BOTTOM, HELP_COPY_GAP, HELP_LEAD_GAP,
    HELP_ROW_GAP, HELP_ROW_GLYPH, HELP_ROW_INSET, HELP_ROW_PAD_Y, HELP_SECTION_GAP, HERO_FLIP_SECS,
    HERO_IMAGE, HERO_SIZE, IDLE_FILL_RGB, LANGUAGE_HEIGHT, PANEL_COLOR_SECS, PANEL_CORNER,
    PRIMARY_CLUSTER_GAP, PRIMARY_HEIGHT, SHADOW_INSET, SHADOW_INSET_TOP, SHADOW_OFFSET_Y,
    SHADOW_OPACITY, SHADOW_RADIUS, SHEET_HEAD_HEIGHT, WARNING_SLOT,
};

const TAG_RESLEEP: isize = 1;
const TAG_BATTERY: isize = 2;
const TAG_SCREEN_OFF: isize = 3;
const TAG_LID: isize = 4;
const TAG_LOCK: isize = 5;
const TAG_LOGIN: isize = 6;

struct PanelIvars {
    proxy: EventLoopProxy<UserEvent>,
    pairing_code: RefCell<String>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "NeverSleepPanelTarget"]
    #[ivars = PanelIvars]
    struct PanelTarget;

    unsafe impl NSObjectProtocol for PanelTarget {}

    impl PanelTarget {
        #[unsafe(method(copyPairing:))]
        fn copy_pairing(&self, sender: Option<&NSButton>) {
            let code = self.ivars().pairing_code.borrow();
            if code.is_empty() { return; }
            let pasteboard = objc2_app_kit::NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            let ok = unsafe { pasteboard.setString_forType(&ns(&code), objc2_app_kit::NSPasteboardTypeString) };
            if ok {
                if let Some(button) = sender { button.setTitle(&ns(if button.title().to_string().contains("复制") { "已复制 ✓" } else { "Copied ✓" })); }
            }
        }

        #[unsafe(method(toggle:))]
        fn toggle(&self, _sender: Option<&AnyObject>) {
            self.emit(UiCommand::Toggle);
        }

        #[unsafe(method(sleepNow:))]
        fn sleep_now(&self, _sender: Option<&AnyObject>) {
            self.emit(UiCommand::SleepDisplayNow);
        }

        #[unsafe(method(phoneBoard:))]
        fn phone_board(&self, _sender: Option<&AnyObject>) { self.emit(UiCommand::PhoneBoard); }

        #[unsafe(method(more:))]
        fn more(&self, _sender: Option<&AnyObject>) {
            self.emit(UiCommand::More);
        }

        #[unsafe(method(back:))]
        fn back(&self, _sender: Option<&AnyObject>) {
            self.emit(UiCommand::Back);
        }

        #[unsafe(method(help:))]
        fn help(&self, _sender: Option<&AnyObject>) {
            self.emit(UiCommand::Help);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            self.emit(UiCommand::Quit);
        }

        #[unsafe(method(durationChanged:))]
        fn duration_changed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
            if let Some(key) = DurationKey::from_index(index) {
                self.emit(UiCommand::SetDuration {
                    value: key.as_ipc().into(),
                });
            }
        }

        #[unsafe(method(optionChanged:))]
        fn option_changed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            let tag: isize = unsafe { msg_send![sender, tag] };
            let state: isize = unsafe { msg_send![sender, state] };
            let key = match tag {
                TAG_RESLEEP => "resleep_display",
                TAG_BATTERY => "battery_floor",
                TAG_SCREEN_OFF => "screen_off",
                TAG_LID => "lid_awake",
                TAG_LOCK => "lock_screen",
                TAG_LOGIN => "launch_at_login",
                _ => return,
            };
            self.emit(UiCommand::SetOption {
                key: key.into(),
                enabled: state != 0,
            });
        }

        #[unsafe(method(languageChanged:))]
        fn language_changed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            let index: isize = unsafe { msg_send![sender, selectedSegment] };
            let language = if index == 1 { "zh" } else { "en" };
            self.emit(UiCommand::SetLanguage {
                language: language.into(),
            });
        }
    }
);

impl PanelTarget {
    fn new(mtm: MainThreadMarker, proxy: EventLoopProxy<UserEvent>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PanelIvars {
            proxy,
            pairing_code: RefCell::new(String::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn emit(&self, command: UiCommand) {
        let _ = self.ivars().proxy.send_event(UserEvent::Ui(command));
    }
}

struct FlipDoneIvars {
    sun: Retained<CALayer>,
    moon: Retained<CALayer>,
    showing_moon: Cell<bool>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "NeverSleepCoinFlipDone"]
    #[ivars = FlipDoneIvars]
    struct CoinFlipDone;

    unsafe impl NSObjectProtocol for CoinFlipDone {}

    impl CoinFlipDone {
        #[unsafe(method(finish))]
        fn finish(&self) {
            rest_coin_faces(
                &self.ivars().sun,
                &self.ivars().moon,
                self.ivars().showing_moon.get(),
            );
        }
    }
);

impl CoinFlipDone {
    fn new(
        sun: Retained<CALayer>,
        moon: Retained<CALayer>,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FlipDoneIvars {
            sun,
            moon,
            showing_moon: Cell::new(false),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn schedule(&self, showing_moon: bool, delay: f64) {
        self.cancel();
        self.ivars().showing_moon.set(showing_moon);
        if delay <= 0.0 {
            rest_coin_faces(&self.ivars().sun, &self.ivars().moon, showing_moon);
            return;
        }
        unsafe {
            let _: () = msg_send![
                self,
                performSelector: sel!(finish),
                withObject: None::<&AnyObject>,
                afterDelay: delay
            ];
        }
    }

    fn cancel(&self) {
        unsafe {
            let _: () = msg_send![NSObject::class(), cancelPreviousPerformRequestsWithTarget: self];
        }
    }
}

pub struct NativePanel {
    _target: Retained<PanelTarget>,
    wash: Retained<NSView>,
    clip: Retained<NSView>,
    coin: Retained<NSView>,
    rotator: Retained<CALayer>,
    sun_face: Retained<CALayer>,
    moon_face: Retained<CALayer>,
    flip_done: Retained<CoinFlipDone>,
    main_view: Retained<NSView>,
    settings_view: Retained<NSView>,
    help_view: Retained<NSView>,
    last_active: Option<bool>,
    status_title: Retained<NSTextField>,
    summary: Retained<NSTextField>,
    elapsed: Retained<NSTextField>,
    warning: Retained<NSTextField>,
    primary: Retained<NSButton>,
    sleep_now: Retained<NSButton>,
    duration: Retained<NSPopUpButton>,
    duration_label: Retained<NSTextField>,
    more: Retained<NSButton>,
    quit_main: Retained<NSButton>,
    back: Retained<NSButton>,
    settings_title: Retained<NSTextField>,
    screen_off_label: Retained<NSTextField>,
    screen_off: Retained<NSSwitch>,
    lid_label: Retained<NSTextField>,
    lid: Retained<NSSwitch>,
    resleep_settings_label: Retained<NSTextField>,
    resleep_settings: Retained<NSSwitch>,
    lock_label: Retained<NSTextField>,
    lock: Retained<NSSwitch>,
    battery_settings_label: Retained<NSTextField>,
    battery_settings: Retained<NSSwitch>,
    login_label: Retained<NSTextField>,
    login: Retained<NSSwitch>,
    quick_duration: Retained<NSPopUpButton>,
    quick_duration_label: Retained<NSTextField>,
    quick_screen: Retained<NSSwitch>,
    quick_screen_label: Retained<NSTextField>,
    quick_card: Retained<NSView>,
    status_stack: Retained<NSStackView>,
    sleep_host: Retained<NSView>,
    phone_main: Retained<NSButton>,
    phone_settings: Retained<NSButton>,
    pairing_view: Retained<NSView>,
    pairing_open: bool,
    pairing_title: Retained<NSTextField>,
    pairing_back: Retained<NSButton>,
    pairing_url: String,
    pairing_lang: Option<never_sleep_core::Lang>,
    pairing_qr: Retained<NSImageView>,
    pairing_copy: Retained<NSButton>,
    pairing_hint: Retained<NSTextField>,
    pairing_label: Retained<NSTextField>,
    pairing_value: Retained<NSTextField>,
    language: Retained<NSSegmentedControl>,
    help_button: Retained<NSButton>,
    quit_settings: Retained<NSButton>,
    help_back: Retained<NSButton>,
    help_title: Retained<NSTextField>,
    help_kicker: Retained<NSTextField>,
    help_lead: Retained<NSTextField>,
    help_how: Retained<NSTextField>,
    help_step1_title: Retained<NSTextField>,
    help_step1_detail: Retained<NSTextField>,
    help_step2_title: Retained<NSTextField>,
    help_step2_detail: Retained<NSTextField>,
    help_step3_title: Retained<NSTextField>,
    help_step3_detail: Retained<NSTextField>,
    help_notes: Retained<NSTextField>,
    help_note_lid: Retained<NSTextField>,
    help_note_battery: Retained<NSTextField>,
    help_note_quit: Retained<NSTextField>,
    help_scroll: Retained<NSScrollView>,
    current: PanelView,
    help_from: PanelView,
}

impl NativePanel {
    pub fn attach(window: &Window, proxy: EventLoopProxy<UserEvent>) -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("native panel: not on the main thread")?;
        let target = PanelTarget::new(mtm, proxy);
        let sun = load_png(include_bytes!("../ui/assets/sun.png"))?;
        let moon = load_png(include_bytes!("../ui/assets/moon.png"))?;
        sun.setSize(objc2_foundation::NSSize::new(HERO_IMAGE, HERO_IMAGE));
        moon.setSize(objc2_foundation::NSSize::new(HERO_IMAGE, HERO_IMAGE));

        let ns_window = unsafe { &*window.ns_window().cast::<NSWindow>() };
        ns_window.setOpaque(false);
        ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
        ns_window.setHasShadow(false);

        let host = unsafe { &*window.ns_view().cast::<NSView>() };
        host.setWantsLayer(true);
        host.setClipsToBounds(false);
        if let Some(layer) = backing_layer(host) {
            layer.setMasksToBounds(false);
            layer.setBackgroundColor(Some(&cg_color(&NSColor::clearColor())));
        }

        let glass_ok = AnyClass::get(c"NSGlassEffectView").is_some();
        let kind = preferred_glass(glass_ok);
        let (root, wash, clip) = panel_shell(host, mtm, kind);

        let pairing_view = NSView::new(mtm);
        pin_fill(&root, &pairing_view);
        let main_view = NSView::new(mtm);
        let settings_view = NSView::new(mtm);
        let help_view = NSView::new(mtm);
        pin_fill(&root, &main_view);
        pin_fill(&root, &settings_view);
        pin_fill(&root, &help_view);

        let coin_box = NSBox::new(mtm);
        coin_box.setBoxType(NSBoxType::Custom);
        coin_box.setTitlePosition(NSTitlePosition::NoTitle);
        coin_box.setCornerRadius(HERO_SIZE / 2.0);
        coin_box.setBorderWidth(0.5);
        coin_box.setBorderColor(&NSColor::separatorColor());
        coin_box.setFillColor(&NSColor::controlBackgroundColor());
        coin_box.setContentViewMargins(objc2_foundation::NSSize::new(0.0, 0.0));
        nv(&*coin_box)
            .widthAnchor()
            .constraintEqualToConstant(HERO_SIZE)
            .setActive(true);
        nv(&*coin_box)
            .heightAnchor()
            .constraintEqualToConstant(HERO_SIZE)
            .setActive(true);

        let well = NSView::new(mtm);
        well.setWantsLayer(true);
        let (coin, rotator, sun_face, moon_face) = coin_stack(&sun, &moon, mtm);
        center_square(&well, &coin);
        if let Some(layer) = backing_layer(&well) {
            apply_perspective(&layer);
        }
        coin_box.setContentView(Some(&well));

        let hero = unsafe {
            NSButton::buttonWithTitle_target_action(
                &ns(""),
                Some(as_any(&target)),
                Some(sel!(toggle:)),
                mtm,
            )
        };
        hero.setBordered(false);
        hero.setImagePosition(NSCellImagePosition::ImageOnly);
        nv(&*hero)
            .widthAnchor()
            .constraintEqualToConstant(HERO_SIZE)
            .setActive(true);
        nv(&*hero)
            .heightAnchor()
            .constraintEqualToConstant(HERO_SIZE)
            .setActive(true);
        let hero_host = NSView::new(mtm);
        nv(&*hero_host)
            .widthAnchor()
            .constraintEqualToConstant(HERO_SIZE)
            .setActive(true);
        nv(&*hero_host)
            .heightAnchor()
            .constraintEqualToConstant(HERO_SIZE)
            .setActive(true);
        pin_fill(&hero_host, nv(&*coin_box));
        pin_fill(&hero_host, nv(&*hero));

        let status_title = heading(mtm, 17.0);
        status_title.setAlignment(NSTextAlignment::Center);
        let summary = wrap_to(mtm, 12.0, panel_inner_width());
        summary.setAlignment(NSTextAlignment::Center);
        let elapsed = heading(mtm, 17.0);
        elapsed.setAlignment(NSTextAlignment::Center);
        elapsed.setFont(Some(&tabular_font(17.0)));
        nv(&*elapsed)
            .heightAnchor()
            .constraintEqualToConstant(ELAPSED_HEIGHT)
            .setActive(true);
        let warning = wrap_to(mtm, 12.0, panel_inner_width());
        warning.setAlignment(NSTextAlignment::Center);
        warning.setTextColor(Some(&NSColor::systemOrangeColor()));

        let primary = push_button(&target, sel!(toggle:), NSBezelStyle::Push, mtm);
        nv(&*primary)
            .heightAnchor()
            .constraintEqualToConstant(PRIMARY_HEIGHT)
            .setActive(true);
        fill_width(nv(&*primary));

        let sleep_now = push_button(&target, sel!(sleepNow:), NSBezelStyle::Push, mtm);
        nv(&*sleep_now)
            .heightAnchor()
            .constraintEqualToConstant(PRIMARY_HEIGHT)
            .setActive(true);
        fill_width(nv(&*sleep_now));
        let sleep_host = NSView::new(mtm);
        nv(&*sleep_host)
            .heightAnchor()
            .constraintEqualToConstant(PRIMARY_HEIGHT)
            .setActive(true);
        pin_fill(&sleep_host, nv(&*sleep_now));

        let duration_label = row_caption(mtm);
        let duration = NSPopUpButton::new(mtm);
        unsafe {
            duration.setTarget(Some(as_any(&target)));
            duration.setAction(Some(sel!(durationChanged:)));
        }
        duration.setBordered(false);

        let more = text_button(&target, sel!(more:), mtm);
        let quit_main = text_button(&target, sel!(quit:), mtm);

        let status = column(mtm, 3.0, 0.0);
        status.setAlignment(NSLayoutAttribute::CenterX);
        status.setDetachesHiddenViews(false);
        arrange(&status, &status_title);
        arrange(&status, &summary);
        arrange(&status, &elapsed);
        arrange(&status, &warning);
        nv(&*warning)
            .heightAnchor()
            .constraintGreaterThanOrEqualToConstant(WARNING_SLOT - 3.0)
            .setActive(true);

        let hero_wrap = column(mtm, 0.0, 0.0);
        hero_wrap.setAlignment(NSLayoutAttribute::CenterX);
        arrange(&hero_wrap, &hero_host);

        let quick_duration_label = row_caption(mtm);
        let quick_duration = NSPopUpButton::new(mtm);
        quick_duration.setBordered(false);
        unsafe {
            quick_duration.setTarget(Some(as_any(&target)));
            quick_duration.setAction(Some(sel!(durationChanged:)));
        }
        let (quick_screen_label, quick_screen, quick_screen_row) =
            labeled_switch(&target, TAG_SCREEN_OFF, mtm);
        let phone_main = text_button(&target, sel!(phoneBoard:), mtm);
        let quick_card = grouped_card(
            mtm,
            &[
                duration_row(&quick_duration_label, quick_duration.as_ref(), mtm),
                quick_screen_row,
                navigation_row(&phone_main, mtm),
            ],
        );
        let footer = chrome_bar(&more, None, Some(&quit_main), mtm);
        let main_slack = NSView::new(mtm);
        stretch(&main_slack);
        main_slack
            .heightAnchor()
            .constraintGreaterThanOrEqualToConstant(FOOTER_GAP)
            .setActive(true);
        let main_stack = column(mtm, 0.0, CONTENT_INSET);
        main_stack.setAlignment(NSLayoutAttribute::CenterX);
        main_stack.setDistribution(NSStackViewDistribution::Fill);
        arrange(&main_stack, &hero_wrap);
        spacer(&main_stack, 12.0, mtm);
        arrange(&main_stack, &status);
        spacer(&main_stack, 14.0, mtm);
        arrange(&main_stack, &primary);
        spacer(&main_stack, PRIMARY_CLUSTER_GAP, mtm);
        arrange(&main_stack, &sleep_host);
        arrange(&main_stack, &quick_card);
        arrange(&main_stack, &main_slack);
        arrange(&main_stack, &footer);
        pin_fill(&main_view, nv(&*main_stack));
        span_stack(&main_stack, nv(&*hero_wrap));
        span_stack(&main_stack, nv(&*status));
        span_stack(&main_stack, nv(&*primary));
        span_stack(&main_stack, &sleep_host);
        span_stack(&main_stack, nv(&*quick_card));
        span_stack(&main_stack, &main_slack);
        span_stack(&main_stack, nv(&*footer));

        let back = icon_button(&target, sel!(back:), "chevron.left", mtm);
        let settings_title = heading(mtm, 13.0);
        settings_title.setAlignment(NSTextAlignment::Center);
        let (screen_off_label, screen_off, screen_off_row) =
            labeled_switch(&target, TAG_SCREEN_OFF, mtm);
        let (lid_label, lid, lid_row) = labeled_switch(&target, TAG_LID, mtm);
        let (resleep_settings_label, resleep_settings, resleep_settings_row) =
            labeled_switch(&target, TAG_RESLEEP, mtm);
        let (lock_label, lock, lock_row) = labeled_switch(&target, TAG_LOCK, mtm);
        lock_label.setFont(Some(&NSFont::systemFontOfSize(11.5)));
        let (battery_settings_label, battery_settings, battery_settings_row) =
            labeled_switch(&target, TAG_BATTERY, mtm);
        let (login_label, login, login_row) = labeled_switch(&target, TAG_LOGIN, mtm);
        let pairing_label = row_caption(mtm);
        let pairing_value = row_caption(mtm);
        pairing_value.setSelectable(true);
        pairing_value.setAlignment(NSTextAlignment::Center);
        pairing_value.setFont(Some(&tabular_font(22.0)));
        let phone_settings = text_button(&target, sel!(phoneBoard:), mtm);
        let settings_card = grouped_card(
            mtm,
            &[
                duration_row(&duration_label, duration.as_ref(), mtm),
                screen_off_row,
                lid_row,
                resleep_settings_row,
                lock_row,
                battery_settings_row,
                login_row,
                navigation_row(&phone_settings, mtm),
            ],
        );
        let pairing_qr = NSImageView::new(mtm);
        pairing_qr
            .heightAnchor()
            .constraintEqualToConstant(160.0)
            .setActive(true);
        let pairing_copy = push_button(&target, sel!(copyPairing:), NSBezelStyle::Push, mtm);
        let pairing_hint = wrap_to(mtm, 11.0, panel_inner_width());
        pairing_hint.setAlignment(NSTextAlignment::Center);
        let language = NSSegmentedControl::new(mtm);
        language.setSegmentCount(2);
        language.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
        language.setLabel_forSegment(&ns("English"), 0);
        language.setLabel_forSegment(&ns("简体中文"), 1);
        unsafe {
            language.setTarget(Some(as_any(&target)));
            language.setAction(Some(sel!(languageChanged:)));
        }
        fill_width(nv(&*language));
        nv(&*language)
            .heightAnchor()
            .constraintEqualToConstant(LANGUAGE_HEIGHT)
            .setActive(true);
        let help_button = text_button(&target, sel!(help:), mtm);
        let quit_settings = text_button(&target, sel!(quit:), mtm);

        let settings_head = sheet_head(&back, &settings_title, mtm);
        let settings_footer = chrome_bar(&help_button, None, Some(&quit_settings), mtm);
        let settings_slack = NSView::new(mtm);
        stretch(&settings_slack);
        settings_slack
            .heightAnchor()
            .constraintGreaterThanOrEqualToConstant(FOOTER_GAP)
            .setActive(true);
        let settings_stack = column(mtm, 0.0, CONTENT_INSET);
        settings_stack.setAlignment(NSLayoutAttribute::Leading);
        settings_stack.setDistribution(NSStackViewDistribution::Fill);
        arrange(&settings_stack, &settings_head);
        spacer(&settings_stack, 8.0, mtm);
        arrange(&settings_stack, &settings_card);
        spacer(&settings_stack, 12.0, mtm);
        arrange(&settings_stack, &language);

        arrange(&settings_stack, &settings_slack);
        arrange(&settings_stack, &settings_footer);
        pin_fill(&settings_view, nv(&*settings_stack));
        span_stack(&settings_stack, nv(&*settings_head));
        span_stack(&settings_stack, nv(&*settings_card));
        span_stack(&settings_stack, nv(&*language));

        span_stack(&settings_stack, &settings_slack);
        span_stack(&settings_stack, nv(&*settings_footer));

        let pairing_back = icon_button(&target, sel!(back:), "chevron.left", mtm);
        let pairing_title = heading(mtm, 13.0);
        let pairing_head = sheet_head(&pairing_back, &pairing_title, mtm);
        let pairing_stack = column(mtm, 0.0, CONTENT_INSET);
        pairing_stack.setAlignment(NSLayoutAttribute::CenterX);
        arrange(&pairing_stack, &pairing_head);
        spacer(&pairing_stack, 16.0, mtm);
        arrange(&pairing_stack, &pairing_hint);
        spacer(&pairing_stack, 16.0, mtm);
        arrange(&pairing_stack, &pairing_qr);
        spacer(&pairing_stack, 16.0, mtm);
        arrange(&pairing_stack, &pairing_label);
        spacer(&pairing_stack, 6.0, mtm);
        arrange(&pairing_stack, &pairing_value);
        spacer(&pairing_stack, 12.0, mtm);
        arrange(&pairing_stack, &pairing_copy);
        let pairing_slack = NSView::new(mtm);
        stretch(&pairing_slack);
        arrange(&pairing_stack, &pairing_slack);
        pin_fill(&pairing_view, nv(&*pairing_stack));
        for view in [
            nv(&*pairing_head),
            nv(&*pairing_hint),
            nv(&*pairing_qr),
            nv(&*pairing_copy),
        ] {
            span_stack(&pairing_stack, view);
        }

        let help_back = icon_button(&target, sel!(back:), "chevron.left", mtm);
        let help_title = heading(mtm, 13.0);
        help_title.setAlignment(NSTextAlignment::Center);
        let help_kicker = heading(mtm, 12.0);
        help_kicker.setTextColor(Some(&NSColor::controlAccentColor()));
        let help_lead = wrap_to(mtm, 14.0, panel_inner_width());
        help_lead.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
        help_lead.setTextColor(Some(&NSColor::labelColor()));
        let help_how = section_header(mtm);
        let help_step1_title = heading(mtm, 13.0);
        let help_step1_detail = wrap(mtm, 12.0);
        let help_step2_title = heading(mtm, 13.0);
        let help_step2_detail = wrap(mtm, 12.0);
        let help_step3_title = heading(mtm, 13.0);
        let help_step3_detail = wrap(mtm, 12.0);
        let help_notes = section_header(mtm);
        let help_note_lid = wrap(mtm, 13.0);
        help_note_lid.setTextColor(Some(&NSColor::labelColor()));
        let help_note_battery = wrap(mtm, 13.0);
        help_note_battery.setTextColor(Some(&NSColor::labelColor()));
        let help_note_quit = wrap(mtm, 13.0);
        help_note_quit.setTextColor(Some(&NSColor::labelColor()));

        let how_card = grouped_card(
            mtm,
            &[
                help_step(&help_step1_title, &help_step1_detail, "1", mtm),
                help_step(&help_step2_title, &help_step2_detail, "2", mtm),
                help_step(&help_step3_title, &help_step3_detail, "3", mtm),
            ],
        );
        let notes_card = grouped_card(
            mtm,
            &[
                help_note(&help_note_lid, "laptopcomputer", mtm),
                help_note(&help_note_battery, "battery.100", mtm),
                help_note(&help_note_quit, "moon.zzz", mtm),
            ],
        );

        let help_body = column(mtm, 0.0, 0.0);
        help_body.setAlignment(NSLayoutAttribute::Leading);
        hug_vertically(nv(&*help_body));
        help_body.setEdgeInsets(NSEdgeInsets {
            top: 0.0,
            left: 0.0,
            bottom: HELP_BODY_PAD_BOTTOM,
            right: 0.0,
        });
        arrange(&help_body, &help_kicker);
        spacer(&help_body, HELP_LEAD_GAP, mtm);
        arrange(&help_body, &help_lead);
        spacer(&help_body, HELP_BLOCK_GAP, mtm);
        arrange(&help_body, &help_how);
        spacer(&help_body, HELP_SECTION_GAP, mtm);
        arrange(&help_body, &how_card);
        spacer(&help_body, HELP_BLOCK_GAP, mtm);
        arrange(&help_body, &help_notes);
        spacer(&help_body, HELP_SECTION_GAP, mtm);
        arrange(&help_body, &notes_card);
        span_stack(&help_body, nv(&*help_kicker));
        span_stack(&help_body, nv(&*help_lead));
        span_stack(&help_body, nv(&*help_how));
        span_stack(&help_body, nv(&*how_card));
        span_stack(&help_body, nv(&*help_notes));
        span_stack(&help_body, nv(&*notes_card));

        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDocumentView(Some(nv(&*help_body)));
        pin_document_width(&scroll, nv(&*help_body));
        stretch(nv(&*scroll));

        let help_head = sheet_head(&help_back, &help_title, mtm);
        let help_stack = column(mtm, 0.0, CONTENT_INSET);
        help_stack.setAlignment(NSLayoutAttribute::Leading);
        arrange(&help_stack, &help_head);
        spacer(&help_stack, 8.0, mtm);
        arrange(&help_stack, &scroll);
        pin_fill(&help_view, nv(&*help_stack));
        span_stack(&help_stack, nv(&*help_head));
        span_stack(&help_stack, nv(&*scroll));

        let flip_done = CoinFlipDone::new(sun_face.clone(), moon_face.clone(), mtm);
        let panel = Self {
            _target: target,
            wash,
            clip,
            coin,
            rotator,
            sun_face,
            moon_face,
            flip_done,
            main_view,
            settings_view,
            help_view,
            last_active: None,
            status_title,
            summary,
            elapsed,
            warning,
            primary,
            sleep_now,
            duration,
            duration_label,
            more,
            quit_main,
            back,
            settings_title,
            screen_off_label,
            screen_off,
            lid_label,
            lid,
            resleep_settings_label,
            resleep_settings,
            lock_label,
            lock,
            battery_settings_label,
            battery_settings,
            login_label,
            login,
            quick_duration,
            quick_duration_label,
            quick_screen,
            quick_screen_label,
            quick_card,
            status_stack: status,
            sleep_host,
            phone_main,
            phone_settings,
            pairing_view,
            pairing_open: false,
            pairing_title,
            pairing_back,
            pairing_url: String::new(),
            pairing_lang: None,
            pairing_qr,
            pairing_copy,
            pairing_hint,
            pairing_label,
            pairing_value,
            language,
            help_button,
            quit_settings,
            help_back,
            help_title,
            help_kicker,
            help_lead,
            help_how,
            help_step1_title,
            help_step1_detail,
            help_step2_title,
            help_step2_detail,
            help_step3_title,
            help_step3_detail,
            help_notes,
            help_note_lid,
            help_note_battery,
            help_note_quit,
            help_scroll: scroll,
            current: PanelView::Main,
            help_from: PanelView::Main,
        };
        panel.apply_view();
        panel.set_active(false, false);
        Ok(panel)
    }

    pub fn apply(&mut self, state: &PanelState) {
        let animate = self.last_active.is_some_and(|was| was != state.active);
        self.set_active(state.active, animate);
        self.last_active = Some(state.active);
        set_text(&self.status_title, &state.status_title);
        set_text(&self.summary, &state.summary);
        set_text(&self.elapsed, &state.elapsed_clock);
        self.elapsed.setHidden(!state.show_elapsed);
        set_text(&self.warning, &state.warning);
        self.warning.setHidden(state.warning.is_empty());
        self.primary.setTitle(&ns(&state.primary_action));
        self.primary
            .setAccessibilityLabel(Some(&ns(&state.primary_action)));
        self.primary
            .setKeyEquivalent(&ns(if state.active { "" } else { "\r" }));
        self.sleep_now.setTitle(&ns(&state.sleep_now_label));
        self.sleep_now
            .setAccessibilityLabel(Some(&ns(&state.sleep_now_label)));
        self.sleep_now.setHidden(!state.show_sleep_now);
        set_text(&self.quick_duration_label, &state.duration_label);
        self.quick_duration.removeAllItems();
        for title in [
            &state.duration_indefinite,
            &state.duration_1h,
            &state.duration_3h,
            &state.duration_8h,
            &state.duration_until,
        ] {
            self.quick_duration.addItemWithTitle(&ns(title));
        }
        self.quick_duration
            .selectItemAtIndex(state.duration.index());
        set_switch_row(
            &self.quick_screen_label,
            &self.quick_screen,
            &state.screen_off_label,
            state.screen_off,
        );
        self.quick_card.setHidden(state.active);
        self.sleep_host.setHidden(!state.active);
        self.status_stack.setDetachesHiddenViews(!state.active);
        self.warning
            .setHidden(!state.active || state.warning.is_empty());
        let warning_tip = ns(&state.warning);
        self.status_title.setToolTip(if state.warning.is_empty() {
            None
        } else {
            Some(&warning_tip)
        });
        let phone_title = format!("{}  ›", state.pairing_label);
        self.phone_main.setTitle(&ns(&phone_title));
        self.phone_settings.setTitle(&ns(&phone_title));
        set_text(&self.pairing_title, &state.pairing_label);
        self.pairing_back.setToolTip(Some(&ns(&state.back)));
        set_text(&self.duration_label, &state.duration_label);
        self.duration.removeAllItems();
        self.duration
            .addItemWithTitle(&ns(&state.duration_indefinite));
        self.duration.addItemWithTitle(&ns(&state.duration_1h));
        self.duration.addItemWithTitle(&ns(&state.duration_3h));
        self.duration.addItemWithTitle(&ns(&state.duration_8h));
        self.duration.addItemWithTitle(&ns(&state.duration_until));
        self.duration.selectItemAtIndex(state.duration.index());
        self.more.setTitle(&ns(&state.more_settings));
        self.quit_main.setTitle(&ns(&state.quit));

        self.back.setToolTip(Some(&ns(&state.back)));
        set_text(&self.settings_title, &state.settings);
        set_switch_row(
            &self.screen_off_label,
            &self.screen_off,
            &state.screen_off_label,
            state.screen_off,
        );
        set_switch_row(
            &self.lid_label,
            &self.lid,
            &state.lid_awake_label,
            state.lid_awake,
        );
        set_switch_row(
            &self.resleep_settings_label,
            &self.resleep_settings,
            &state.resleep,
            state.resleep_display,
        );
        set_switch_row(
            &self.lock_label,
            &self.lock,
            &state.lock_screen_label,
            state.lock_screen,
        );
        set_switch_row(
            &self.battery_settings_label,
            &self.battery_settings,
            &state.battery,
            state.battery_floor,
        );
        set_switch_row(
            &self.login_label,
            &self.login,
            &state.launch_at_login_label,
            state.launch_at_login,
        );
        let zh = state.lang == never_sleep_core::Lang::Zh;
        let changed = *self._target.ivars().pairing_code.borrow() != state.pairing_code;
        if changed || self.pairing_lang != Some(state.lang) {
            *self._target.ivars().pairing_code.borrow_mut() = state.pairing_code.clone();
            self.pairing_copy.setTitle(&ns(if zh {
                "复制配对码"
            } else {
                "Copy pairing code"
            }));
        }
        self.pairing_copy.setEnabled(!state.pairing_code.is_empty());
        self.pairing_lang = Some(state.lang);
        if self.pairing_url != state.pairing_url {
            let qr = pairing_qr_image(&state.pairing_url);
            self.pairing_qr.setImage(qr.as_deref());
            self.pairing_url.clone_from(&state.pairing_url);
        }
        set_text(
            &self.pairing_hint,
            if state.pairing_code.is_empty() {
                if zh {
                    "连接后将自动生成新的配对码"
                } else {
                    "A new code appears when connected"
                }
            } else if zh {
                "用手机相机扫描二维码，即可打开看板并配对这台 Mac。"
            } else {
                "Scan with your phone camera to open the board and pair this Mac."
            },
        );
        set_text(
            &self.pairing_label,
            if zh { "配对码" } else { "Pairing code" },
        );
        set_text(
            &self.pairing_value,
            if state.pairing_code.is_empty() {
                "—"
            } else {
                &state.pairing_code
            },
        );
        self.language
            .setSelectedSegment(if state.lang == never_sleep_core::Lang::Zh {
                1
            } else {
                0
            });
        self.help_button.setTitle(&ns(&state.help));
        self.quit_settings.setTitle(&ns(&state.quit));

        self.help_back.setToolTip(Some(&ns(&state.back)));
        set_text(&self.help_title, &state.help);
        set_text(&self.help_kicker, &state.help_kicker);
        set_text(&self.help_lead, &state.help_lead);
        let how = if state.lang == never_sleep_core::Lang::En {
            state.help_how.to_uppercase()
        } else {
            state.help_how.clone()
        };
        set_text(&self.help_how, &how);
        set_text(&self.help_step1_title, &state.help_step1_title);
        set_text(&self.help_step1_detail, &state.help_step1_detail);
        set_text(&self.help_step2_title, &state.help_step2_title);
        set_text(&self.help_step2_detail, &state.help_step2_detail);
        set_text(&self.help_step3_title, &state.help_step3_title);
        set_text(&self.help_step3_detail, &state.help_step3);
        let notes = if state.lang == never_sleep_core::Lang::En {
            state.help_notes.to_uppercase()
        } else {
            state.help_notes.clone()
        };
        set_text(&self.help_notes, &notes);
        set_text(&self.help_note_lid, &state.help_note_lid);
        set_text(&self.help_note_battery, &state.help_note_battery);
        set_text(&self.help_note_quit, &state.help_note_quit);
        self.apply_view();
    }

    pub fn set_elapsed_clock(&self, value: &str) {
        set_text(&self.elapsed, value);
    }

    pub fn show_help(&mut self) {
        self.open_help(self.current, false);
    }

    pub fn show_help_from_menu(&mut self) {
        self.open_help(menu_help_origin(), true);
    }

    fn open_help(&mut self, origin: PanelView, from_menu: bool) {
        self.help_from = help_from_after_open(self.current, self.help_from, origin, from_menu);
        self.show_pane(SidebarItem::Help);
        self.scroll_help_to_top();
    }

    fn scroll_help_to_top(&self) {
        let scroll = &self.help_scroll;
        let Some(doc) = scroll.documentView() else {
            return;
        };
        self.help_view.layoutSubtreeIfNeeded();
        let clip = scroll.contentView();
        let width = clip.bounds().size.width;
        if width > 0.0 {
            doc.setFrameSize(objc2_foundation::NSSize::new(
                width,
                doc.fittingSize().height,
            ));
        }
        doc.layoutSubtreeIfNeeded();
        let y = help_scroll_y(
            doc.frame().size.height,
            clip.bounds().size.height,
            clip.isFlipped(),
        );
        clip.scrollToPoint(NSPoint::new(0.0, y));
        scroll.reflectScrolledClipView(&clip);
    }

    pub fn show_settings(&mut self) {
        self.show_pane(SidebarItem::Display);
    }

    pub fn show_pairing(&mut self) {
        self.pairing_open = true;
        self.apply_view();
    }

    pub fn go_back(&mut self) {
        if self.pairing_open {
            self.pairing_open = false;
            self.apply_view();
            return;
        }
        match self.current {
            PanelView::Help => {
                let item = match help_back_target(self.help_from) {
                    PanelView::Settings => SidebarItem::Display,
                    _ => SidebarItem::Standby,
                };
                self.show_pane(item);
            }
            PanelView::Settings | PanelView::Main => self.show_pane(SidebarItem::Standby),
        }
    }

    pub fn show_pane(&mut self, item: SidebarItem) {
        self.pairing_open = false;
        let _ = item.symbol();
        self.current = item.as_panel_view();
        self.apply_view();
    }

    fn apply_view(&self) {
        self.pairing_view.setHidden(!self.pairing_open);
        self.main_view
            .setHidden(self.pairing_open || self.current != PanelView::Main);
        self.settings_view
            .setHidden(self.pairing_open || self.current != PanelView::Settings);
        self.help_view
            .setHidden(self.pairing_open || self.current != PanelView::Help);
    }

    fn set_active(&self, active: bool, animate: bool) {
        let reduce = reduce_motion();
        let color_secs = if animate {
            motion_duration_secs(reduce, PANEL_COLOR_SECS)
        } else {
            0.0
        };
        let flip_secs = if animate && hero_flips(reduce) {
            HERO_FLIP_SECS
        } else {
            0.0
        };
        // Appearance first, actions off, then flush so Dark Aqua cannot restart the flip.
        set_chrome_appearance(&self.clip, active);
        set_fill_color(&self.wash, panel_fill_rgb(active), color_secs);
        set_coin_flip(
            &self.coin,
            &self.rotator,
            &self.sun_face,
            &self.moon_face,
            &self.flip_done,
            hero_shows_moon(active),
            flip_secs,
        );
        if let Some(window) = self.wash.window() {
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
        }
    }
}

fn as_any(obj: &PanelTarget) -> &AnyObject {
    // SAFETY: PanelTarget is an Objective-C class instance.
    unsafe { &*(core::ptr::from_ref(obj).cast::<AnyObject>()) }
}

fn ns(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}

fn load_png(bytes: &[u8]) -> Result<Retained<NSImage>, String> {
    let data = NSData::with_bytes(bytes);
    NSImage::initWithData(NSImage::alloc(), &data).ok_or_else(|| "panel image".into())
}

fn nv<T: AsRef<NSView>>(obj: &T) -> &NSView {
    obj.as_ref()
}

fn arrange<T>(stack: &NSStackView, child: &T)
where
    T: Deref,
    T::Target: AsRef<NSView>,
{
    stack.addArrangedSubview(child.deref().as_ref());
}

fn pin_fill(parent: &NSView, child: &NSView) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
    child
        .leadingAnchor()
        .constraintEqualToAnchor(&parent.leadingAnchor())
        .setActive(true);
    child
        .trailingAnchor()
        .constraintEqualToAnchor(&parent.trailingAnchor())
        .setActive(true);
    child
        .topAnchor()
        .constraintEqualToAnchor(&parent.topAnchor())
        .setActive(true);
    child
        .bottomAnchor()
        .constraintEqualToAnchor(&parent.bottomAnchor())
        .setActive(true);
}

fn center_square(parent: &NSView, child: &NSView) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
    child
        .centerXAnchor()
        .constraintEqualToAnchor(&parent.centerXAnchor())
        .setActive(true);
    child
        .centerYAnchor()
        .constraintEqualToAnchor(&parent.centerYAnchor())
        .setActive(true);
}

fn fill_width(view: &NSView) {
    view.setContentHuggingPriority_forOrientation(
        1.0_f32,
        NSLayoutConstraintOrientation::Horizontal,
    );
}

fn span_stack(stack: &NSStackView, child: &NSView) {
    let inset = stack.edgeInsets();
    child
        .widthAnchor()
        .constraintEqualToAnchor_constant(&stack.widthAnchor(), -(inset.left + inset.right))
        .setActive(true);
}

fn stretch(view: &NSView) {
    view.setContentHuggingPriority_forOrientation(1.0_f32, NSLayoutConstraintOrientation::Vertical);
}

fn hug_vertically(view: &NSView) {
    view.setContentHuggingPriority_forOrientation(
        750.0_f32,
        NSLayoutConstraintOrientation::Vertical,
    );
    view.setContentCompressionResistancePriority_forOrientation(
        1000.0_f32,
        NSLayoutConstraintOrientation::Vertical,
    );
}

fn spacer(stack: &NSStackView, height: f64, mtm: MainThreadMarker) {
    let gap = NSView::new(mtm);
    gap.heightAnchor()
        .constraintEqualToConstant(height)
        .setActive(true);
    arrange(stack, &gap);
}

fn pin_document_width(scroll: &NSScrollView, document: &NSView) {
    document.setTranslatesAutoresizingMaskIntoConstraints(false);
    let clip = scroll.contentView();
    document
        .leadingAnchor()
        .constraintEqualToAnchor(&clip.leadingAnchor())
        .setActive(true);
    document
        .widthAnchor()
        .constraintEqualToAnchor(&clip.widthAnchor())
        .setActive(true);
}

fn panel_shell(
    host: &NSView,
    mtm: MainThreadMarker,
    kind: GlassKind,
) -> (Retained<NSView>, Retained<NSView>, Retained<NSView>) {
    let card = NSView::new(mtm);
    card.setWantsLayer(true);
    card.setTranslatesAutoresizingMaskIntoConstraints(false);
    host.addSubview(&card);
    card.leadingAnchor()
        .constraintEqualToAnchor_constant(&host.leadingAnchor(), SHADOW_INSET)
        .setActive(true);
    card.trailingAnchor()
        .constraintEqualToAnchor_constant(&host.trailingAnchor(), -SHADOW_INSET)
        .setActive(true);
    card.topAnchor()
        .constraintEqualToAnchor_constant(&host.topAnchor(), SHADOW_INSET_TOP)
        .setActive(true);
    card.bottomAnchor()
        .constraintEqualToAnchor_constant(&host.bottomAnchor(), -SHADOW_INSET)
        .setActive(true);
    decorate_card_shadow(&card);

    let clip = NSView::new(mtm);
    clip.setWantsLayer(true);
    pin_fill(&card, &clip);
    if let Some(layer) = backing_layer(&clip) {
        layer_set_corner_radius(&layer, PANEL_CORNER);
        layer.setMasksToBounds(true);
    }

    let root = panel_chrome(&clip, mtm, kind);
    let wash = NSView::new(mtm);
    wash.setWantsLayer(true);
    pin_fill(&root, &wash);
    set_fill_color(&wash, panel_fill_rgb(false), 0.0);
    (root, wash, clip)
}

fn panel_chrome(host: &NSView, mtm: MainThreadMarker, kind: GlassKind) -> Retained<NSView> {
    match kind {
        GlassKind::LiquidGlass => {
            let glass = NSGlassEffectView::new(mtm);
            glass.setStyle(NSGlassEffectViewStyle::Regular);
            glass.setCornerRadius(PANEL_CORNER);
            let content = NSView::new(mtm);
            content.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            glass.setContentView(Some(&content));
            pin_fill(host, nv(&*glass));
            content
        }
        GlassKind::Vibrancy => {
            let visual = NSVisualEffectView::new(mtm);
            visual.setMaterial(NSVisualEffectMaterial::Popover);
            visual.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            visual.setState(NSVisualEffectState::Active);
            visual.setWantsLayer(true);
            pin_fill(host, nv(&*visual));
            let content = NSView::new(mtm);
            pin_fill(nv(&*visual), &content);
            content
        }
    }
}

fn grouped_card(mtm: MainThreadMarker, rows: &[Retained<NSStackView>]) -> Retained<NSView> {
    let body = column(mtm, 0.0, 0.0);
    body.setAlignment(NSLayoutAttribute::Width);
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            if CARD_SEPARATOR_GAP > 0.0 {
                spacer(&body, CARD_SEPARATOR_GAP, mtm);
            }
            let line = separator(mtm);
            arrange(&body, &line);
            span_stack(&body, nv(&*line));
            if CARD_SEPARATOR_GAP > 0.0 {
                spacer(&body, CARD_SEPARATOR_GAP, mtm);
            }
        }
        arrange(&body, row);
        span_stack(&body, nv(&**row));
    }
    let card = NSBox::new(mtm);
    card.setBoxType(NSBoxType::Custom);
    card.setTitlePosition(NSTitlePosition::NoTitle);
    card.setCornerRadius(CARD_RADIUS);
    card.setBorderWidth(0.5);
    card.setBorderColor(&NSColor::separatorColor());
    card.setFillColor(&NSColor::controlBackgroundColor());
    card.setContentViewMargins(objc2_foundation::NSSize::new(0.0, 0.0));
    card.setContentView(Some(nv(&*body)));
    fill_width(nv(&*card));
    hug_vertically(nv(&*body));
    hug_vertically(nv(&*card));
    Retained::into_super(card)
}

fn separator(mtm: MainThreadMarker) -> Retained<NSBox> {
    let line = NSBox::new(mtm);
    line.setBoxType(NSBoxType::Separator);
    line.setContentViewMargins(objc2_foundation::NSSize::new(0.0, 0.0));
    nv(&*line)
        .heightAnchor()
        .constraintEqualToConstant(CARD_HAIRLINE)
        .setActive(true);
    line
}

fn column(mtm: MainThreadMarker, spacing: f64, inset: f64) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setSpacing(spacing);
    stack.setEdgeInsets(NSEdgeInsets {
        top: inset,
        left: inset,
        bottom: inset,
        right: inset,
    });
    stack
}

fn heading(mtm: MainThreadMarker, size: f64) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&ns(""), mtm);
    field.setFont(Some(&NSFont::boldSystemFontOfSize(size)));
    field.setTextColor(Some(&NSColor::labelColor()));
    field.setAlignment(NSTextAlignment::Left);
    field
}

fn section_header(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&ns(""), mtm);
    field.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    field.setAlignment(NSTextAlignment::Left);
    field
}

fn wrap(mtm: MainThreadMarker, size: f64) -> Retained<NSTextField> {
    wrap_to(mtm, size, grouped_copy_max_width())
}

fn wrap_to(mtm: MainThreadMarker, size: f64, max_width: f64) -> Retained<NSTextField> {
    let field = NSTextField::wrappingLabelWithString(&ns(""), mtm);
    field.setSelectable(false);
    field.setFont(Some(&NSFont::systemFontOfSize(size)));
    field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    field.setAlignment(NSTextAlignment::Left);
    field.setPreferredMaxLayoutWidth(max_width);
    field.setContentHuggingPriority_forOrientation(
        1.0_f32,
        NSLayoutConstraintOrientation::Horizontal,
    );
    field.setContentCompressionResistancePriority_forOrientation(
        250.0_f32,
        NSLayoutConstraintOrientation::Horizontal,
    );
    field
}

fn row_caption(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let field = NSTextField::wrappingLabelWithString(&ns(""), mtm);
    field.setSelectable(false);
    field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    field.setTextColor(Some(&NSColor::labelColor()));
    field.setAlignment(NSTextAlignment::Left);
    field.setPreferredMaxLayoutWidth(switch_copy_max_width());
    field.setContentHuggingPriority_forOrientation(
        1.0_f32,
        NSLayoutConstraintOrientation::Horizontal,
    );
    field.setContentCompressionResistancePriority_forOrientation(
        250.0_f32,
        NSLayoutConstraintOrientation::Horizontal,
    );
    field.setContentCompressionResistancePriority_forOrientation(
        750.0_f32,
        NSLayoutConstraintOrientation::Vertical,
    );
    field
}

fn set_text(field: &NSTextField, value: &str) {
    field.setStringValue(&ns(value));
}

fn set_switch(control: &NSSwitch, on: bool) {
    control.setState(if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
}

fn set_switch_row(label: &NSTextField, toggle: &NSSwitch, text: &str, on: bool) {
    set_text(label, text);
    set_switch(toggle, on);
    toggle.setAccessibilityLabel(Some(&ns(text)));
}

fn bind_switch(toggle: &NSSwitch, target: &PanelTarget, tag: isize) {
    unsafe {
        toggle.setTarget(Some(as_any(target)));
        toggle.setAction(Some(sel!(optionChanged:)));
    }
    toggle.setTag(tag);
}

fn tabular_font(size: f64) -> Retained<NSFont> {
    unsafe { msg_send![NSFont::class(), monospacedDigitSystemFontOfSize: size, weight: 0.0] }
}

fn push_button(
    target: &PanelTarget,
    action: Sel,
    bezel: NSBezelStyle,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(&ns(""), Some(as_any(target)), Some(action), mtm)
    };
    button.setBezelStyle(bezel);
    button
}

fn text_button(target: &PanelTarget, action: Sel, mtm: MainThreadMarker) -> Retained<NSButton> {
    let button = push_button(target, action, NSBezelStyle::AccessoryBarAction, mtm);
    button.setBordered(false);
    button.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    button
}

fn icon_button(
    target: &PanelTarget,
    action: Sel,
    symbol: &str,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let button = push_button(target, action, NSBezelStyle::AccessoryBarAction, mtm);
    button.setBordered(false);
    if let Some(image) =
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(symbol), None)
    {
        image.setTemplate(true);
        button.setImage(Some(&image));
        button.setImagePosition(NSCellImagePosition::ImageOnly);
    }
    nv(&*button)
        .widthAnchor()
        .constraintEqualToConstant(28.0)
        .setActive(true);
    nv(&*button)
        .heightAnchor()
        .constraintEqualToConstant(28.0)
        .setActive(true);
    button
}

fn labeled_switch(
    target: &PanelTarget,
    tag: isize,
    mtm: MainThreadMarker,
) -> (
    Retained<NSTextField>,
    Retained<NSSwitch>,
    Retained<NSStackView>,
) {
    let caption = row_caption(mtm);
    let toggle = NSSwitch::new(mtm);
    bind_switch(&toggle, target, tag);
    nv(&*toggle).setContentHuggingPriority_forOrientation(
        750.0_f32,
        NSLayoutConstraintOrientation::Horizontal,
    );
    let row = control_row(&caption, nv(&*toggle), mtm);
    (caption, toggle, row)
}

fn duration_row(
    caption: &NSTextField,
    popup: &NSPopUpButton,
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    control_row(caption, nv(popup), mtm)
}

/// The entire row is the navigation target, including its trailing chevron.
fn navigation_row(button: &NSButton, mtm: MainThreadMarker) -> Retained<NSStackView> {
    button.setAlignment(NSTextAlignment::Left);
    let row = column(mtm, 0.0, 0.0);
    row.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: CARD_ROW_INSET_X,
        bottom: 0.0,
        right: CARD_ROW_INSET_X,
    });
    arrange(&row, button);
    span_stack(&row, nv(button));
    nv(button)
        .heightAnchor()
        .constraintEqualToConstant(CARD_ROW_HEIGHT)
        .setActive(true);
    row
}

fn control_row(
    caption: &NSTextField,
    control: &NSView,
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    caption.setAlignment(NSTextAlignment::Left);
    control.setContentHuggingPriority_forOrientation(
        750.0_f32,
        NSLayoutConstraintOrientation::Horizontal,
    );
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setDistribution(NSStackViewDistribution::Fill);
    row.setAlignment(NSLayoutAttribute::CenterY);
    row.setSpacing(CONTROL_ROW_GAP);
    row.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: CARD_ROW_INSET_X,
        bottom: 0.0,
        right: CARD_ROW_INSET_X,
    });
    arrange(&row, caption);
    row.addArrangedSubview(control);
    nv(&*row)
        .heightAnchor()
        .constraintGreaterThanOrEqualToConstant(CARD_ROW_HEIGHT)
        .setActive(true);
    fill_width(nv(&*row));
    row
}

fn sheet_head(
    back: &NSButton,
    title: &NSTextField,
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    let tail = NSView::new(mtm);
    nv(&*tail)
        .widthAnchor()
        .constraintEqualToConstant(28.0)
        .setActive(true);
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::CenterY);
    row.setDistribution(NSStackViewDistribution::Fill);
    row.setSpacing(0.0);
    arrange(&row, back);
    arrange(&row, title);
    arrange(&row, &tail);
    nv(&*row)
        .heightAnchor()
        .constraintEqualToConstant(SHEET_HEAD_HEIGHT)
        .setActive(true);
    fill_width(nv(&*row));
    row
}

fn chrome_bar(
    leading: &NSButton,
    middle: Option<&NSButton>,
    trailing: Option<&NSButton>,
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::CenterY);
    row.setSpacing(2.0);
    let wrap = column(mtm, 0.0, 0.0);
    wrap.setAlignment(NSLayoutAttribute::CenterX);
    arrange(&row, leading);
    if let Some(middle) = middle {
        arrange(&row, &hairline(mtm));
        arrange(&row, middle);
    }
    if let Some(trailing) = trailing {
        arrange(&row, &hairline(mtm));
        arrange(&row, trailing);
    }
    arrange(&wrap, &row);
    fill_width(nv(&*wrap));
    nv(&*wrap)
        .heightAnchor()
        .constraintEqualToConstant(FOOTER_HEIGHT)
        .setActive(true);
    wrap
}

fn hairline(mtm: MainThreadMarker) -> Retained<NSBox> {
    let line = NSBox::new(mtm);
    line.setBoxType(NSBoxType::Separator);
    nv(&*line)
        .widthAnchor()
        .constraintEqualToConstant(1.0)
        .setActive(true);
    nv(&*line)
        .heightAnchor()
        .constraintEqualToConstant(12.0)
        .setActive(true);
    line
}

fn index_badge(index: &str, mtm: MainThreadMarker) -> Retained<NSView> {
    let badge = NSView::new(mtm);
    badge.setWantsLayer(true);
    nv(&*badge)
        .widthAnchor()
        .constraintEqualToConstant(HELP_ROW_GLYPH)
        .setActive(true);
    nv(&*badge)
        .heightAnchor()
        .constraintEqualToConstant(HELP_ROW_GLYPH)
        .setActive(true);
    if let Some(layer) = backing_layer(&badge) {
        layer_set_corner_radius(&layer, HELP_ROW_GLYPH / 2.0);
        layer.setBackgroundColor(Some(&cg_color(&NSColor::controlAccentColor())));
    }
    let label = NSTextField::labelWithString(&ns(index), mtm);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
    label.setTextColor(Some(&NSColor::whiteColor()));
    label.setAlignment(NSTextAlignment::Center);
    label.setDrawsBackground(false);
    label.setBezeled(false);
    label.setBordered(false);
    label.setTranslatesAutoresizingMaskIntoConstraints(false);
    badge.addSubview(&label);
    label
        .centerXAnchor()
        .constraintEqualToAnchor(&badge.centerXAnchor())
        .setActive(true);
    // Unflipped NSView: a small negative offset drops the digit onto the optical center.
    label
        .centerYAnchor()
        .constraintEqualToAnchor_constant(&badge.centerYAnchor(), -0.5)
        .setActive(true);
    badge
}

fn help_step(
    title: &NSTextField,
    detail: &NSTextField,
    index: &str,
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    let copy = column(mtm, HELP_COPY_GAP, 0.0);
    copy.setAlignment(NSLayoutAttribute::Leading);
    arrange(&copy, title);
    arrange(&copy, detail);
    fill_width(nv(&*copy));
    glyph_copy_row(nv(&*index_badge(index, mtm)), nv(&*copy), mtm)
}

fn help_note(copy: &NSTextField, symbol: &str, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let icon = NSImageView::new(mtm);
    if let Some(image) =
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(symbol), None)
    {
        image.setTemplate(true);
        icon.setImage(Some(&image));
    }
    icon.setContentTintColor(Some(&NSColor::controlAccentColor()));
    icon.setEditable(false);
    nv(&*icon)
        .widthAnchor()
        .constraintEqualToConstant(HELP_ROW_GLYPH)
        .setActive(true);
    nv(&*icon)
        .heightAnchor()
        .constraintEqualToConstant(HELP_ROW_GLYPH)
        .setActive(true);
    glyph_copy_row(nv(&*icon), nv(copy), mtm)
}

fn glyph_copy_row(glyph: &NSView, copy: &NSView, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let inner = NSStackView::new(mtm);
    inner.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    inner.setAlignment(NSLayoutAttribute::Top);
    inner.setDistribution(NSStackViewDistribution::Fill);
    inner.setSpacing(HELP_ROW_GAP);
    inner.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: HELP_ROW_INSET,
        bottom: 0.0,
        right: HELP_ROW_INSET,
    });
    inner.addArrangedSubview(glyph);
    inner.addArrangedSubview(copy);
    fill_width(nv(&*inner));
    copy.setContentHuggingPriority_forOrientation(
        1.0_f32,
        NSLayoutConstraintOrientation::Horizontal,
    );
    copy.setContentCompressionResistancePriority_forOrientation(
        250.0_f32,
        NSLayoutConstraintOrientation::Horizontal,
    );
    pin_beside_glyph(nv(&*inner), copy);

    let row = column(mtm, 0.0, 0.0);
    spacer(&row, HELP_ROW_PAD_Y, mtm);
    arrange(&row, &inner);
    spacer(&row, HELP_ROW_PAD_Y, mtm);
    span_stack(&row, nv(&*inner));
    fill_width(nv(&*row));
    row
}

fn pin_beside_glyph(row: &NSView, copy: &NSView) {
    let inset = HELP_ROW_INSET * 2.0 + HELP_ROW_GAP + HELP_ROW_GLYPH;
    copy.widthAnchor()
        .constraintEqualToAnchor_constant(&row.widthAnchor(), -inset)
        .setActive(true);
}

fn coin_stack(
    sun: &NSImage,
    moon: &NSImage,
    mtm: MainThreadMarker,
) -> (
    Retained<NSView>,
    Retained<CALayer>,
    Retained<CALayer>,
    Retained<CALayer>,
) {
    let coin = NSView::new(mtm);
    coin.setWantsLayer(true);
    nv(&*coin)
        .widthAnchor()
        .constraintEqualToConstant(HERO_IMAGE)
        .setActive(true);
    nv(&*coin)
        .heightAnchor()
        .constraintEqualToConstant(HERO_IMAGE)
        .setActive(true);
    let rotator = transform_layer();
    let sun_face = face_layer(sun);
    let moon_face = face_layer(moon);
    set_rotation_y(&moon_face, std::f64::consts::PI, std::f64::consts::PI, 0.0);
    moon_face.setHidden(true);
    rotator.addSublayer(&sun_face);
    rotator.addSublayer(&moon_face);
    if let Some(host) = backing_layer(&coin) {
        apply_perspective(&host);
        host.addSublayer(&rotator);
    }
    layout_coin_layers(&coin, &rotator, &sun_face, &moon_face);
    (coin, rotator, sun_face, moon_face)
}

fn transform_layer() -> Retained<CALayer> {
    let cls = AnyClass::get(c"CATransformLayer").unwrap_or_else(CALayer::class);
    unsafe { msg_send![cls, layer] }
}

fn face_layer(image: &NSImage) -> Retained<CALayer> {
    let layer: Retained<CALayer> = unsafe { msg_send![CALayer::class(), layer] };
    unsafe {
        let _: () = msg_send![&*layer, setContents: image];
        let _: () = msg_send![&*layer, setContentsGravity: &*ns("resizeAspect")];
        let _: () = msg_send![&*layer, setContentsScale: 2.0];
    }
    layer.setDoubleSided(false);
    layer
}

fn rest_coin_faces(sun: &CALayer, moon: &CALayer, showing_moon: bool) {
    with_actions_disabled(|| {
        sun.setHidden(showing_moon);
        moon.setHidden(!showing_moon);
    });
}

fn set_coin_flip(
    coin: &NSView,
    rotator: &CALayer,
    sun: &CALayer,
    moon: &CALayer,
    flip_done: &CoinFlipDone,
    showing_moon: bool,
    duration: f64,
) {
    let to = hero_flip_radians(showing_moon);
    if duration <= 0.0 {
        flip_done.cancel();
        rest_coin_faces(sun, moon, showing_moon);
        coin.layoutSubtreeIfNeeded();
        with_actions_disabled(|| layout_coin_layers(coin, rotator, sun, moon));
        set_rotation_y(rotator, to, to, 0.0);
        return;
    }
    // Do not relayout during a flip: setBounds/setAnchorPoint queues another transform action.
    with_actions_disabled(|| {
        sun.setHidden(false);
        moon.setHidden(false);
    });
    set_rotation_y(rotator, hero_flip_radians(!showing_moon), to, duration);
    flip_done.schedule(showing_moon, duration);
}

fn set_rotation_y(layer: &CALayer, from: f64, to: f64, duration: f64) {
    let key_path = ns("transform.rotation.y");
    // Read the onscreen pose before removing the previous animation. A quick
    // reversal must not jump back to the previous state's resting face.
    let from = if duration > 0.0 {
        unsafe {
            layer.presentationLayer().map_or(from, |presentation| {
                let value: Retained<AnyObject> =
                    msg_send![&*presentation, valueForKeyPath: &*key_path];
                msg_send![&*value, doubleValue]
            })
        }
    } else {
        from
    };
    let to_num = ns_double(to);
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    layer.removeAnimationForKey(&ns("flip"));
    unsafe {
        let _: () = msg_send![layer, setValue: &*to_num, forKeyPath: &*key_path];
    }
    if duration > 0.0 {
        if let Some(cls) = AnyClass::get(c"CASpringAnimation") {
            unsafe {
                let anim: Retained<AnyObject> = msg_send![cls, animationWithKeyPath: &*key_path];
                let _: () = msg_send![&*anim, setFromValue: &*ns_double(from)];
                let _: () = msg_send![&*anim, setToValue: &*to_num];
                // Near-critical damping gives the coin weight: one restrained
                // overshoot (under one degree), followed by a quiet landing.
                let _: () = msg_send![&*anim, setMass: 1.0_f64];
                let _: () = msg_send![&*anim, setStiffness: 240.0_f64];
                let _: () = msg_send![&*anim, setDamping: 27.0_f64];
                let _: () = msg_send![&*anim, setInitialVelocity: 0.0_f64];
                // Play the complete spring solution within the panel's motion
                // budget, rather than cutting a still-moving spring short.
                let settling: f64 = msg_send![&*anim, settlingDuration];
                let _: () = msg_send![&*anim, setDuration: settling];
                let _: () = msg_send![&*anim, setSpeed: (settling / duration) as f32];
                if let Some(tf_cls) = AnyClass::get(c"CAMediaTimingFunction") {
                    let tf: Retained<AnyObject> =
                        msg_send![tf_cls, functionWithName: &*ns("linear")];
                    let _: () = msg_send![&*anim, setTimingFunction: &*tf];
                }
                let _: () = msg_send![layer, addAnimation: &*anim, forKey: &*ns("flip")];
            }
        }
    }
    CATransaction::commit();
}

fn with_actions_disabled(body: impl FnOnce()) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    body();
    CATransaction::commit();
}

fn set_chrome_appearance(clip: &NSView, active: bool) {
    let name = unsafe {
        if active {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    let Some(appearance) = NSAppearance::appearanceNamed(name) else {
        return;
    };
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    clip.setAppearance(Some(&appearance));
    CATransaction::commit();
    CATransaction::flush();
}

fn layout_coin_layers(coin: &NSView, rotator: &CALayer, sun: &CALayer, moon: &CALayer) {
    let bounds = coin.bounds();
    let size = if bounds.size.width > 0.0 {
        bounds.size.width.min(bounds.size.height)
    } else {
        HERO_IMAGE
    };
    place_square_layer(rotator, size);
    place_square_layer(sun, size);
    place_square_layer(moon, size);
}

fn place_square_layer(layer: &CALayer, size: f64) {
    let bounds = objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(0.0, 0.0),
        objc2_foundation::NSSize::new(size, size),
    );
    layer.setBounds(bounds);
    layer.setAnchorPoint(objc2_foundation::NSPoint::new(0.5, 0.5));
    layer.setPosition(objc2_foundation::NSPoint::new(size / 2.0, size / 2.0));
}

fn apply_perspective(layer: &CALayer) {
    let transform = CATransform3D {
        m11: 1.0,
        m12: 0.0,
        m13: 0.0,
        m14: 0.0,
        m21: 0.0,
        m22: 1.0,
        m23: 0.0,
        m24: 0.0,
        m31: 0.0,
        m32: 0.0,
        m33: 1.0,
        m34: -1.0 / 600.0,
        m41: 0.0,
        m42: 0.0,
        m43: 0.0,
        m44: 1.0,
    };
    layer.setSublayerTransform(transform);
}

fn ns_double(value: f64) -> Retained<AnyObject> {
    let cls = AnyClass::get(c"NSNumber").expect("NSNumber");
    unsafe { msg_send![cls, numberWithDouble: value] }
}

fn decorate_card_shadow(card: &NSView) {
    let Some(layer) = backing_layer(card) else {
        return;
    };
    layer_set_corner_radius(&layer, PANEL_CORNER);
    layer.setMasksToBounds(false);
    layer.setBackgroundColor(Some(&rgb_color(IDLE_FILL_RGB)));
    layer.setShadowOpacity(SHADOW_OPACITY);
    layer_set_shadow_radius(&layer, SHADOW_RADIUS);
    layer_set_shadow_offset(&layer, 0.0, -SHADOW_OFFSET_Y);
    layer.setShadowColor(Some(&cg_color(&NSColor::colorWithRed_green_blue_alpha(
        0.0, 0.0, 0.0, 1.0,
    ))));
}

fn backing_layer(view: &NSView) -> Option<Retained<CALayer>> {
    view.setWantsLayer(true);
    unsafe { msg_send![view, layer] }
}

fn set_fill_color(view: &NSView, rgb: [u8; 3], duration: f64) {
    let Some(layer) = backing_layer(view) else {
        return;
    };
    let color = rgb_color(rgb);
    CATransaction::begin();
    CATransaction::setAnimationDuration(duration);
    CATransaction::setDisableActions(duration <= 0.0);
    layer.setBackgroundColor(Some(&color));
    CATransaction::commit();
}

fn rgb_color(rgb: [u8; 3]) -> Retained<CGColor> {
    cg_color(&NSColor::colorWithRed_green_blue_alpha(
        f64::from(rgb[0]) / 255.0,
        f64::from(rgb[1]) / 255.0,
        f64::from(rgb[2]) / 255.0,
        1.0,
    ))
}

fn cg_color(color: &NSColor) -> Retained<CGColor> {
    unsafe { msg_send![color, CGColor] }
}

fn reduce_motion() -> bool {
    NSUserDefaults::standardUserDefaults().boolForKey(&ns("AppleReduceMotion"))
}

fn layer_set_corner_radius(layer: &CALayer, radius: f64) {
    unsafe {
        let _: () = msg_send![layer, setCornerRadius: radius];
    }
}

fn layer_set_shadow_radius(layer: &CALayer, radius: f64) {
    unsafe {
        let _: () = msg_send![layer, setShadowRadius: radius];
    }
}

fn layer_set_shadow_offset(layer: &CALayer, width: f64, height: f64) {
    let size = objc2_foundation::NSSize::new(width, height);
    unsafe {
        let _: () = msg_send![layer, setShadowOffset: size];
    }
}

/// Encode locally: pairing credentials never leave the app for QR generation.
fn pairing_qr_image(url: &str) -> Option<Retained<NSImage>> {
    if url.is_empty() {
        return None;
    }
    let qr = qrcode::QrCode::new(url).ok()?;
    let scale = 4;
    let width = (qr.width() + 8) * scale;
    let stride = (width * 3 + 3) & !3;
    let size = 54 + stride * width;
    let mut bmp = vec![255u8; size];
    bmp[..54].fill(0);
    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&(size as u32).to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&(width as i32).to_le_bytes());
    bmp[22..26].copy_from_slice(&(-(width as i32)).to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    for y in 0..qr.width() {
        for x in 0..qr.width() {
            if qr[(x, y)] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let offset =
                            54 + ((y + 4) * scale + dy) * stride + ((x + 4) * scale + dx) * 3;
                        bmp[offset..offset + 3].fill(0);
                    }
                }
            }
        }
    }
    load_png(&bmp).ok()
}
