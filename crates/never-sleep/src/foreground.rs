use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use never_sleep_core::{DurationPref, Engine, Input, Lang, PowerPlan, StopReason, HEARTBEAT_MS};

use crate::apply::{dispatch, stop_for_quit};
use crate::ipc::{menu_socket_absent, try_send};
use crate::persist::load_config;
use crate::platform::Platform;
use crate::protocol::{self, IpcRequest};

pub fn run_foreground(
    platform: &mut dyn Platform,
    duration: Option<DurationPref>,
) -> Result<(), String> {
    platform.cleanup_orphans();
    let mut engine = Engine::new(load_config());
    if crate::ipc::should_refuse_foreground_while_menu_live(menu_socket_absent()) {
        return Err(engine.config.tr().menu_ipc_timed_out().into());
    }
    let mut cloud_ok = crate::cloud::cloud_enabled(&engine.config);
    // Claim before the second socket probe so a menu bind cannot skip the lock.
    // `true` is the first probe's conclusion; do not snapshot the socket again.
    let needs_reporter = crate::session_lock::should_claim_foreground_reporter_lock(cloud_ok, true);
    let reporter_claimed = crate::session_lock::try_claim_reporter_lock(std::process::id());
    if crate::session_lock::should_abort_foreground_without_reporter_lock(
        needs_reporter,
        reporter_claimed,
    ) {
        return Err(engine.config.tr().foreground_already_running().into());
    }
    if crate::ipc::should_refuse_foreground_while_menu_live(menu_socket_absent()) {
        crate::session_lock::release_reporter_lock(std::process::id());
        return Err(engine.config.tr().menu_ipc_timed_out().into());
    }
    let input = match duration {
        Some(d) => Input::StartWith(d),
        None => Input::Start,
    };
    dispatch(&mut engine, platform, input);
    if !engine.is_active() {
        if reporter_claimed {
            crate::session_lock::release_reporter_lock(std::process::id());
        }
        return Err(engine.config.tr().foreground_failed().into());
    }

    let running = std::sync::Arc::new(AtomicBool::new(true));
    let r = running.clone();
    let _ = ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    });

    let mut cloud = if reporter_claimed && cloud_ok {
        spawn_foreground_reporter(engine.config.lang(), true)
    } else {
        None
    };
    let mut pairing = None;
    let mut handoff_id: Option<String> = None;
    let mut handoff_seq = 0u64;

    let t = engine.config.tr();
    println!("{}", t.foreground_started());
    println!("{}", t.foreground_status_hint());

    while running.load(Ordering::SeqCst) && engine.is_active() {
        // A menu can change the persisted preference before adopting this session.
        engine.config.remote_enabled = load_config().remote_enabled;
        cloud_ok = crate::cloud::cloud_enabled(&engine.config);
        if !cloud_ok {
            if let Some(handle) = cloud.take() {
                handle.detach();
            }
            pairing = None;
        }
        if let Some(ack) = crate::protocol::read_handoff_ack() {
            if crate::protocol::donor_should_stop_after_successor_gone(
                handoff_id.as_deref(),
                !menu_socket_absent(),
                Some(ack.id.as_str()),
            ) {
                let reason = match ack.outcome {
                    crate::protocol::HandoffAckOutcome::Adopted => StopReason::AppQuit,
                    crate::protocol::HandoffAckOutcome::Stop => StopReason::User,
                };
                if engine.is_active() {
                    dispatch(&mut engine, platform, Input::Stop { reason });
                }
                stop_for_quit(&mut engine, platform);
                if let Some(handle) = take_foreground_reporter(&mut cloud) {
                    if crate::protocol::donor_should_flush_offline_after_ack(ack.reporter) {
                        crate::cloud::publish_and_flush(
                            handle,
                            engine.json_status(&platform.snapshot()),
                            engine.config.lang(),
                        );
                    } else {
                        handle.detach();
                    }
                }
                crate::session_lock::release_reporter_lock(std::process::id());
                crate::protocol::clear_handoff_ack();
                println!("{}", engine.config.tr().foreground_ended());
                return Ok(());
            }
        }
        if let Some(handle) = cloud.as_ref() {
            crate::cloud::apply_polled_commands(&mut engine, platform, handle, &mut pairing);
        }
        if !engine.is_active() {
            break;
        }
        if try_send(&IpcRequest::Ping).is_some() {
            if let Some(handle) = cloud.as_ref() {
                handle.quiesce();
                crate::cloud::apply_polled_commands(&mut engine, platform, handle, &mut pairing);
            }
            if !engine.is_active() {
                break;
            }
            if !running.load(Ordering::SeqCst) {
                break;
            }
            if handoff_id.is_none() {
                handoff_seq += 1;
                handoff_id = Some(crate::protocol::format_handoff_id(
                    std::process::id(),
                    Some(crate::session_lock::process_instance_token(
                        std::process::id(),
                    )),
                    handoff_seq,
                    Some(engine.config.keep_awake_on_lid_close),
                ));
            }
            let req = handoff_request(
                &engine,
                &platform.snapshot(),
                cloud
                    .as_ref()
                    .map(crate::cloud::CloudHandle::applied_command_ids)
                    .unwrap_or_default(),
                handoff_id.clone(),
            );
            if let Some(resp) = try_send(&req) {
                if crate::protocol::should_stop_successor_on_cancel(
                    !running.load(Ordering::SeqCst),
                    true,
                    true,
                ) {
                    if crate::protocol::menu_accepted_handoff(&resp)
                        || crate::protocol::donor_should_stop(&resp)
                    {
                        let _ = try_send(&IpcRequest::Off);
                    }
                    break;
                }
                if crate::protocol::menu_accepted_handoff(&resp) {
                    if let Some(handle) = take_foreground_reporter(&mut cloud) {
                        let successor_reporter = crate::protocol::successor_reporter_after_adopt(
                            resp.reporter,
                            crate::protocol::read_handoff_ack().and_then(|ack| {
                                crate::protocol::matching_handoff_ack_reporter(
                                    handoff_id.as_deref(),
                                    Some(ack.id.as_str()),
                                    ack.reporter,
                                )
                            }),
                        );
                        let successor_reporter =
                            crate::protocol::successor_reporter_after_fresh_ack(
                                successor_reporter,
                                resp.reporter,
                                crate::protocol::read_handoff_ack().and_then(|ack| {
                                    crate::protocol::matching_handoff_ack_reporter(
                                        handoff_id.as_deref(),
                                        Some(ack.id.as_str()),
                                        ack.reporter,
                                    )
                                }),
                            );
                        if crate::protocol::donor_should_flush_offline_after_ack(successor_reporter)
                        {
                            crate::cloud::publish_and_flush(
                                handle,
                                engine.json_status(&platform.snapshot()),
                                engine.config.lang(),
                            );
                        } else {
                            handle.detach();
                        }
                    }
                    crate::session_lock::release_reporter_lock(std::process::id());
                    if engine.is_active() {
                        dispatch(
                            &mut engine,
                            platform,
                            Input::Stop {
                                reason: StopReason::AppQuit,
                            },
                        );
                    }
                    stop_for_quit(&mut engine, platform);
                    println!("{}", engine.config.tr().foreground_ended());
                    return Ok(());
                }
                if crate::protocol::donor_should_stop(&resp) {
                    if engine.is_active() {
                        dispatch(
                            &mut engine,
                            platform,
                            Input::Stop {
                                reason: StopReason::User,
                            },
                        );
                    }
                    stop_for_quit(&mut engine, platform);
                    if let Some(handle) = take_foreground_reporter(&mut cloud) {
                        handle.detach();
                    }
                    crate::session_lock::release_reporter_lock(std::process::id());
                    println!("{}", engine.config.tr().foreground_ended());
                    return Ok(());
                }
                if crate::session_lock::should_reapply_donor_clamshell(
                    engine.is_active(),
                    crate::protocol::donor_should_reapply_clamshell(&resp),
                ) {
                    let host = platform.snapshot();
                    let plan = PowerPlan::for_session(&engine.config, host.on_ac);
                    let _ = platform.apply_power(plan);
                }
            }
            if let Some(handle) = cloud.as_ref() {
                handle.resume();
            }
        } else {
            if crate::cloud::should_release_applied_retention(!menu_socket_absent()) {
                if let Some(handle) = cloud.as_ref() {
                    handle.release_applied_retention();
                }
            }
            if cloud.is_none() && cloud_ok && menu_socket_absent() {
                cloud = spawn_foreground_reporter(engine.config.lang(), false);
            }
        }
        dispatch(&mut engine, platform, Input::Tick);
        if crate::session_lock::take_pending_donor_clamshell_reapply(engine.is_active()) {
            let host = platform.snapshot();
            let plan = PowerPlan::for_session(&engine.config, host.on_ac);
            let _ = platform.apply_power(plan);
        }
        if let Some(handle) = cloud.as_ref() {
            crate::cloud::sync_cloud(&mut engine, platform, handle, &mut pairing);
        }
        if !engine.is_active() {
            break;
        }
        thread::sleep(Duration::from_millis(HEARTBEAT_MS));
    }

    if crate::protocol::should_stop_successor_on_cancel(
        !running.load(Ordering::SeqCst),
        handoff_id.is_some(),
        !menu_socket_absent(),
    ) {
        let _ = try_send(&IpcRequest::Off);
    }

    if engine.is_active() {
        dispatch(
            &mut engine,
            platform,
            Input::Stop {
                reason: StopReason::User,
            },
        );
    }
    stop_for_quit(&mut engine, platform);
    if let Some(handle) = take_foreground_reporter(&mut cloud) {
        crate::cloud::publish_and_flush(
            handle,
            engine.json_status(&platform.snapshot()),
            engine.config.lang(),
        );
    }
    crate::session_lock::release_reporter_lock(std::process::id());
    println!("{}", engine.config.tr().foreground_ended());
    Ok(())
}

