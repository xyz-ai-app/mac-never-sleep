//! Phone board pages and the Cloudflare Worker that hosts them.
//!
//! Public origin is `https://xyz-ai.app/never-sleep/`. The gateway already
//! binds Worker `mac-never-sleep` and strips `/never-sleep`, so API paths on
//! the Worker are `/api/...` while the phone fetches `/never-sleep/api/...`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const SITE_ORIGIN: &str = "https://xyz-ai.app/never-sleep";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("missing {rel}: {err}"))
}

#[test]
fn wrangler_worker_is_the_existing_mac_never_sleep_service() {
    let wrangler = read("wrangler.jsonc");
    assert!(
        wrangler.contains(r#""name": "mac-never-sleep""#),
        "must keep the gateway Service Binding name"
    );
    assert!(wrangler.contains(r#""directory": "./site""#));
    assert!(wrangler.contains("run_worker_first"));
    assert!(wrangler.contains("/api/*"));
    assert!(wrangler.contains("BoardHub"));
    assert!(
        wrangler.contains("\"workers_dev\": false"),
        "workers.dev must not bypass the pair/start rate gate"
    );
    assert!(wrangler.contains("\"preview_urls\": false"));
}

#[test]
fn wrangler_creates_boardhub_with_sqlite_backend_for_free_plan() {
    let wrangler = read("wrangler.jsonc");
    assert!(
        wrangler.contains(r#""new_sqlite_classes": ["BoardHub"]"#),
        "Workers Free only allows SQLite-backed Durable Objects; BoardHub must be created with new_sqlite_classes"
    );
    assert!(
        !wrangler.contains(r#""new_classes""#),
        "new_classes creates a KV-backed namespace and fails on the Free plan (API code 10097)"
    );
}

#[test]
fn board_pages_are_mobile_first_and_bilingual() {
    let en = read("site/board/index.html");
    let zh = read("site/zh/board/index.html");
    assert!(en.contains("<html lang=\"en\""));
    assert!(zh.contains("<html lang=\"zh-Hans\""));
    assert!(en.contains("Start Screen-Off Standby") || en.contains("board.js"));
    assert!(en.contains("noindex"));
    assert!(zh.contains("noindex"));
    assert_eq!(
        en.split(r#"rel="canonical" href=""#)
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap(),
        format!("{SITE_ORIGIN}/board/")
    );
    assert!(en.contains(r#"href="../assets/style.css""#));
    assert!(zh.contains(r#"href="../../assets/style.css""#));
    assert!(en.contains("Start or End Screen-Off Standby") || en.contains("will not fight"));
    assert!(zh.contains("开始") && zh.contains("结束关屏待命"));
}

#[test]
fn board_client_uses_public_prefix_and_has_no_unauthenticated_toggle() {
    let js = read("site/assets/board.js");
    assert!(js.contains("/never-sleep/api"));
    assert!(js.contains("function apiBase"));
    assert!(js.contains("Start Screen-Off Standby"));
    assert!(js.contains("开始关屏待命"));
    assert!(js.contains("End Standby"));
    assert!(js.contains("结束待命"));
    assert!(js.contains("device_token"));
    assert!(js.contains("/command"));
    assert!(!js.contains("cmd: \"toggle\""));
    assert!(!js.contains("\"cmd\":\"quit\""));
    assert!(
        !js.contains("innerHTML"),
        "untrusted heartbeat fields must not go through innerHTML"
    );
    assert!(js.contains("textContent"));
    assert!(js.contains("networkError"));
    assert!(
        !js.contains("st.online && st.active"),
        "offline Macs must keep last-known standby, not paint Standby off"
    );
    assert!(js.contains("st.active ? copy.standbyOn : copy.standbyOff"));
    assert!(
        js.contains("function withListFailure"),
        "list transport failures must mark cached devices offline"
    );
    assert!(js.contains("withListFailure(lastStatuses)"));
    assert!(js.contains("LIST_MAX_DEVICES = 32"));
    assert!(js.contains("while (next.length > LIST_MAX_DEVICES)"));
    assert!(js.contains("next.shift()"));
    assert!(js.contains("function hrefWithPairingCode"));
    assert!(js.contains("waitForClaimThenLanguageHref"));
    assert!(js.contains("preventDefault"));
    assert!(js.contains("clearPairingQuery"));
    assert!(js.contains("history.replaceState"));
    assert!(js.contains("function mergeListStatuses"));
    assert!(js.contains("claimInFlight"));
    assert!(js.contains("function devicePendingCmd"));
    assert!(js.contains("pendingByDevice"));
    assert!(js.contains("function nextRefreshGeneration"));
    assert!(js.contains("isCurrentRefresh"));
    assert!(js.contains("refreshInFlight"));
    assert!(js.contains("refreshQueued"));
    assert!(js.contains("void refresh()"));
    assert!(!js.contains("} while (refreshQueued);"));
    assert!(js.contains("function canStoreClaim"));
    assert!(js.contains("storageCanHold"));
    assert!(js.contains("claimReservation"));
    assert!(js.contains("MAX_DISPLAY_NAME_CHARS = 128"));
    assert!(js.contains("function boundDisplayName"));
    assert!(js.contains("\\u0000"));
    assert!(js.contains("storageError"));
    assert!(
        !js.contains("padStart(2, \"0\")}:00"),
        "hour-long remaining must keep live seconds, not hard-code :00"
    );
}

#[test]
fn worker_board_logic_is_tested_in_node() {
    let status = Command::new("node")
        .args(["--test", "worker/test/board.test.js"])
        .current_dir(root())
        .status()
        .expect("node --test");
    assert!(status.success(), "worker/test/board.test.js must pass");
}

#[test]
fn worker_shards_per_device_not_one_global_board() {
    let index = read("worker/src/index.js");
    assert!(
        !index.contains("idFromName(\"board\")"),
        "a single named DO serializes every home onto one queue"
    );
    assert!(index.contains("device:"));
    assert!(index.contains("pair:"));
    assert!(index.contains("shardName"));
    assert!(index.contains("expired_codes"));
    assert!(index.contains("capListEntries"));
    assert!(index.contains("persistBoardAction"));
    assert!(index.contains("publishReservedPairing"));
    assert!(index.contains("blockConcurrencyWhile"));
    assert!(index.contains("createSerialQueue"));
    assert!(index.contains("setAlarm"));
    assert!(index.contains("alarm()"));
    let board = read("worker/src/board.js");
    assert!(board.contains("pairingCodeIsLive"));
    assert!(board.contains("LIST_MAX_DEVICES = 32"));
    assert!(board.contains("DEVICE_ID_LEN"));
    assert!(board.contains("deviceCredentialsAreValid"));
    let shard_fn = board
        .split("export function shardName")
        .nth(1)
        .expect("shardName")
        .split("export const LIST_MAX_DEVICES")
        .next()
        .unwrap();
    assert!(
        shard_fn.contains("deviceCredentialsAreValid")
            && !shard_fn.contains("isHexLen(id, DEVICE_ID_LEN)"),
        "heartbeat/command must reject non 32/64-hex credentials before idFromName"
    );
    let cap_fn = board
        .split("export function capListEntries")
        .nth(1)
        .expect("capListEntries")
        .split("export function fitStoredDevices")
        .next()
        .unwrap();
    assert!(
        cap_fn.contains("deviceCredentialsAreValid"),
        "/api/list must reject non 32/64-hex credentials before idFromName"
    );
    assert!(board.contains("COMMAND_TTL_SECS"));
    assert!(board.contains("fitStoredDevices"));
    assert!(board.contains("expireOffers"));
    assert!(board.contains("#dropUnverifiedDevices") || board.contains("lastSeen != null"));
    assert!(board.contains("collectListParts"));
    assert!(index.contains("collectListParts"));
    assert!(board.contains("bestEffortCleanup"));
    assert!(index.contains("bestEffortCleanup"));
    assert!(board.contains("alarmNeedsUpdate"));
    assert!(index.contains("rate:pair-start"));
    assert!(index.contains("rate:list"));
    assert!(index.contains("internal/list-rate"));
    let list_rate_at = index.find("internal/list-rate").expect("list-rate gate");
    let list_fanout_at = index.find("await collectListParts").expect("list fan-out");
    assert!(
        list_rate_at < list_fanout_at,
        "do not fan out /api/list before the rate gate"
    );
    assert!(index.contains("rate:pair-claim"));
    assert!(index.contains("internal/claim-rate"));
    assert!(index.contains("rate:device"));
    assert!(index.contains("internal/device-rate"));
    let after_pair_start = index
        .split(r#"started.error || "pair_busy""#)
        .nth(1)
        .expect("pair/start catch-all");
    let catch_all_shard = after_pair_start
        .find("const name = shardName(path, body);")
        .expect("device catch-all shardName");
    let catch_all = &after_pair_start[..catch_all_shard];
    assert!(
        catch_all.contains("/api/heartbeat") && catch_all.contains("/api/command"),
        "unknown /api paths must not open device shards"
    );
    let not_found_at = catch_all.find("not_found").expect("unknown path 404");
    let device_rate_at = catch_all.find("rate:device").expect("device rate gate");
    assert!(
        not_found_at < device_rate_at,
        "unknown /api paths must 404 before the device rate shard"
    );
    let claim_block = index
        .split(r#"path === "/api/pair/claim""#)
        .nth(1)
        .expect("pair/claim route");
    let claim_rate_at = claim_block
        .find("rate:pair-claim")
        .expect("claim rate gate");
    let claim_peek_at = claim_block.find("internal/pair-peek").expect("pair-peek");
    assert!(
        claim_rate_at < claim_peek_at,
        "do not open pair shards before the claim rate gate"
    );
    let client = read("site/assets/board.js");
    assert!(
        client.contains("setInterval(refresh, 2500)"),
        "list global cap is sized from this poll interval"
    );
    assert!(board.contains("LIST_GLOBAL_MIN_BOARDS"));
    assert!(index.contains("clientIp"));
    assert!(board.contains("takePairStartSlot"));
    assert!(board.contains("takeListSlot"));
    assert!(board.contains("takeClaimSlot"));
    assert!(board.contains("takeDeviceSlot"));
    assert!(
        board.contains("LIST_IP_MIN_BOARDS"),
        "per-IP list cap must be sized for several boards behind one NAT"
    );
    assert!(
        board.contains("DEVICE_HEARTBEAT_INTERVAL_MS = 1000")
            || board.contains("DEVICE_ACTIVE_TICK_MS = 1000"),
        "device caps must follow the active one-second panel clock"
    );
    assert!(
        board.contains("bucketTake") || board.contains("windowStart"),
        "rate-limit state must use bounded buckets, not unbounded timestamp arrays"
    );
    let cloud_src = read("crates/never-sleep/src/cloud.rs");
    assert!(
        cloud_src.contains("expires_unix"),
        "CloudEvent::Pairing must carry expires_unix so stale codes are not displayed"
    );
    assert!(
        board.contains("expires_unix: offer.expires")
            || board.contains("expires_unix: pairing?.expires_unix"),
        "heartbeat must return the stored offer deadline, not a fresh TTL"
    );
    assert!(
        board.contains("offline === true"),
        "quit heartbeats must mark the Mac offline before the TTL"
    );
    assert!(
        client.contains("retryLater") && client.contains("稍后再试"),
        "claim 429 must not reuse the invalid-code copy"
    );
    let pairing = board
        .split("export async function publishReservedPairing")
        .nth(1)
        .expect("publishReservedPairing")
        .split("function asBool")
        .next()
        .unwrap();
    assert!(
        pairing.contains("reserved = await reserve(code)")
            && pairing.contains("bestEffortCleanup")
            && pairing.contains("continue;"),
        "setAlarm rejection after pair-offer persist must drop the shard"
    );
    let started = pairing
        .split("started = await startDevice")
        .nth(1)
        .expect("startDevice")
        .split("if (started?.ok)")
        .next()
        .unwrap();
    assert!(
        started.contains("forgotten")
            && started.contains("if (forgotten)")
            && started.contains("drop(code)")
            && started.contains("forget(code)"),
        "ambiguous startDevice failure must drop the device offer before the pair shard"
    );
    let confirm = pairing
        .split("if (confirmLive)")
        .nth(1)
        .expect("confirmLive");
    assert!(
        confirm.contains("catch")
            && confirm.contains("forgotten")
            && confirm.contains("if (forgotten)")
            && confirm.contains("drop(code)")
            && confirm.contains("forget(code)"),
        "live-pairing confirmation failures must drop the pair shard only after the device offer is gone"
    );
    assert!(board.contains("rate_limited"));
    assert!(board.contains("commitAlarmUnix"));
    assert!(board.contains("commitPersistedAlarm"));
    assert!(board.contains("MAX_DISPLAY_NAME_CHARS"));
    assert!(board.contains("boundDisplayName"));
    let persist = index
        .split("async #persist()")
        .nth(1)
        .expect("#persist must exist");
    assert!(
        persist.contains("catch") && persist.contains("Board.fromJSON(this.stored)"),
        "persist failures must restore the live board from the last stored snapshot"
    );
    assert!(index.contains("bestEffortCleanup(res, () =>"));
    assert!(board.contains("confirmLive"));
    assert!(board.contains("isAllowedDuration"));
    assert!(board.contains("error: \"taken\""));
    assert!(
        !board.contains("7 * 24 * 3600") && !board.contains("604800"),
        "status counters follow JsonStatus u64, not a seven-day ceiling"
    );
    let cloud = read("crates/never-sleep/src/cloud.rs");
    assert!(cloud.contains("retain_pending"));
    assert!(cloud.contains("take_latest"));
    assert!(cloud.contains("should_reporter_tick"));
    assert!(cloud.contains("bound_display_name"));
    assert!(
        !cloud.contains("const CAP: usize = 64"),
        "delivered command ids must not be truncated before ack"
    );
}

#[test]
fn mac_display_name_uses_gethostname_not_etc_hostname() {
    let cloud = read("crates/never-sleep/src/cloud.rs");
    assert!(cloud.contains("libc::gethostname"));
    assert!(
        !cloud.contains("\"/etc/hostname\""),
        "LaunchServices GUI sessions do not have /etc/hostname"
    );
    assert!(!cloud.contains("\"/proc/sys/kernel/hostname\""));
}

#[test]
fn readme_describes_pairing_and_remote_standby() {
    let en = read("README.md");
    let zh = read("README.zh-CN.md");
    assert!(en.contains("## Phone board"));
    assert!(zh.contains("## 手机看板"));
    assert!(en.contains("never-sleep pair"));
    assert!(zh.contains("never-sleep pair"));
    assert!(en.contains("Start Screen-Off Standby"));
    assert!(en.contains("did not apply"));
    assert!(zh.contains("不会假装"));
    assert!(en.contains(&format!("{SITE_ORIGIN}/board/")));
    assert!(zh.contains(&format!("{SITE_ORIGIN}/zh/board/")));
}

#[test]
fn pairing_and_home_offer_direct_actions() {
    let native = read("crates/never-sleep/src/native_panel.rs");
    assert!(
        native.contains("copyPairing:"),
        "pairing needs a copy action"
    );
    assert!(
        native.contains("pairing_qr"),
        "pairing needs a local QR image"
    );
    assert!(
        native.contains("quick_duration"),
        "home needs a duration shortcut"
    );
    let js = read("site/assets/board.js");
    assert!(
        js.contains("device-coin"),
        "phone uses the app state illustration"
    );
    assert!(js.contains("startBtn.hidden = st.active"));
    assert!(js.contains("endBtn.hidden = !st.active"));
}