fn handoff_request(
    engine: &Engine,
    host: &never_sleep_core::HostSnapshot,
    applied_command_ids: Vec<String>,
    handoff_id: Option<String>,
) -> IpcRequest {
    let status = engine.json_status(host);
    let req = IpcRequest::handoff(
        Some(crate::protocol::duration_pref_to_ipc(
            engine.config.duration,
        )),
        status.remaining_secs,
        status.elapsed_secs,
    )
    .with_applied_command_ids(applied_command_ids);
    match handoff_id {
        Some(id) => req.with_handoff_id(id),
        None => req,
    }
}

fn take_foreground_reporter(
    cloud: &mut Option<crate::cloud::CloudHandle>,
) -> Option<crate::cloud::CloudHandle> {
    cloud.take()
}

fn spawn_foreground_reporter(
    lang: Lang,
    already_claimed: bool,
) -> Option<crate::cloud::CloudHandle> {
    if !already_claimed && !crate::session_lock::try_claim_reporter_lock(std::process::id()) {
        return None;
    }
    match crate::cloud::load_or_create_identity() {
        Ok(identity) => Some(crate::cloud::spawn_reporter(
            identity,
            crate::cloud::default_display_name(),
            lang,
        )),
        Err(err) => {
            eprintln!("never-sleep cloud identity: {err}");
            None
        }
    }
}

pub fn parse_optional_duration(
    raw: Option<&str>,
    lang: Lang,
) -> Result<Option<DurationPref>, String> {
    protocol::parse_on_duration_in(raw, lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_observes_remote_opt_out_while_menu_is_taking_over() {
        let source = include_str!("foreground.rs");
        let loop_body = source
            .split("while running.load")
            .nth(1)
            .unwrap()
            .split("fn spawn_foreground_reporter")
            .next()
            .unwrap();
        assert!(loop_body.contains("load_config().remote_enabled"));
        assert!(loop_body.contains("handle.detach()"));
    }

    #[test]
    fn parse_optional_duration_passthrough() {
        assert_eq!(parse_optional_duration(None, Lang::En).unwrap(), None);
        assert_eq!(
            parse_optional_duration(Some("1h"), Lang::En).unwrap(),
            Some(DurationPref::Hours { hours: 1 })
        );
        assert!(parse_optional_duration(Some("0h"), Lang::En).is_err());
    }

    #[test]
    fn foreground_does_not_spawn_a_second_cloud_reporter() {
        let src = include_str!("foreground.rs");
        let start = src.find("pub fn run_foreground").expect("run_foreground");
        let body = src[start..]
            .split("pub fn parse_optional_duration")
            .next()
            .unwrap();
        assert!(
            body.contains("try_send") && body.contains("IpcRequest::Ping"),
            "foreground must not spawn a reporter while the menu process is already serving IPC"
        );
        assert!(
            body.contains("should_refuse_foreground_while_menu_live")
                && body.contains("menu_ipc_timed_out"),
            "a live ipc.sock after a timed-out CLI On must not dispatch a second local Start"
        );
        assert!(
            body.contains("publish_and_flush") && body.contains("take()"),
            "a later menu launch must stop the foreground reporter so only one process heartbeats"
        );
        let spawn = body
            .split("fn spawn_foreground_reporter")
            .nth(1)
            .expect("spawn_foreground_reporter")
            .split("pub fn parse_optional_duration")
            .next()
            .unwrap();
        let err_arm = spawn.split("Err(err)").nth(1).expect("identity Err arm");
        assert!(
            !err_arm.contains("release_reporter_lock"),
            "identity load failure must keep reporter.lock so a second on cannot Start"
        );
    }

    #[test]
    fn foreground_hands_off_engine_when_menu_appears() {
        let src = include_str!("foreground.rs");
        let start = src.find("pub fn run_foreground").expect("run_foreground");
        let body = src[start..]
            .split("pub fn parse_optional_duration")
            .next()
            .unwrap();
        let ping_at = body
            .find("IpcRequest::Ping")
            .expect("menu presence is detected with Ping");
        let after_ping = &body[ping_at..];
        let handoff = after_ping
            .split("return Ok(())")
            .next()
            .expect("handoff returns after a successful adopt");
        assert!(
            handoff.contains("handoff_request"),
            "hand the leftover duration, not the original Hours preference"
        );
        assert!(
            src.contains("remaining_secs") && src.contains("elapsed_secs"),
            "handoff IPC must carry leftover and elapsed seconds of the live session"
        );
        let loop_at = body.find("while running").expect("foreground loop");
        let loop_body = &body[loop_at..];
        let drain_at = loop_body
            .find("apply_polled_commands")
            .expect("drain the foreground reporter before handing off");
        let ack_at = loop_body
            .find("read_handoff_ack")
            .expect("read persisted adopt/stop before contacting a replacement menu");
        assert!(
            ack_at < drain_at,
            "a matching ack must stop this donor before it applies a phone Off that the menu still owns"
        );
        let quiesce_at = loop_body
            .find("quiesce(")
            .expect("quiesce the reporter so an in-flight heartbeat cannot ack during IPC");
        let adopt_at = loop_body
            .find("handoff_request")
            .expect("handoff_request after drain");
        assert!(
            drain_at < quiesce_at,
            "apply queued commands before waiting for the in-flight heartbeat"
        );
        assert!(
            quiesce_at < adopt_at,
            "a heartbeat that returns Off during Ping must be drained before detach()"
        );
        let after_quiesce = &loop_body[quiesce_at..];
        let second_drain = after_quiesce
            .find("apply_polled_commands")
            .expect("drain commands delivered by the in-flight heartbeat");
        let adopt_after = after_quiesce
            .find("handoff_request")
            .expect("handoff after the post-quiesce drain");
        assert!(
            second_drain < adopt_after,
            "transfer the last phone Off before relinquishing the reporter"
        );
        assert!(
            handoff.contains("menu_accepted_handoff"),
            "keep the foreground session unless the menu reports ok && active"
        );
        assert!(
            handoff.contains("successor_reporter_after_adopt")
                && handoff.contains("successor_reporter_after_fresh_ack")
                && handoff.contains("matching_handoff_ack_reporter")
                && handoff.contains("resp.reporter")
                && handoff.contains("detach(")
                && handoff.contains("publish_and_flush")
                && handoff.contains("donor_should_flush_offline_after_ack"),
            "adopt must detach only when a successor reporter remains; otherwise flush"
        );
        assert!(
            handoff.contains("StopReason::AppQuit") && handoff.contains("stop_for_quit"),
            "handoff must release assertions silently; AppQuit does not notify 待命已结束"
        );
        assert!(
            !handoff.contains("StopReason::User"),
            "User stop would show an ended notification after the menu already said started"
        );
        assert!(
            after_ping.contains("return Ok(())"),
            "exit the fallback loop after handoff so Tick cannot re-apply assertions"
        );
        let accept_at = loop_body
            .find("menu_accepted_handoff")
            .expect("accepted handoff");
        let resume_at = loop_body
            .find("resume(")
            .expect("unpark the reporter when the menu does not take over");
        assert!(
            accept_at < resume_at,
            "only resume after the adopt decision, never instead of quiesce"
        );
        let after_accept = &loop_body[loop_body.find("return Ok(())").expect("success return")..];
        assert!(
            after_accept.contains("resume("),
            "Ping-then-gone or a rejected On must resume heartbeats; paused has no other exit"
        );
        let between = &loop_body[accept_at..resume_at];
        assert!(
            between.contains("donor_should_stop") && between.contains("StopReason::User"),
            "failed adopt with a deferred Off must stop this donor before resume"
        );
        let stop_arm = loop_body
            .split("if crate::protocol::donor_should_stop(&resp)")
            .nth(1)
            .expect("stop_donor arm")
            .split("resume(")
            .next()
            .unwrap();
        assert!(
            stop_arm.contains("detach(") && !stop_arm.contains("publish_and_flush"),
            "stop_donor must detach so the surviving menu reporter is not marked offline"
        );
        assert!(
            loop_body.contains("handoff_id")
                && loop_body.contains("handoff_seq")
                && loop_body.contains("format_handoff_id")
                && loop_body.contains("process_instance_token")
                && loop_body.contains("keep_awake_on_lid_close"),
            "handoff ids must include a process-start token and the donor clamshell bit"
        );
        assert!(
            loop_body.contains("should_stop_successor_on_cancel")
                && loop_body.contains("IpcRequest::Off"),
            "Ctrl-C during handoff must Off a successor that may already have adopted"
        );
        assert!(
            loop_body.contains("release_applied_retention")
                && loop_body.contains("should_release_applied_retention")
                && loop_body.contains("menu_socket_absent"),
            "clear retained command ids only after the successor socket is gone"
        );
        assert!(
            loop_body.contains("donor_should_stop_after_successor_gone")
                && loop_body.contains("read_handoff_ack"),
            "a lost adopt reply then menu Quit must stop this donor from a persisted ack, not Tick"
        );
        let ping_at = loop_body.find("IpcRequest::Ping").expect("loop Ping");
        assert!(
            ack_at < ping_at,
            "a matching ack must stop this donor before it can hand off to a freshly launched menu"
        );
        assert!(
            loop_body.contains("donor_should_flush_offline_after_ack")
                && loop_body.contains("ack.reporter")
                && loop_body.contains("publish_and_flush"),
            "ack stop must flush when the successor ack says no reporter remains"
        );
    }

    #[test]
    fn transient_menu_gone_starts_a_reporter() {
        let src = include_str!("foreground.rs");
        let start = src.find("pub fn run_foreground").expect("run_foreground");
        let body = src[start..]
            .split("pub fn parse_optional_duration")
            .next()
            .unwrap();
        let loop_at = body.find("while running").expect("foreground loop");
        let loop_body = &body[loop_at..];
        let ping_at = loop_body
            .find("IpcRequest::Ping")
            .expect("loop Ping decides whether a menu is live");
        let after_ping = &loop_body[ping_at..];
        assert!(
            after_ping.contains("spawn_foreground_reporter")
                || after_ping.contains("spawn_reporter"),
            "Ping failure with no live menu must create a reporter so the phone board can recover"
        );
        assert!(
            after_ping.contains("cloud.is_none()"),
            "do not start a second reporter while this process already has one"
        );
        assert!(
            after_ping.contains("menu_socket_absent"),
            "a timed-out Ping while the menu still owns the socket must not start a second reporter"
        );
    }

    #[test]
    fn handoff_request_sends_remaining_secs_not_full_pref() {
        use never_sleep_core::{AppConfig, DurationPref, HostSnapshot, Input, Thermal};
        let mut engine = Engine::new(AppConfig {
            duration: DurationPref::Hours { hours: 8 },
            display_off_delay_ms: 1_500,
            ..AppConfig::default()
        });
        let host = HostSnapshot {
            monotonic_ms: 0,
            continuous_ms: 0,
            unix_secs: 1_700_000_000,
            utc_offset_secs: 0,
            on_ac: true,
            battery_percent: Some(80),
            lid_closed: false,
            display_asleep: Some(false),
            hid_idle_ms: 60_000,
            thermal: Thermal::Nominal,
        };
        engine.handle(Input::StartWith(DurationPref::Hours { hours: 8 }), &host);
        let mut later = host.clone();
        later.monotonic_ms = 7 * 3_600_000;
        later.continuous_ms = 7 * 3_600_000;
        later.unix_secs = host.unix_secs;
        let req = handoff_request(&engine, &later, vec!["phone-on".into()], None);
        match req {
            IpcRequest::On {
                duration,
                remaining_secs,
                elapsed_secs,
                handoff,
                applied_command_ids,
                handoff_id,
            } => {
                assert_eq!(duration.as_deref(), Some("8h"));
                assert_eq!(remaining_secs, Some(3600));
                assert_eq!(elapsed_secs, Some(7 * 3600));
                assert!(handoff);
                assert_eq!(applied_command_ids, vec!["phone-on"]);
                assert!(handoff_id.is_none());
            }
            other => panic!("expected On handoff, got {other:?}"),
        }
    }

    #[test]
    fn foreground_joins_reporter_after_final_status() {
        let src = include_str!("foreground.rs");
        let start = src.find("pub fn run_foreground").expect("run_foreground");
        let body = src[start..]
            .split("pub fn parse_optional_duration")
            .next()
            .unwrap();
        assert!(
            body.contains("publish_and_flush") || body.contains("flush_and_join"),
            "process exit must wait for the inactive heartbeat POST"
        );
    }
}
