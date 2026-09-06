import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  Board,
  capListEntries,
  collectListParts,
  COMMAND_TTL_SECS,
  createSerialQueue,
  fitStoredDevices,
  handleApi,
  HEARTBEAT_TTL_SECS,
  isAllowedDuration,
  LIST_MAX_DEVICES,
  PAIRING_TTL_SECS,
  pairingCodeIsLive,
  pairingUrl,
  persistBoardAction,
  publicSiteOrigin,
  publishReservedPairing,
  sanitizeStatus,
  shardName,
  alarmNeedsUpdate,
  bestEffortCleanup,
  boundDisplayName,
  clientIp,
  commitAlarmUnix,
  commitPersistedAlarm,
  MAX_DISPLAY_NAME_CHARS,
  PAIR_START_GLOBAL_LIMIT,
  PAIR_START_IP_LIMIT,
  LIST_IP_LIMIT,
  LIST_GLOBAL_LIMIT,
  LIST_IP_MIN_BOARDS,
  LIST_GLOBAL_MIN_BOARDS,
  PAIR_CLAIM_IP_LIMIT,
  PAIR_CLAIM_GLOBAL_LIMIT,
  DEVICE_IP_LIMIT,
  DEVICE_GLOBAL_LIMIT,
  DEVICE_HEARTBEAT_INTERVAL_MS,
  DEVICE_IP_MIN_MACS,
  DEVICE_GLOBAL_MIN_MACS,
  DEVICE_ID_LEN,
  DEVICE_TOKEN_LEN,
  deviceCredentialsAreValid,
  takePairStartSlot,
  takeListSlot,
  takeClaimSlot,
  takeDeviceSlot,
  RATE_IP_MAP_MAX,
} from "../src/board.js";

function sampleStatus(overrides = {}) {
  return {
    active: false,
    display: "awake",
    lid: "open",
    on_ac: true,
    battery: 64,
    remaining_secs: null,
    user_present: true,
    elapsed_secs: null,
    stop_reason: null,
    stop_reason_code: null,
    screen_off_enabled: true,
    lid_awake_enabled: true,
    ...overrides,
  };
}

function identity() {
  return {
    device_id: "ab".repeat(16),
    device_token: "cd".repeat(32),
    display_name: "Studio",
  };
}

async function post(board, path, body) {
  return handleApi(
    board,
    new Request(`https://xyz-ai.app${path}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

async function json(res) {
  return { status: res.status, body: await res.json() };
}

test("pairing creates a device the list endpoint returns", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  const started = await json(
    await post(board, "/api/pair/start", {
      device_id: id.device_id,
      device_token: id.device_token,
      display_name: id.display_name,
    }),
  );
  assert.equal(started.status, 200);
  assert.equal(started.body.ok, true);
  assert.match(started.body.pairing_code, /^[0-9A-Z]{4}-[0-9A-Z]{4}$/);
  assert.ok(started.body.pairing_url.includes("/board/"));

  const claimed = await json(
    await post(board, "/api/pair/claim", {
      pairing_code: started.body.pairing_code,
    }),
  );
  assert.equal(claimed.status, 200);
  assert.equal(claimed.body.device_id, id.device_id);
  assert.equal(claimed.body.device_token, id.device_token);

  const listed = await json(
    await post(board, "/api/list", {
      devices: [
        { device_id: id.device_id, device_token: id.device_token },
      ],
    }),
  );
  assert.equal(listed.body.devices.length, 1);
  assert.equal(listed.body.devices[0].display_name, "Studio");
  assert.equal(listed.body.devices[0].device_id, id.device_id);
  assert.equal(listed.body.devices[0].online, false);
});

test("heartbeat auth rejects bad tokens", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  await post(board, "/api/pair/start", id);
  const bad = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: "ff".repeat(32),
      status: sampleStatus(),
    }),
  );
  assert.equal(bad.status, 401);
  assert.equal(bad.body.ok, false);
  const good = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      display_name: "Studio",
      status: sampleStatus({ active: true, display: "asleep" }),
    }),
  );
  assert.equal(good.status, 200);
  assert.equal(good.body.ok, true);
});

test("offline after heartbeat TTL", async () => {
  let now = 1_000;
  const board = new Board(() => now);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus({ active: true }),
  });
  now = 1_000 + HEARTBEAT_TTL_SECS;
  let listed = await json(
    await post(board, "/api/list", {
      devices: [{ device_id: id.device_id, device_token: id.device_token }],
    }),
  );
  assert.equal(listed.body.devices[0].online, true);
  assert.equal(listed.body.devices[0].active, true);
  now = 1_000 + HEARTBEAT_TTL_SECS + 1;
  listed = await json(
    await post(board, "/api/list", {
      devices: [{ device_id: id.device_id, device_token: id.device_token }],
    }),
  );
  assert.equal(listed.body.devices[0].online, false);
  assert.equal(
    listed.body.devices[0].active,
    true,
    "last status is retained for last-seen; the phone must key off online",
  );
});

test("shutdown heartbeat marks the Mac offline immediately", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus({ active: false }),
  });
  let listed = await json(
    await post(board, "/api/list", {
      devices: [{ device_id: id.device_id, device_token: id.device_token }],
    }),
  );
  assert.equal(listed.body.devices[0].online, true);

  const gone = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus({ active: false }),
      offline: true,
    }),
  );
  assert.equal(gone.body.ok, true);
  listed = await json(
    await post(board, "/api/list", {
      devices: [{ device_id: id.device_id, device_token: id.device_token }],
    }),
  );
  assert.equal(
    listed.body.devices[0].online,
    false,
    "a quit heartbeat must not keep the Mac online for the TTL",
  );
  const cmd = await json(
    await post(board, "/api/command", {
      device_id: id.device_id,
      device_token: id.device_token,
      cmd: "on",
    }),
  );
  assert.equal(cmd.status, 409);
  assert.equal(cmd.body.error, "offline");
  assert.equal(cmd.body.accepted, false);
});

test("offline heartbeat clears pending commands so relaunch cannot replay them", async () => {
  let now = 1_000;
  const board = new Board(() => now);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus(),
  });
  await post(board, "/api/command", {
    device_id: id.device_id,
    device_token: id.device_token,
    cmd: "on",
  });
  const hb = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus({ active: false }),
      offline: true,
    }),
  );
  assert.equal(
    hb.body.commands.length,
    0,
    "offline heartbeat must return no commands so the Mac cannot apply a stale on after relaunch",
  );
});

test("authorized on/off queues a command; bad token is rejected", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus(),
  });
  const denied = await json(
    await post(board, "/api/command", {
      device_id: id.device_id,
      device_token: "00".repeat(32),
      cmd: "on",
    }),
  );
  assert.equal(denied.status, 401);

  const on = await json(
    await post(board, "/api/command", {
      device_id: id.device_id,
      device_token: id.device_token,
      cmd: "on",
      duration: "8h",
    }),
  );
  assert.equal(on.status, 200);
  assert.equal(on.body.accepted, true);

  const beat = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(beat.body.commands.length, 1);
  assert.equal(beat.body.commands[0].cmd, "on");
  assert.equal(beat.body.commands[0].duration, "8h");

  const off = await json(
    await post(board, "/api/command", {
      device_id: id.device_id,
      device_token: id.device_token,
      cmd: "off",
    }),
  );
  assert.equal(off.body.accepted, true);
});

test("offline Mac does not report command success", async () => {
  let now = 1_000;
  const board = new Board(() => now);
  const id = identity();
  await post(board, "/api/pair/start", id);
  now = 1_000 + HEARTBEAT_TTL_SECS + 5;
  const res = await json(
    await post(board, "/api/command", {
      device_id: id.device_id,
      device_token: id.device_token,
      cmd: "on",
    }),
  );
  assert.equal(res.status, 409);
  assert.equal(res.body.ok, false);
  assert.equal(res.body.error, "offline");
  assert.equal(res.body.accepted, false);
});

test("network path rejects toggle/quit and does not leak other devices", async () => {
  const board = new Board(() => 1_000);
  const a = identity();
  const b = {
    device_id: "11".repeat(16),
    device_token: "22".repeat(32),
    display_name: "Kitchen",
  };
  await post(board, "/api/pair/start", a);
  await post(board, "/api/pair/start", b);
  await post(board, "/api/heartbeat", {
    device_id: a.device_id,
    device_token: a.device_token,
    status: sampleStatus(),
  });
  const toggle = await json(
    await post(board, "/api/command", {
      device_id: a.device_id,
      device_token: a.device_token,
      cmd: "toggle",
    }),
  );
  assert.equal(toggle.status, 400);
  const quit = await json(
    await post(board, "/api/command", {
      device_id: a.device_id,
      device_token: a.device_token,
      cmd: "quit",
    }),
  );
  assert.equal(quit.status, 400);

  const listed = await json(
    await post(board, "/api/list", {
      devices: [
        { device_id: a.device_id, device_token: a.device_token },
        { device_id: b.device_id, device_token: a.device_token },
      ],
    }),
  );
  assert.equal(listed.body.devices.length, 1);
  assert.equal(listed.body.devices[0].device_id, a.device_id);
  assert.equal(
    listed.body.devices[0].display_name,
    "Studio",
    "wrong token must not reveal Kitchen",
  );
});

test("pairing code is one-time: second claim is 404", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  const started = await json(await post(board, "/api/pair/start", id));
  const first = await json(
    await post(board, "/api/pair/claim", {
      pairing_code: started.body.pairing_code,
    }),
  );
  assert.equal(first.status, 200);
  assert.equal(first.body.device_token, id.device_token);
  const second = await json(
    await post(board, "/api/pair/claim", {
      pairing_code: started.body.pairing_code,
    }),
  );
  assert.equal(second.status, 404);
  assert.equal(second.body.ok, false);
  assert.equal(second.body.error, "unknown_code");
});

test("commands stay queued until the Mac acks receipt", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus(),
  });
  const on = await json(
    await post(board, "/api/command", {
      device_id: id.device_id,
      device_token: id.device_token,
      cmd: "on",
    }),
  );
  assert.equal(on.body.accepted, true);
  const commandId = on.body.command_id;
  const first = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(first.body.commands.length, 1);
  assert.equal(first.body.commands[0].id, commandId);
  const lostRetry = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(
    lostRetry.body.commands.length,
    1,
    "a lost heartbeat response must not drop the command",
  );
  const acked = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
      ack_command_ids: [commandId],
    }),
  );
  assert.equal(acked.body.commands.length, 0);
  const afterAck = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(afterAck.body.commands.length, 0);
});

test("pairing URL uses the serving origin, not a hard-coded production host", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  const res = await handleApi(
    board,
    new Request("http://127.0.0.1:8787/api/pair/start", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(id),
    }),
  );
  const body = await res.json();
  assert.equal(res.status, 200);
  assert.ok(
    body.pairing_url.startsWith("http://127.0.0.1:8787/board/"),
    body.pairing_url,
  );
  assert.ok(!body.pairing_url.includes("xyz-ai.app"));
  assert.equal(
    publicSiteOrigin("https://xyz-ai.app/api/pair/start"),
    "https://xyz-ai.app/never-sleep",
  );
  assert.equal(
    publicSiteOrigin("https://mac-never-sleep.example.workers.dev/api/x"),
    "https://mac-never-sleep.example.workers.dev",
  );
  assert.equal(
    publicSiteOrigin("http://127.0.0.1:8787/api/x", {
      NEVER_SLEEP_CLOUD_URL: "https://preview.example/never-sleep",
    }),
    "https://preview.example/never-sleep",
  );
  assert.equal(
    pairingUrl("AB7K2Q9M", true, "http://127.0.0.1:8787").includes("/zh/board/"),
    true,
  );
});

test("Chinese lang on pair/start and heartbeat returns /zh/board/", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  const started = await json(
    await post(board, "/api/pair/start", { ...id, lang: "zh" }),
  );
  assert.ok(started.body.pairing_url.includes("/zh/board/"));
  const beat = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      lang: "zh",
      status: sampleStatus(),
    }),
  );
  assert.ok(beat.body.pairing_url.includes("/zh/board/"));
});

test("heartbeat sanitizes untrusted status before it can reach the phone", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: {
      active: true,
      display: "<script>alert(1)</script>",
      lid: "closed<img>",
      on_ac: "yes",
      battery: "<img src=x onerror=alert(1)>",
      remaining_secs: "1e999",
      user_present: 1,
      elapsed_secs: { steal: true },
      stop_reason: "<img src=x onerror=document.location=localStorage>",
      stop_reason_code: "user<script>",
      screen_off_enabled: "true",
      lid_awake_enabled: 2,
    },
  });
  const listed = await json(
    await post(board, "/api/list", {
      devices: [{ device_id: id.device_id, device_token: id.device_token }],
    }),
  );
  const st = listed.body.devices[0];
  assert.equal(st.display, "awake");
  assert.equal(st.lid, "open");
  assert.equal(st.on_ac, false);
  assert.equal(st.battery, null);
  assert.equal(st.remaining_secs, null);
  assert.equal(st.user_present, false);
  assert.equal(st.elapsed_secs, null);
  assert.equal(st.stop_reason, null);
  assert.equal(st.stop_reason_code, null);
  assert.equal(st.screen_off_enabled, false);
  assert.equal(st.lid_awake_enabled, false);
  assert.equal(st.active, true);
  const xss = sanitizeStatus({
    battery: "<script>",
    display: "asleep",
    lid: "closed",
    on_ac: true,
    active: false,
    user_present: false,
    screen_off_enabled: true,
    lid_awake_enabled: true,
  });
  assert.equal(xss.battery, null);
  assert.equal(xss.display, "asleep");
});

test("API traffic is sharded per device and pairing code, never a global board", () => {
  const id = identity();
  assert.equal(shardName("/api/pair/start", id), `device:${id.device_id}`);
  assert.equal(shardName("/api/heartbeat", id), `device:${id.device_id}`);
  assert.equal(shardName("/api/command", id), `device:${id.device_id}`);
  assert.equal(
    shardName("/api/pair/claim", { pairing_code: "AB7K-2Q9M" }),
    "pair:AB7K2Q9M",
  );
  assert.equal(shardName("/api/list", { devices: [id] }), null);
  assert.notEqual(shardName("/api/pair/start", id), "board");
  assert.notEqual(shardName("/api/heartbeat", id), "board");
});

test("heartbeat and command reject invalid tokens before allocating a shard", () => {
  const id = identity();
  assert.equal(
    shardName("/api/heartbeat", { device_id: id.device_id, device_token: "t" }),
    null,
  );
  assert.equal(
    shardName("/api/command", {
      device_id: id.device_id,
      device_token: "f".repeat(128),
    }),
    null,
  );
  assert.equal(
    shardName("/api/heartbeat", {
      device_id: "f".repeat(64),
      device_token: id.device_token,
    }),
    null,
  );
  assert.equal(shardName("/api/heartbeat", id), `device:${id.device_id}`);
  assert.equal(shardName("/api/command", id), `device:${id.device_id}`);
});

test("pair/start rejects oversized credentials before allocating a shard", async () => {
  const id = identity();
  assert.equal(id.device_id.length, DEVICE_ID_LEN);
  assert.equal(id.device_token.length, DEVICE_TOKEN_LEN);
  assert.equal(deviceCredentialsAreValid(id.device_id, id.device_token), true);
  assert.equal(
    shardName("/api/pair/start", {
      device_id: "f".repeat(64),
      device_token: "f".repeat(128),
    }),
    null,
  );
  assert.equal(
    shardName("/api/pair/start", {
      device_id: id.device_id,
      device_token: "f".repeat(128),
    }),
    null,
  );
  const board = new Board(() => 1_000);
  const huge = await json(
    await post(board, "/api/pair/start", {
      device_id: "f".repeat(64),
      device_token: "f".repeat(128),
    }),
  );
  assert.equal(huge.status, 400);
  assert.equal(huge.body.error, "bad_identity");
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const index = fs.readFileSync(path.join(root, "worker/src/index.js"), "utf8");
  const startAt = index.indexOf('if (path === "/api/pair/start")');
  const shardAt = index.indexOf("const name = shardName(path, body);", startAt);
  const stubAt = index.indexOf("stubFetch", shardAt);
  assert.ok(startAt >= 0 && shardAt > startAt && stubAt > shardAt);
  const region = index.slice(startAt, stubAt);
  assert.ok(
    region.includes("bad_identity"),
    "pair/start must reject before idFromName or pair-offer reservation",
  );
});

test("expired pairing codes are returned so the router can drop pair shards", async () => {
  let now = 1_000;
  const board = new Board(() => now);
  const id = identity();
  const started = await json(await post(board, "/api/pair/start", id));
  const raw = started.body.pairing_code.replace("-", "");
  now = 1_000 + PAIRING_TTL_SECS + 1;
  const beat = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(beat.body.pairing_code, null);
  assert.ok(
    beat.body.expired_codes.includes(raw),
    "heartbeat must report the expired offer so pair:{code} can be deleted",
  );
  const renewed = await json(await post(board, "/api/pair/start", id));
  assert.equal(renewed.status, 200);
  assert.notEqual(renewed.body.pairing_code, started.body.pairing_code);
});

test("startPairing after TTL lists the expired code as replaced", async () => {
  let now = 1_000;
  const board = new Board(() => now);
  const id = identity();
  const first = await json(await post(board, "/api/pair/start", id));
  const raw = first.body.pairing_code.replace("-", "");
  now = 1_000 + PAIRING_TTL_SECS + 1;
  const second = await json(await post(board, "/api/pair/start", id));
  assert.ok(
    second.body.replaced_codes.includes(raw),
    "router must be told to drop the expired pair:{code} shard",
  );
});

test("startPairing lists the previous live code as replaced", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  const first = await json(await post(board, "/api/pair/start", id));
  const raw = first.body.pairing_code.replace("-", "");
  const second = await json(await post(board, "/api/pair/start", id));
  assert.ok(second.body.replaced_codes.includes(raw));
  assert.notEqual(second.body.pairing_code, first.body.pairing_code);
});

test("stale pair/start must not publish after a newer code is live", () => {
  assert.equal(pairingCodeIsLive("ZZZZ-YYYY", "ZZZZ-YYYY"), true);
  assert.equal(
    pairingCodeIsLive("ZZZZ-YYYY", "AB7K-2Q9M"),
    false,
    "a superseded start must not resurrect its pair shard",
  );
  assert.equal(pairingCodeIsLive(null, "AB7K-2Q9M"), false);
  const board = new Board(() => 1_000);
  const id = identity();
  board.startPairing({
    deviceId: id.device_id,
    deviceToken: id.device_token,
    displayName: id.display_name,
  });
  const live = board.livePairing(id.device_id);
  const stale = board.startPairing({
    deviceId: id.device_id,
    deviceToken: id.device_token,
    displayName: id.display_name,
  });
  const newest = board.livePairing(id.device_id);
  assert.equal(
    pairingCodeIsLive(newest.pairing_code, live.pairing_code),
    false,
  );
  assert.equal(
    pairingCodeIsLive(newest.pairing_code, stale.pairing_code),
    true,
  );
});

test("list entries are capped and deduplicated before fan-out", () => {
  const token = "cd".repeat(32);
  const a = { device_id: "aa".repeat(16), device_token: token };
  const b = { device_id: "bb".repeat(16), device_token: token };
  const capped = capListEntries([
    a,
    a,
    b,
    { device_id: "x" },
    { device_id: "ff".repeat(40), device_token: token },
    { device_id: "cc".repeat(16), device_token: "t" },
    { device_id: "dd".repeat(16), device_token: "ee".repeat(40) },
  ]);
  assert.equal(capped.length, 2);
  assert.equal(capped[0].device_id, a.device_id);
  assert.equal(capped[1].device_id, b.device_id);
  const many = Array.from({ length: LIST_MAX_DEVICES + 10 }, (_, i) => ({
    device_id: String(i).padStart(32, "0"),
    device_token: token,
  }));
  assert.equal(capListEntries(many).length, LIST_MAX_DEVICES);
});

test("empty lookup boards are not persisted", () => {
  assert.equal(persistBoardAction(null, { devices: {}, codes: {} }), "skip");
  assert.equal(
    persistBoardAction(null, { devices: { ab: { token: "x" } }, codes: {} }),
    "put",
  );
  assert.equal(
    persistBoardAction({ devices: { ab: {} }, codes: {} }, { devices: {}, codes: {} }),
    "delete",
  );
  assert.equal(
    persistBoardAction({ devices: {}, codes: {} }, { devices: {}, codes: {} }),
    "delete",
    "leftover empty boards from failed lookups are dropped",
  );
  assert.equal(
    persistBoardAction(null, {
      devices: {},
      codes: {},
      pairStart: { global: [1_000], ips: { "1.1.1.1": [1_000] } },
    }),
    "put",
    "pair/start rate-limit hits must persist",
  );
  assert.equal(
    persistBoardAction(null, {
      devices: {},
      codes: {},
      listRate: { global: [1_000], ips: { "1.1.1.1": [1_000] } },
    }),
    "put",
    "list rate-limit hits must persist",
  );
  assert.equal(
    persistBoardAction(null, {
      devices: {},
      codes: {},
      claimRate: { global: [1_000], ips: { "1.1.1.1": [1_000] } },
    }),
    "put",
    "pair/claim rate-limit hits must persist",
  );
  assert.equal(
    persistBoardAction(null, {
      devices: {},
      codes: {},
      deviceRate: { global: [1_000], ips: { "1.1.1.1": [1_000] } },
    }),
    "put",
    "heartbeat/command rate-limit hits must persist",
  );
});

test("unchanged list snapshots skip Durable Object writes", () => {
  const stored = {
    devices: { ab: { token: "x", lastSeen: 1, status: { active: true } } },
    codes: {},
  };
  assert.equal(
    persistBoardAction(stored, stored),
    "skip",
    "read-only /api/list must not rewrite an unchanged shard",
  );
  assert.equal(
    persistBoardAction(stored, JSON.parse(JSON.stringify(stored))),
    "skip",
  );
  const mutated = JSON.parse(JSON.stringify(stored));
  mutated.devices.ab.lastSeen = 2;
  assert.equal(persistBoardAction(stored, mutated), "put");
});

test("command duration must be a Rust-parseable string", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus(),
  });
  for (const duration of [8, { hours: 8 }, true, "nope", "0h"]) {
    const res = await json(
      await post(board, "/api/command", {
        device_id: id.device_id,
        device_token: id.device_token,
        cmd: "on",
        duration,
      }),
    );
    assert.equal(res.status, 400, JSON.stringify(duration));
    assert.equal(res.body.error, "bad_duration");
  }
  const emptyBeat = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(emptyBeat.body.commands.length, 0);
  const ok = await json(
    await post(board, "/api/command", {
      device_id: id.device_id,
      device_token: id.device_token,
      cmd: "on",
      duration: "240h",
    }),
  );
  assert.equal(ok.status, 200);
  const beat = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(beat.body.commands[0].duration, "240h");
});

test("sanitizeStatus keeps week-plus remaining and elapsed seconds", () => {
  const st = sanitizeStatus({
    active: true,
    display: "asleep",
    lid: "open",
    on_ac: true,
    remaining_secs: 240 * 3600,
    elapsed_secs: 8 * 24 * 3600,
    user_present: false,
    screen_off_enabled: true,
    lid_awake_enabled: true,
  });
  assert.equal(st.remaining_secs, 240 * 3600);
  assert.equal(st.elapsed_secs, 8 * 24 * 3600);
  const maxHours = sanitizeStatus({
    remaining_secs: 0xffffffff * 3600,
    elapsed_secs: Number.MAX_SAFE_INTEGER,
  });
  assert.equal(maxHours.remaining_secs, 0xffffffff * 3600);
  assert.equal(maxHours.elapsed_secs, Number.MAX_SAFE_INTEGER);
});

test("isAllowedDuration matches the Rust duration parser", () => {
  for (const raw of ["240h", "indefinite", "until=08:00", "22:30", "3小时"]) {
    assert.equal(isAllowedDuration(raw), true, raw);
  }
  assert.equal(isAllowedDuration("4294967295h"), true);
  assert.equal(isAllowedDuration("4294967296h"), false);
  assert.equal(isAllowedDuration(8), false);
  assert.equal(isAllowedDuration({ hours: 8 }), false);
});

test("pair/start uses a reserved pairing code when provided", async () => {
  const board = new Board(() => 1_000);
  const started = await json(
    await post(board, "/api/pair/start", {
      ...identity(),
      pairing_code: "AB7K-2Q9M",
      expires_unix: 2_000,
    }),
  );
  assert.equal(started.status, 200);
  assert.equal(started.body.pairing_code, "AB7K-2Q9M");
  assert.equal(started.body.expires_unix, 2000);
});

test("independent snapshots drop a racing command on last write", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus(),
  });
  const snap = board.toJSON();
  const phone = Board.fromJSON(snap);
  phone.nowSecs = () => 1_000;
  const mac = Board.fromJSON(snap);
  mac.nowSecs = () => 1_000;
  await post(phone, "/api/command", {
    device_id: id.device_id,
    device_token: id.device_token,
    cmd: "on",
  });
  await post(mac, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus(),
  });
  const restored = Board.fromJSON(mac.toJSON());
  restored.nowSecs = () => 1_000;
  const beat = await json(
    await post(restored, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(
    beat.body.commands.length,
    0,
    "last snapshot wins and the queued on is gone",
  );
});

test("serial queue finishes one request before starting the next", async () => {
  const enqueue = createSerialQueue();
  const order = [];
  const first = enqueue(async () => {
    order.push("a-start");
    await new Promise((resolve) => setTimeout(resolve, 20));
    order.push("a-end");
    return 1;
  });
  const second = enqueue(async () => {
    order.push("b-start");
    order.push("b-end");
    return 2;
  });
  assert.deepEqual(await Promise.all([first, second]), [1, 2]);
  assert.deepEqual(order, ["a-start", "a-end", "b-start", "b-end"]);
});

test("pair shard reservation is create-if-absent and retries on collision", async () => {
  const board = new Board(() => 1_000);
  const a = identity();
  const b = {
    device_id: "11".repeat(16),
    device_token: "22".repeat(32),
    display_name: "Kitchen",
  };
  const first = board.rememberOffer({
    pairingCode: "AB7K2Q9M",
    deviceId: a.device_id,
    deviceToken: a.device_token,
    displayName: a.display_name,
  });
  assert.equal(first.ok, true);
  const clash = board.rememberOffer({
    pairingCode: "AB7K2Q9M",
    deviceId: b.device_id,
    deviceToken: b.device_token,
    displayName: b.display_name,
  });
  assert.equal(clash.ok, false);
  assert.equal(clash.status, 409);
  const codes = ["AB7K2Q9M", "ZZZZYYYY"];
  const result = await publishReservedPairing({
    generateCode: () => codes.shift(),
    reserve: async (code) =>
      code === "AB7K2Q9M" ? { ok: false, error: "taken" } : { ok: true },
    startDevice: async (code) => ({ ok: true, pairing_code: code }),
  });
  assert.equal(result.pairing_code, "ZZZZYYYY");
});

test("fitStoredDevices evicts the oldest Mac when the phone list is full", () => {
  const first = { device_id: "aa".repeat(16), device_token: "t" };
  const extra = { device_id: "zz".repeat(16), device_token: "t" };
  const full = Array.from({ length: LIST_MAX_DEVICES }, (_, i) => ({
    device_id: String(i).padStart(32, "0"),
    device_token: "t",
  }));
  full[0] = first;
  const next = fitStoredDevices(full, extra);
  assert.equal(next.length, LIST_MAX_DEVICES);
  assert.equal(next[next.length - 1].device_id, extra.device_id);
  assert.equal(
    next.some((d) => d.device_id === first.device_id),
    false,
  );
  const same = fitStoredDevices(full, first);
  assert.equal(same.length, LIST_MAX_DEVICES);
  assert.equal(same[same.length - 1].device_id, first.device_id);
});

test("stale commands expire after the delivery window", async () => {
  let now = 1_000;
  const board = new Board(() => now);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus(),
  });
  await post(board, "/api/command", {
    device_id: id.device_id,
    device_token: id.device_token,
    cmd: "on",
  });
  now = 1_000 + COMMAND_TTL_SECS + 1;
  const beat = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(beat.body.commands.length, 0);
});

test("expired pairing offers are dropped without a later heartbeat", () => {
  const board = new Board(() => 1_000);
  board.startPairing({
    deviceId: identity().device_id,
    deviceToken: identity().device_token,
    displayName: identity().display_name,
  });
  assert.equal(board.nextAlarmUnix(), 1_000 + PAIRING_TTL_SECS);
  board.nowSecs = () => 1_000 + PAIRING_TTL_SECS + 1;
  const expired = board.expireOffers();
  assert.equal(expired.length, 1);
  assert.equal(board.nextAlarmUnix(), null);
});

test("unverified devices expire with the pairing offer", () => {
  const board = new Board(() => 1_000);
  const id = identity();
  board.startPairing({
    deviceId: id.device_id,
    deviceToken: id.device_token,
    displayName: id.display_name,
  });
  const stored = board.toJSON();
  assert.ok(stored.devices[id.device_id]);
  board.nowSecs = () => 1_000 + PAIRING_TTL_SECS + 1;
  board.expireOffers();
  const empty = board.toJSON();
  assert.equal(Object.keys(empty.devices || {}).length, 0);
  assert.equal(persistBoardAction(stored, empty), "delete");
});

test("a heartbeat keeps the device after the pairing offer expires", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", {
    device_id: id.device_id,
    device_token: id.device_token,
    status: sampleStatus({ active: true }),
  });
  board.nowSecs = () => 1_000 + PAIRING_TTL_SECS + 1;
  board.expireOffers();
  assert.equal(
    board.toJSON().devices[id.device_id].status.active,
    true,
    "verified Macs must not be deleted with the pairing code",
  );
});

test("list fan-out keeps healthy Macs when one shard fails", async () => {
  const devices = await collectListParts(
    async (entry) => {
      if (entry.device_id === "bad") throw new Error("do down");
      return [{ device_id: entry.device_id }];
    },
    [{ device_id: "good" }, { device_id: "bad" }, { device_id: "also" }],
  );
  assert.deepEqual(
    devices.map((d) => d.device_id),
    ["good", "also"],
  );
});

function quotaStorage(limitBytes) {
  const data = new Map();
  const used = () => {
    let n = 0;
    for (const [key, value] of data) n += key.length + value.length;
    return n;
  };
  return {
    getItem(key) {
      return data.has(key) ? data.get(key) : null;
    },
    setItem(key, value) {
      const next = String(value);
      const projected =
        used() -
        (data.has(key) ? key.length + data.get(key).length : 0) +
        key.length +
        next.length;
      if (projected > limitBytes) {
        const err = new Error("QuotaExceededError");
        err.name = "QuotaExceededError";
        throw err;
      }
      data.set(key, next);
    },
    removeItem(key) {
      data.delete(key);
    },
  };
}

function boardClientHelpers() {
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const src = fs.readFileSync(path.join(root, "site/assets/board.js"), "utf8");
  const start = src.indexOf("function hrefWithPairingCode");
  assert.notEqual(start, -1, "board.js must expose hrefWithPairingCode");
  const end = src.indexOf("\n  function syncLanguageLinks");
  assert.ok(end > start, "pairing href helpers must sit together");
  const helpers = new Function(
    `${src.slice(start, end)}\nreturn { hrefWithPairingCode, languageHrefAfterClaim, waitForClaimThenLanguageHref };`,
  )();
  return { src, ...helpers };
}

test("language href keeps the pairing code until claim succeeds", () => {
  const { src, hrefWithPairingCode } = boardClientHelpers();
  assert.equal(
    hrefWithPairingCode("../zh/board/", "AB7K-2Q9M"),
    "../zh/board/?code=AB7K-2Q9M",
  );
  assert.equal(
    hrefWithPairingCode("../../board/?code=OLD", ""),
    "../../board/",
  );
  const saveAt = src.indexOf("saveDevices(devices)");
  const clearAt = src.indexOf("clearPairingQuery");
  assert.ok(saveAt >= 0 && clearAt > saveAt, "strip ?code= only after the token is stored");
  assert.match(src, /history\.replaceState/);
  assert.match(src, /syncLanguageLinks/);
});

test("language links wait for in-flight claim before navigating", async () => {
  const { src, waitForClaimThenLanguageHref } = boardClientHelpers();
  let resolveClaim;
  const claimPromise = new Promise((resolve) => {
    resolveClaim = resolve;
  });
  const pending = waitForClaimThenLanguageHref(
    "../zh/board/?code=AB7K-2Q9M",
    "AB7K-2Q9M",
    claimPromise,
  );
  let settled = false;
  pending.then(() => {
    settled = true;
  });
  await Promise.resolve();
  assert.equal(settled, false, "navigation must wait for the in-flight claim");
  resolveClaim(true);
  assert.equal(
    await pending,
    "../zh/board/",
    "drop ?code= after the in-flight claim stores the token",
  );
  assert.equal(
    await waitForClaimThenLanguageHref(
      "../zh/board/",
      "AB7K-2Q9M",
      Promise.resolve(false),
    ),
    "../zh/board/?code=AB7K-2Q9M",
    "keep the code if claim did not store a token",
  );
  assert.match(src, /preventDefault/);
  assert.match(src, /waitForClaimThenLanguageHref/);
  assert.match(src, /claimPromise/);
});

test("claim returns the token when pair-shard cleanup fails", async () => {
  const payload = { ok: true, device_id: "ab".repeat(16), device_token: "t" };
  const result = await bestEffortCleanup(payload, async () => {
    throw new Error("pair shard down");
  });
  assert.deepEqual(result, payload);
});

test("unchanged alarm deadlines skip Durable Object alarm writes", () => {
  assert.equal(alarmNeedsUpdate(undefined, 1_600), true);
  assert.equal(alarmNeedsUpdate(1_600, 1_600), false);
  assert.equal(alarmNeedsUpdate(1_600, 1_700), true);
  assert.equal(alarmNeedsUpdate(1_600, null), true);
  assert.equal(alarmNeedsUpdate(null, null), false);
});

test("superseded pair/start is not returned after a newer code is live", async () => {
  let released = [];
  const forgotten = [];
  const result = await publishReservedPairing({
    generateCode: () => "AAAA1111",
    reserve: async () => ({ ok: true }),
    startDevice: async (code) => ({
      ok: true,
      pairing_code: code,
      status: 200,
    }),
    confirmLive: async () => false,
    release: async (code) => {
      released.push(code);
    },
    forgetDevice: async (code) => {
      forgotten.push(code);
    },
  });
  assert.equal(result.ok, false);
  assert.deepEqual(released, ["AAAA1111"]);
  assert.deepEqual(
    forgotten,
    ["AAAA1111"],
    "an unconfirmed start must drop the device offer so heartbeat cannot advertise it",
  );
});

test("partial list polls keep cached statuses for missing Macs", () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function mergeListStatuses");
  assert.notEqual(start, -1, "board.js must merge partial /list responses");
  const end = src.indexOf("\n  function ", start + 10);
  const mergeListStatuses = new Function(
    `${src.slice(start, end)}\nreturn mergeListStatuses;`,
  )();
  const cached = [
    { device_id: "good", active: true, display: "asleep" },
    { device_id: "down", active: true, display: "asleep" },
  ];
  const partial = [{ device_id: "good", active: true, display: "asleep" }];
  const merged = mergeListStatuses(cached, partial);
  assert.equal(
    merged.find((d) => d.device_id === "down").active,
    true,
    "a failed shard must not drop the last-known standby state",
  );
  assert.equal(
    merged.find((d) => d.device_id === "down").online,
    false,
    "a Mac omitted from the current /list must not stay Online",
  );
  assert.equal(merged.find((d) => d.device_id === "good").online, undefined);
  assert.match(src, /const fallback = mergeListStatuses/);
});

test("beginClaim reuses an in-flight request and restores the pairing button", async () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function beginClaim");
  assert.notEqual(start, -1);
  const end = src.indexOf("\n  document.querySelectorAll", start);
  const submit = { disabled: false, textContent: "Add Mac" };
  const beginClaim = new Function("document", "zh", "copy",
    `let claimPromise = null;\nlet claimInFlight = false;\nfunction claim() { return Promise.resolve(true); }\n${src.slice(start, end)}\nreturn beginClaim;`,
  )({ querySelector: () => submit }, false, { add: "Add Mac" });
  const first = beginClaim("AB7K-2Q9M");
  const second = beginClaim("AB7K-2Q9M");
  assert.equal(first, second, "double submit must not start a second claim");
  assert.equal(submit.disabled, true);
  assert.equal(submit.textContent, "Pairing…");
  await first;
  assert.equal(submit.disabled, false);
  assert.equal(submit.textContent, "Add Mac");
  assert.match(src, /claimInFlight/);
});

test("command pending state is tracked per Mac", () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function devicePendingCmd");
  assert.notEqual(start, -1, "board.js must track in-flight commands per device");
  const end = src.indexOf("\n  function hrefWithPairingCode");
  const { devicePendingCmd, withDevicePending, withoutDevicePending } =
    new Function(
      `${src.slice(start, end)}\nreturn { devicePendingCmd, withDevicePending, withoutDevicePending };`,
    )();
  assert.equal(devicePendingCmd({ "mac-c": "sleep_display" }, "mac-c"), "sleep_display");
  let pending = {};
  pending = withDevicePending(pending, "mac-a", "on");
  pending = withDevicePending(pending, "mac-b", "off");
  assert.equal(devicePendingCmd(pending, "mac-a"), "on");
  assert.equal(devicePendingCmd(pending, "mac-b"), "off");
  pending = withoutDevicePending(pending, "mac-a");
  assert.equal(
    devicePendingCmd(pending, "mac-a"),
    null,
    "finishing one Mac must not clear another Mac's in-flight command",
  );
  assert.equal(devicePendingCmd(pending, "mac-b"), "off");
  assert.match(src, /pendingByDevice = withDevicePending/);
  assert.match(src, /pendingByDevice = withoutDevicePending/);
  assert.doesNotMatch(
    src,
    /pendingId = null/,
    "a global pendingId would clear every Mac when the first request finishes",
  );
});

test("stale list refreshes do not replace a newer snapshot", () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function nextRefreshGeneration");
  assert.notEqual(start, -1, "board.js must stamp each /list poll");
  const end = src.indexOf("\n  function hrefWithPairingCode");
  const { nextRefreshGeneration, isCurrentRefresh } = new Function(
    `${src.slice(start, end)}\nreturn { nextRefreshGeneration, isCurrentRefresh };`,
  )();
  const first = nextRefreshGeneration(0);
  const second = nextRefreshGeneration(first);
  assert.equal(isCurrentRefresh(first, second), false);
  assert.equal(isCurrentRefresh(second, second), true);
  assert.match(src, /isCurrentRefresh\(started, refreshGen\)/);
});

test("toJSON snapshots are detached from live board mutations", () => {
  const board = new Board(() => 1_000);
  board.devices.set("ab", {
    token: "t",
    lastSeen: 1,
    status: { active: true },
  });
  const stored = board.toJSON();
  board.devices.get("ab").status.active = false;
  board.devices.get("ab").lastSeen = 2;
  assert.equal(
    stored.devices.ab.status.active,
    true,
    "this.stored must not alias mutable Board values",
  );
  assert.equal(stored.devices.ab.lastSeen, 1);
  assert.equal(
    persistBoardAction(stored, board.toJSON()),
    "put",
    "a heartbeat after the first persist must still write",
  );
});

test("alarm scheduling failure is propagated so pair/start can retry", async () => {
  let scheduled = undefined;
  scheduled = await commitAlarmUnix(scheduled, 1_600, async () => {});
  assert.equal(scheduled, 1_600);
  await assert.rejects(
    () =>
      commitAlarmUnix(1_600, 1_700, async () => {
        throw new Error("transient setAlarm");
      }),
    /transient setAlarm/,
    "swallowing setAlarm still lets pair/start return success with no expiry",
  );
  const skipped = await commitAlarmUnix(1_600, 1_600, async () => {
    throw new Error("unchanged alarms must not call storage");
  });
  assert.equal(skipped, 1_600);
});

test("claim still returns after a transient deleteAlarm failure", async () => {
  const cleared = await commitPersistedAlarm(1_600, null, async () => {
    throw new Error("transient deleteAlarm");
  });
  assert.equal(
    cleared,
    1_600,
    "the one-time code is already persisted; keep retrying delete later",
  );
  await assert.rejects(
    () =>
      commitPersistedAlarm(1_600, 1_700, async () => {
        throw new Error("transient setAlarm");
      }),
    /transient setAlarm/,
    "pair/start still needs setAlarm to succeed",
  );
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const index = fs.readFileSync(path.join(root, "worker/src/index.js"), "utf8");
  assert.match(
    index,
    /commitPersistedAlarm/,
    "BoardHub must not fail a persisted claim when deleteAlarm rejects",
  );
});

test("pair/start and claim cap oversized display names", async () => {
  const board = new Board(() => 1_000);
  const id = identity();
  const long = "名".repeat(200);
  const started = await json(
    await post(board, "/api/pair/start", {
      ...id,
      display_name: long,
    }),
  );
  assert.equal(started.status, 200);
  const claimed = await json(
    await post(board, "/api/pair/claim", {
      pairing_code: started.body.pairing_code,
    }),
  );
  assert.equal(claimed.status, 200);
  assert.equal([...claimed.body.display_name].length, MAX_DISPLAY_NAME_CHARS);
  assert.equal(boundDisplayName(long).length, MAX_DISPLAY_NAME_CHARS);
  assert.equal(boundDisplayName("  Studio  "), "Studio");
  assert.equal(boundDisplayName("   "), "Mac");
});

test("failed persist restores the live board from the stored snapshot", () => {
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const index = fs.readFileSync(path.join(root, "worker/src/index.js"), "utf8");
  assert.match(
    index,
    /catch[\s\S]*this\.board = Board\.fromJSON\(this\.stored\)/,
    "storage.put rejection must roll back in-memory mutations",
  );
});

test("hour-long remaining countdowns keep live seconds", () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function formatRemaining");
  assert.notEqual(start, -1, "board.js must format remaining_secs");
  const end = src.indexOf("\n  function el(");
  const { formatRemaining } = new Function(
    `${src.slice(start, end)}\nreturn { formatRemaining };`,
  )();
  assert.equal(formatRemaining(7_183), "1:59:43");
  assert.equal(formatRemaining(3_600), "1:00:00");
  assert.equal(formatRemaining(59), "0:59");
});

test("slow list polls queue one follow-up instead of discarding every response", () => {
  const { src } = boardClientHelpers();
  assert.match(
    src,
    /if \(refreshInFlight\)/,
    "do not start a second /list while one is in flight",
  );
  assert.match(src, /refreshQueued/);
});

test("refresh completion is not tied to continuous polling", () => {
  const { src } = boardClientHelpers();
  assert.equal(
    src.includes("} while (refreshQueued);"),
    false,
    "an always-slow /list must not keep claim() awaiting the poll loop",
  );
  assert.match(
    src,
    /void refresh\(\)/,
    "queued follow-ups continue in the background after the current poll settles",
  );
});

test("claim reserves space for the credential when quota is almost full", async () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function withDevices");
  assert.notEqual(
    start,
    -1,
    "board.js must build the prospective device list before claiming",
  );
  const end = src.indexOf("\n  async function post(");
  const {
    withDevices,
    storageCanHold,
    canStoreClaim,
    claimReservation,
    boundDisplayName,
  } = new Function(
    `const LIST_MAX_DEVICES = 32;\nconst MAX_DISPLAY_NAME_CHARS = 128;\n${src.slice(start, end)}\nreturn { withDevices, storageCanHold, canStoreClaim, claimReservation, boundDisplayName };`,
  )();

  const STORAGE_KEY = "never-sleep-devices";
  const existing = [];
  for (let i = 0; i < 31; i += 1) {
    existing.push({
      device_id: String(i).padStart(32, "a"),
      device_token: String(i).padStart(64, "b"),
      display_name: "Studio",
    });
  }
  const existingRaw = JSON.stringify(existing);
  const used = STORAGE_KEY.length + existingRaw.length;
  const tinyExtra = `${STORAGE_KEY}:ok`.length + 1;
  const storage = quotaStorage(used + tinyExtra + 16);
  storage.setItem(STORAGE_KEY, existingRaw);
  storage.setItem(`${STORAGE_KEY}:ok`, "1");
  storage.removeItem(`${STORAGE_KEY}:ok`);

  assert.equal(
    await canStoreClaim(storage, STORAGE_KEY, existing, claimReservation()),
    false,
    "a 1-byte probe must not hide a quota that cannot hold the new token",
  );
  assert.equal(
    storageCanHold(storage, STORAGE_KEY, withDevices(existing, claimReservation())),
    false,
  );

  const exclusive = src.indexOf("function runExclusiveClaim");
  const claimPost = src.indexOf('post("/pair/claim"');
  const storageCheck = src.indexOf("storageCanHold", exclusive);
  assert.ok(
    exclusive >= 0 && exclusive < claimPost && storageCheck > exclusive && storageCheck < claimPost,
    "do not consume the one-time code before the credential payload fits",
  );
  const claimFn = src.slice(src.indexOf("async function claim("));
  assert.ok(
    claimFn.indexOf("runExclusiveClaim") >= 0
      && claimFn.indexOf("runExclusiveClaim") < claimFn.indexOf('post("/pair/claim"'),
    "probe, /pair/claim, and commit must share one storage reservation",
  );
  assert.match(src, /storageError/);
  assert.equal(boundDisplayName("名".repeat(200)).length, MAX_DISPLAY_NAME_CHARS);
});

test("claim reservation covers a max-length display name near quota", async () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function withDevices");
  const end = src.indexOf("\n  async function post(");
  const { withDevices, canStoreClaim, claimReservation } = new Function(
    `const LIST_MAX_DEVICES = 32;\nconst MAX_DISPLAY_NAME_CHARS = 128;\n${src.slice(start, end)}\nreturn { withDevices, canStoreClaim, claimReservation };`,
  )();

  const STORAGE_KEY = "never-sleep-devices";
  const existing = [];
  for (let i = 0; i < 31; i += 1) {
    existing.push({
      device_id: String(i).padStart(32, "a"),
      device_token: String(i).padStart(64, "b"),
      display_name: "Studio",
    });
  }
  const shortPlaceholder = {
    device_id: "f".repeat(64),
    device_token: "f".repeat(128),
    display_name: "Mac".repeat(16),
  };
  const reserved = claimReservation();
  assert.equal(reserved.display_name.length, MAX_DISPLAY_NAME_CHARS);
  assert.ok(
    reserved.display_name.length > shortPlaceholder.display_name.length,
    "48 characters is smaller than the documented display-name max",
  );

  const shortPayload = JSON.stringify(withDevices(existing, shortPlaceholder));
  const longPayload = JSON.stringify(withDevices(existing, reserved));
  assert.ok(longPayload.length > shortPayload.length);

  const existingRaw = JSON.stringify(existing);
  // storageCanHold overwrites the real key; quota must fit shortPayload but not longPayload.
  const storage = quotaStorage(
    STORAGE_KEY.length + shortPayload.length + 8,
  );
  storage.setItem(STORAGE_KEY, existingRaw);

  assert.equal(
    await canStoreClaim(storage, STORAGE_KEY, existing, shortPlaceholder),
    true,
    "the old 48-character placeholder still fits this constrained quota",
  );
  assert.equal(
    await canStoreClaim(storage, STORAGE_KEY, existing, reserved),
    false,
    "do not consume the one-time code when a max-length name will not fit",
  );
});

test("claim reservation covers JSON-escaped and emoji display names near quota", async () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function withDevices");
  const end = src.indexOf("\n  async function post(");
  const { withDevices, canStoreClaim, claimReservation } = new Function(
    `const LIST_MAX_DEVICES = 32;\nconst MAX_DISPLAY_NAME_CHARS = 128;\n${src.slice(start, end)}\nreturn { withDevices, canStoreClaim, claimReservation };`,
  )();

  const STORAGE_KEY = "never-sleep-devices";
  const existing = [];
  for (let i = 0; i < 31; i += 1) {
    existing.push({
      device_id: String(i).padStart(32, "a"),
      device_token: String(i).padStart(64, "b"),
      display_name: "Studio",
    });
  }
  const asciiPlaceholder = {
    device_id: "f".repeat(64),
    device_token: "f".repeat(128),
    display_name: "M".repeat(MAX_DISPLAY_NAME_CHARS),
  };
  const quotesReal = {
    device_id: "a".repeat(32),
    device_token: "b".repeat(64),
    display_name: '"'.repeat(MAX_DISPLAY_NAME_CHARS),
  };
  const emojiReal = {
    device_id: "a".repeat(32),
    device_token: "b".repeat(64),
    display_name: "😀".repeat(MAX_DISPLAY_NAME_CHARS),
  };
  const reserved = claimReservation();
  const reservedRaw = JSON.stringify(reserved);
  assert.ok(
    reservedRaw.length >= JSON.stringify(quotesReal).length,
    "128 quotes with a normal identity must still fit under the reservation",
  );
  assert.ok(
    reservedRaw.length >= JSON.stringify(emojiReal).length,
    "128 emoji with a normal identity must still fit under the reservation",
  );

  const asciiPayload = JSON.stringify(withDevices(existing, asciiPlaceholder));
  const quotesPayload = JSON.stringify(withDevices(existing, quotesReal));
  const reservedPayload = JSON.stringify(withDevices(existing, reserved));
  assert.ok(quotesPayload.length > asciiPayload.length);
  assert.ok(reservedPayload.length >= quotesPayload.length);

  const existingRaw = JSON.stringify(existing);
  // storageCanHold overwrites the real key; quota must fit asciiPayload but not quotesPayload.
  const storage = quotaStorage(
    STORAGE_KEY.length + asciiPayload.length + 8,
  );
  storage.setItem(STORAGE_KEY, existingRaw);

  assert.equal(
    await canStoreClaim(storage, STORAGE_KEY, existing, asciiPlaceholder),
    true,
    "the old 128-M placeholder still fits this constrained quota",
  );
  assert.equal(
    await canStoreClaim(storage, STORAGE_KEY, existing, quotesReal),
    false,
    "quotes expand in JSON and must not sneak past the probe",
  );
  assert.equal(
    await canStoreClaim(storage, STORAGE_KEY, existing, reserved),
    false,
    "do not consume the one-time code when the serialized name will not fit",
  );
});

test("heartbeat returns the stored pairing expiry", async () => {
  let now = 1_000;
  const board = new Board(() => now);
  const id = identity();
  const started = await json(await post(board, "/api/pair/start", id));
  assert.equal(started.body.expires_unix, 1_000 + PAIRING_TTL_SECS);
  now = 1_200;
  const beat = await json(
    await post(board, "/api/heartbeat", {
      device_id: id.device_id,
      device_token: id.device_token,
      status: sampleStatus(),
    }),
  );
  assert.equal(beat.status, 200);
  assert.equal(
    beat.body.expires_unix,
    1_000 + PAIRING_TTL_SECS,
    "heartbeat must not mint a fresh TTL for a still-live offer",
  );
});

test("claim commits merge under a cross-tab lock", async () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function withDevices");
  const end = src.indexOf("\n  async function post(");
  const { commitClaimedDevice } = new Function(
    `const LIST_MAX_DEVICES = 32;\nconst MAX_DISPLAY_NAME_CHARS = 128;\n${src.slice(start, end)}\nreturn { commitClaimedDevice };`,
  )();
  assert.equal(typeof commitClaimedDevice, "function");

  const STORAGE_KEY = "never-sleep-devices";
  const storage = quotaStorage(1_000_000);
  storage.setItem(STORAGE_KEY, "[]");
  let chain = Promise.resolve();
  const locks = {
    request(_name, fn) {
      const run = chain.then(() => fn());
      chain = run.catch(() => {});
      return run;
    },
  };
  const a = {
    device_id: "a".repeat(32),
    device_token: "a".repeat(64),
    display_name: "A",
  };
  const b = {
    device_id: "b".repeat(32),
    device_token: "b".repeat(64),
    display_name: "B",
  };
  await Promise.all([
    commitClaimedDevice(storage, STORAGE_KEY, a, locks),
    commitClaimedDevice(storage, STORAGE_KEY, b, locks),
  ]);
  const stored = JSON.parse(storage.getItem(STORAGE_KEY));
  assert.deepEqual(
    stored.map((d) => d.device_id).sort(),
    [a.device_id, b.device_id].sort(),
    "the last claim must not discard the other tab's one-time token",
  );
  assert.match(src, /await (?:commitClaimedDevice\(localStorage|runExclusiveClaim\()/);
  assert.match(src, /navigator\?\.locks/);
});

test("capacity probe shares the claim lock so it cannot restore a stale list", async () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function withDevices");
  const end = src.indexOf("\n  async function post(");
  const { canStoreClaim, commitClaimedDevice, claimReservation } = new Function(
    `const LIST_MAX_DEVICES = 32;\nconst MAX_DISPLAY_NAME_CHARS = 128;\n${src.slice(start, end)}\nreturn { canStoreClaim, commitClaimedDevice, claimReservation };`,
  )();

  const STORAGE_KEY = "never-sleep-devices";
  const storage = quotaStorage(1_000_000);
  storage.setItem(STORAGE_KEY, "[]");
  let held = false;
  let probeUsedLock = false;
  let chain = Promise.resolve();
  const locks = {
    request(_name, fn) {
      const run = chain.then(() => {
        held = true;
        try {
          return fn();
        } finally {
          held = false;
        }
      });
      chain = run.catch(() => {});
      return run;
    },
  };
  const wrapped = {
    getItem(key) {
      if (held) probeUsedLock = true;
      return storage.getItem(key);
    },
    setItem(key, value) {
      if (held) probeUsedLock = true;
      storage.setItem(key, value);
    },
    removeItem(key) {
      storage.removeItem(key);
    },
  };
  const real = {
    device_id: "a".repeat(32),
    device_token: "b".repeat(64),
    display_name: "Studio",
  };
  await Promise.all([
    canStoreClaim(wrapped, STORAGE_KEY, [], claimReservation(), locks),
    commitClaimedDevice(storage, STORAGE_KEY, real, locks),
  ]);
  assert.equal(probeUsedLock, true, "the capacity probe must run inside the same Web Lock");
  const stored = JSON.parse(storage.getItem(STORAGE_KEY));
  assert.equal(stored[0]?.device_id, real.device_id);
});

test("claim holds the storage lock through the one-time POST", async () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function withDevices");
  const end = src.indexOf("\n  async function post(");
  const { runExclusiveClaim } = new Function(
    `const LIST_MAX_DEVICES = 32;\nconst MAX_DISPLAY_NAME_CHARS = 128;\n${src.slice(start, end)}\nreturn { runExclusiveClaim };`,
  )();
  assert.equal(typeof runExclusiveClaim, "function");

  const STORAGE_KEY = "never-sleep-devices";
  const storage = quotaStorage(1_000_000);
  storage.setItem(STORAGE_KEY, "[]");
  let chain = Promise.resolve();
  let held = false;
  const locks = {
    request(_name, fn) {
      const run = chain.then(async () => {
        held = true;
        try {
          return await fn();
        } finally {
          held = false;
        }
      });
      chain = run.catch(() => {});
      return run;
    },
  };
  let sawLockDuringAcquire = false;
  const first = {
    device_id: "a".repeat(32),
    device_token: "a".repeat(64),
    display_name: "A",
  };
  const second = {
    device_id: "b".repeat(32),
    device_token: "b".repeat(64),
    display_name: "B",
  };
  const results = await Promise.all([
    runExclusiveClaim(storage, STORAGE_KEY, locks, async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
      sawLockDuringAcquire = held;
      return { ok: true, entry: first };
    }),
    runExclusiveClaim(storage, STORAGE_KEY, locks, async () => ({
      ok: true,
      entry: second,
    })),
  ]);
  assert.equal(sawLockDuringAcquire, true, "the one-time POST must run under the same lock as the probe");
  assert.equal(results[0].ok, true);
  assert.equal(results[1].ok, true);
  const stored = JSON.parse(storage.getItem(STORAGE_KEY));
  assert.deepEqual(
    stored.map((d) => d.device_id).sort(),
    [first.device_id, second.device_id].sort(),
    "a second tab must wait for the in-flight claim instead of racing the one-time code",
  );
});

test("forget serializes with claim commits", async () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function withDevices");
  const end = src.indexOf("\n  async function post(");
  const { commitClaimedDevice, forgetDevice } = new Function(
    `const LIST_MAX_DEVICES = 32;\nconst MAX_DISPLAY_NAME_CHARS = 128;\n${src.slice(start, end)}\nreturn { commitClaimedDevice, forgetDevice };`,
  )();
  assert.equal(typeof forgetDevice, "function");
  assert.match(src, /forgetDevice\(localStorage/);

  const STORAGE_KEY = "never-sleep-devices";
  const storage = quotaStorage(1_000_000);
  const existing = {
    device_id: "a".repeat(32),
    device_token: "a".repeat(64),
    display_name: "Keep",
  };
  storage.setItem(STORAGE_KEY, JSON.stringify([existing]));
  let chain = Promise.resolve();
  const locks = {
    request(_name, fn) {
      const run = chain.then(() => fn());
      chain = run.catch(() => {});
      return run;
    },
  };
  const claimed = {
    device_id: "b".repeat(32),
    device_token: "b".repeat(64),
    display_name: "New",
  };
  await Promise.all([
    forgetDevice(storage, STORAGE_KEY, existing.device_id, locks),
    commitClaimedDevice(storage, STORAGE_KEY, claimed, locks),
  ]);
  const stored = JSON.parse(storage.getItem(STORAGE_KEY));
  assert.deepEqual(
    stored.map((d) => d.device_id),
    [claimed.device_id],
    "forget must not clobber a concurrent claim, and claim must not restore a forgotten device",
  );
});

test("capacity probe treats blocked localStorage reads as a miss", async () => {
  const { src } = boardClientHelpers();
  const start = src.indexOf("function withDevices");
  const end = src.indexOf("\n  async function post(");
  const { storageCanHold, canStoreClaim, claimReservation } = new Function(
    `const LIST_MAX_DEVICES = 32;\nconst MAX_DISPLAY_NAME_CHARS = 128;\n${src.slice(start, end)}\nreturn { storageCanHold, canStoreClaim, claimReservation };`,
  )();

  const blocked = {
    getItem() {
      const err = new Error("The operation is insecure.");
      err.name = "SecurityError";
      throw err;
    },
    setItem() {
      throw new Error("blocked storage must not write after a failed read");
    },
    removeItem() {},
  };
  assert.equal(
    storageCanHold(blocked, "never-sleep-devices", []),
    false,
    "SecurityError on getItem must not reject claim(); treat it as no capacity",
  );
  assert.equal(
    await canStoreClaim(blocked, "never-sleep-devices", [], claimReservation()),
    false,
  );

  const fn = src.slice(
    src.indexOf("function storageCanHold"),
    src.indexOf("function canStoreClaim"),
  );
  const tryAt = fn.indexOf("try {");
  const getAt = fn.indexOf("getItem(key)");
  assert.ok(
    tryAt >= 0 && getAt > tryAt,
    "the initial localStorage read must sit inside the guarded section",
  );
});

test("expired IP buckets are dropped before a new pair-start is recorded", () => {
  const windowSecs = 60;
  const now = 1_700_000_000;
  const ips = {};
  for (let i = 0; i < 2000; i += 1) {
    const ip = `2001:db8:${i.toString(16).padStart(4, "0")}:0000:0000:0000:0000:0001`;
    ips[ip] = { windowStart: now - windowSecs - 1, count: 8 };
  }
  const next = takePairStartSlot(
    { global: null, ips },
    "2001:db8::ffff",
    now,
    { ipWindowSecs: windowSecs, ipLimit: 8 },
  );
  assert.equal(next.ok, true);
  assert.equal(Object.keys(next.state.ips).length, 1);
  assert.ok(next.state.ips["2001:db8::ffff"]);
});

test("live IP map stays under Durable Object 128 KiB with full-length IPv6 keys", () => {
  const windowSecs = 60;
  const now = 1_700_000_000;
  let state = { global: null, ips: {} };
  for (let i = 0; i < RATE_IP_MAP_MAX; i += 1) {
    const ip = [
      "2001",
      "0db8",
      i.toString(16).padStart(4, "0"),
      "ffff",
      "ffff",
      "ffff",
      "ffff",
      "ffff",
    ].join(":");
    const next = takePairStartSlot(state, ip, now, {
      ipWindowSecs: windowSecs,
      ipLimit: 8,
      globalLimit: RATE_IP_MAP_MAX + 8,
    });
    assert.equal(next.ok, true, `full-length IPv6 ${i} must be recorded`);
    state = next.state;
  }
  assert.equal(Object.keys(state.ips).length, RATE_IP_MAP_MAX);
  const serialized = JSON.stringify({
    devices: {},
    codes: {},
    pairStart: state,
  });
  const bytes = Buffer.byteLength(serialized, "utf8");
  assert.ok(
    bytes < 128 * 1024,
    `serialized board with ${RATE_IP_MAP_MAX} IPv6 buckets is ${bytes} bytes`,
  );
});

test("rate-limit state is bounded regardless of concurrent callers", () => {
  const globalLimit = DEVICE_GLOBAL_LIMIT;
  const ipLimit = DEVICE_IP_LIMIT;
  let state = { global: [], ips: {} };
  for (let i = 0; i < globalLimit; i += 1) {
    const res = takeDeviceSlot(state, `203.0.113.${i % 256}`, 1_000);
    state = res.state;
  }
  const stateJson = JSON.stringify(state);
  const stateBytes = new TextEncoder().encode(stateJson).byteLength;
  assert.ok(
    stateBytes < 128 * 1024,
    `rate-limit state must fit in 128 KiB, was ${stateBytes} bytes`,
  );
});

test("pair/start is rate-limited before shards are reserved", () => {
  let state = { global: [], ips: {} };
  for (let i = 0; i < PAIR_START_IP_LIMIT; i += 1) {
    const allowed = takePairStartSlot(state, "203.0.113.1", 1_000);
    assert.equal(allowed.ok, true, `attempt ${i + 1} should pass`);
    state = allowed.state;
  }
  const denied = takePairStartSlot(state, "203.0.113.1", 1_000);
  assert.equal(denied.ok, false);
  assert.equal(denied.status, 429);
  assert.equal(denied.error, "rate_limited");

  const other = takePairStartSlot(denied.state, "203.0.113.2", 1_000);
  assert.equal(other.ok, true, "a different IP still has budget");

  let flooded = { global: [], ips: {} };
  for (let i = 0; i < PAIR_START_GLOBAL_LIMIT; i += 1) {
    const allowed = takePairStartSlot(flooded, `198.51.100.${i}`, 2_000, {
      ipLimit: 1,
      globalLimit: PAIR_START_GLOBAL_LIMIT,
    });
    assert.equal(allowed.ok, true);
    flooded = allowed.state;
  }
  const globalDenied = takePairStartSlot(flooded, "198.51.100.254", 2_000, {
    ipLimit: 1,
    globalLimit: PAIR_START_GLOBAL_LIMIT,
  });
  assert.equal(globalDenied.ok, false, "unique IDs must not allocate unbounded shards");

  const later = takePairStartSlot(denied.state, "203.0.113.1", 1_000 + 60);
  assert.equal(later.ok, true, "the window must expire");

  const headers = new Headers({ "cf-connecting-ip": " 203.0.113.9 " });
  assert.equal(clientIp({ headers }), "203.0.113.9");

  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const index = fs.readFileSync(path.join(root, "worker/src/index.js"), "utf8");
  const rateAt = index.indexOf("rate:pair-start");
  const reserveAt = index.indexOf("await publishReservedPairing");
  assert.ok(
    rateAt >= 0 && rateAt < reserveAt,
    "do not allocate pair/device shards before the rate gate",
  );
});

test("list fan-out is rate-limited before selecting shards", () => {
  let state = { global: [], ips: {} };
  for (let i = 0; i < LIST_IP_LIMIT; i += 1) {
    const allowed = takeListSlot(state, "203.0.113.8", 3_000);
    assert.equal(allowed.ok, true);
    state = allowed.state;
  }
  const denied = takeListSlot(state, "203.0.113.8", 3_000);
  assert.equal(denied.ok, false);
  assert.equal(denied.status, 429);

  let flooded = { global: [], ips: {} };
  const floodCap = 8;
  for (let i = 0; i < floodCap; i += 1) {
    const allowed = takeListSlot(flooded, `198.51.100.${i % 250}`, 4_000, {
      ipLimit: floodCap,
      globalLimit: floodCap,
    });
    assert.equal(allowed.ok, true);
    flooded = allowed.state;
  }
  const globalDenied = takeListSlot(flooded, "203.0.113.9", 4_000, {
    ipLimit: floodCap,
    globalLimit: floodCap,
  });
  assert.equal(
    globalDenied.ok,
    false,
    "repeating 32-id lists must not create unbounded shard traffic",
  );

  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const index = fs.readFileSync(path.join(root, "worker/src/index.js"), "utf8");
  const rateAt = index.indexOf("internal/list-rate");
  const fanoutAt = index.indexOf("await collectListParts");
  assert.ok(
    rateAt >= 0 && rateAt < fanoutAt,
    "do not fan out /api/list before the rate gate",
  );
});

test("list global rate limit retains headroom for legacy polling", () => {
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const client = fs.readFileSync(path.join(root, "site/assets/board.js"), "utf8");
  const pollMs = 2500; // Old boards still poll during a rolling upgrade.
  assert.ok(client.includes("fallbackAt < 30000"), "new boards use a slow fallback");
  const pollsPerBoardPerMin = Math.ceil(60_000 / pollMs);
  assert.ok(
    LIST_IP_LIMIT >= pollsPerBoardPerMin,
    "one open board must not hit the per-IP list cap",
  );
  assert.ok(
    LIST_IP_LIMIT >= pollsPerBoardPerMin * 8,
    "multiple open boards behind one NAT must not hit the per-IP list cap",
  );
  assert.ok(
    LIST_IP_LIMIT > pollsPerBoardPerMin * LIST_IP_MIN_BOARDS,
    "leave room for command and claim refreshes above the 2.5s polling baseline",
  );
  assert.ok(
    LIST_GLOBAL_LIMIT >= pollsPerBoardPerMin * 100,
    "a handful of open boards must not 429 every Mac offline",
  );
  assert.ok(
    LIST_GLOBAL_LIMIT > pollsPerBoardPerMin * LIST_GLOBAL_MIN_BOARDS,
    "leave room for extra refreshes above scheduled global polling",
  );
});

test("claim 429 is retryable not a bad pairing code", () => {
  const { src } = boardClientHelpers();
  assert.match(src, /retryLater:/);
  assert.match(src, /稍后再试/);
  assert.match(src, /try again in a moment/);
  const start = src.indexOf("function claimFailureMessage");
  assert.notEqual(start, -1, "claim must classify retryable failures");
  const end = src.indexOf("\n  async function claim(");
  assert.ok(end > start, "claimFailureMessage must sit next to claim()");
  const claimFailureMessage = new Function(
    `${src.slice(start, end)}\nreturn claimFailureMessage;`,
  )();
  const copy = { badCode: "bad", retryLater: "retry" };
  assert.equal(
    claimFailureMessage(
      { status: 429 },
      { ok: false, error: "rate_limited" },
      copy,
    ),
    "retry",
    "a rate-limit 429 is not an invalid pairing code",
  );
  assert.equal(
    claimFailureMessage({ status: 503 }, { ok: false }, copy),
    "retry",
    "a transient 5xx must ask the user to retry",
  );
  assert.equal(
    claimFailureMessage(
      { status: 404 },
      { ok: false, error: "unknown_code" },
      copy,
    ),
    "bad",
  );
  const claimAt = src.indexOf("async function claim(");
  const beginAt = src.indexOf("let claimPromise");
  const claimFn = src.slice(claimAt, beginAt);
  assert.match(
    claimFn,
    /claimFailureMessage/,
    "claim() must not map every failure to copy.badCode",
  );
});

test("device routes are rate-limited before opening shards", () => {
  let state = { global: [], ips: {} };
  for (let i = 0; i < DEVICE_IP_LIMIT; i += 1) {
    const allowed = takeDeviceSlot(state, "203.0.113.14", 7_000);
    assert.equal(allowed.ok, true);
    state = allowed.state;
  }
  const denied = takeDeviceSlot(state, "203.0.113.14", 7_000);
  assert.equal(denied.ok, false);
  assert.equal(denied.status, 429);

  let flooded = { global: [], ips: {} };
  const floodCap = 8;
  for (let i = 0; i < floodCap; i += 1) {
    const allowed = takeDeviceSlot(flooded, `198.51.100.${i % 250}`, 8_000, {
      ipLimit: floodCap,
      globalLimit: floodCap,
    });
    assert.equal(allowed.ok, true);
    flooded = allowed.state;
  }
  const globalDenied = takeDeviceSlot(flooded, "203.0.113.15", 8_000, {
    ipLimit: floodCap,
    globalLimit: floodCap,
  });
  assert.equal(
    globalDenied.ok,
    false,
    "unique device ids must not open unbounded shards",
  );

  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const index = fs.readFileSync(path.join(root, "worker/src/index.js"), "utf8");
  const board = fs.readFileSync(path.join(root, "worker/src/board.js"), "utf8");
  const afterBusy = index.indexOf('started.error || "pair_busy"');
  const lastShard = index.lastIndexOf("const res = await stubFetch(env, name");
  assert.ok(afterBusy >= 0 && lastShard > afterBusy);
  const region = index.slice(afterBusy, lastShard);
  assert.ok(
    region.includes('"/api/heartbeat"') && region.includes('"/api/command"'),
    "only heartbeat and command may reach the device catch-all",
  );
  const notFoundAt = region.indexOf("not_found");
  const rateAt = region.indexOf("await deviceEntryGate");
  assert.ok(
    notFoundAt >= 0,
    "unknown /api paths must 404 before opening a device shard",
  );
  assert.ok(rateAt >= 0, "legacy deployments must retain rate:device as fallback");
  assert.ok(
    notFoundAt < rateAt,
    "unknown /api paths must 404 before the rate shard",
  );
  assert.ok(index.includes("internal/device-rate"));
  assert.match(board, /export function takeDeviceSlot/);
});

test("legacy rate limit still covers old one-second clients", () => {
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const panel = fs.readFileSync(
    path.join(root, "crates/never-sleep/src/panel.rs"),
    "utf8",
  );
  assert.match(panel, /PANEL_TICK_ACTIVE_MS:\s*u64\s*=\s*1_000/);
  assert.equal(
    DEVICE_HEARTBEAT_INTERVAL_MS,
    1000,
    "rolling upgrades must allow the old panel-driven heartbeat rate",
  );
  const beatsPerMacPerMin = Math.ceil(60_000 / DEVICE_HEARTBEAT_INTERVAL_MS);
  assert.ok(
    DEVICE_IP_LIMIT >= beatsPerMacPerMin * DEVICE_IP_MIN_MACS,
    "eight active Macs behind one NAT must not 429 heartbeats",
  );
  assert.ok(
    DEVICE_IP_LIMIT > beatsPerMacPerMin * DEVICE_IP_MIN_MACS,
    "leave room for command and UI-triggered refreshes on the same IP",
  );
  assert.ok(
    DEVICE_GLOBAL_LIMIT >= beatsPerMacPerMin * DEVICE_GLOBAL_MIN_MACS,
    "aggregate active heartbeats must not 429 every Mac offline",
  );
});

test("pair/claim is rate-limited before opening pair shards", () => {
  let state = { global: [], ips: {} };
  for (let i = 0; i < PAIR_CLAIM_IP_LIMIT; i += 1) {
    const allowed = takeClaimSlot(state, "203.0.113.11", 5_000);
    assert.equal(allowed.ok, true);
    state = allowed.state;
  }
  const denied = takeClaimSlot(state, "203.0.113.11", 5_000);
  assert.equal(denied.ok, false);
  assert.equal(denied.status, 429);

  let flooded = { global: [], ips: {} };
  const floodCap = 8;
  for (let i = 0; i < floodCap; i += 1) {
    const allowed = takeClaimSlot(flooded, `198.51.100.${i % 250}`, 6_000, {
      ipLimit: floodCap,
      globalLimit: floodCap,
    });
    assert.equal(allowed.ok, true);
    flooded = allowed.state;
  }
  const globalDenied = takeClaimSlot(flooded, "203.0.113.12", 6_000, {
    ipLimit: floodCap,
    globalLimit: floodCap,
  });
  assert.equal(globalDenied.ok, false, "unique codes must not open unbounded pair shards");

  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const index = fs.readFileSync(path.join(root, "worker/src/index.js"), "utf8");
  const claimAt = index.indexOf('path === "/api/pair/claim"');
  const startAt = index.indexOf('path === "/api/pair/start"');
  assert.ok(claimAt >= 0 && startAt > claimAt);
  const block = index.slice(claimAt, startAt);
  const rateAt = block.indexOf("rate:pair-claim");
  const peekAt = block.indexOf("internal/pair-peek");
  assert.ok(
    rateAt >= 0 && rateAt < peekAt,
    "do not open pair shards before the claim rate gate",
  );
});

test("alarm failure during pair reservation releases the shard", async () => {
  const released = [];
  const result = await publishReservedPairing({
    generateCode: () => (released.length ? "BBBB2222" : "AAAA1111"),
    reserve: async (code) => {
      if (code === "AAAA1111") throw new Error("transient setAlarm");
      return { ok: true };
    },
    startDevice: async (code) => ({
      ok: true,
      pairing_code: code,
      status: 200,
    }),
    release: async (code) => {
      released.push(code);
    },
  });
  assert.deepEqual(
    released,
    ["AAAA1111"],
    "a persisted pair shard without an alarm must be dropped",
  );
  assert.equal(result.pairing_code, "BBBB2222");
});

test("startDevice failure during pair reservation releases the shard", async () => {
  const released = [];
  const forgotten = [];
  const result = await publishReservedPairing({
    generateCode: () => (released.length ? "BBBB2222" : "AAAA1111"),
    reserve: async () => ({ ok: true }),
    startDevice: async (code) => {
      if (code === "AAAA1111") throw new Error("durable object put failed");
      return {
        ok: true,
        pairing_code: code,
        status: 200,
      };
    },
    release: async (code) => {
      released.push(code);
    },
    forgetDevice: async (code) => {
      forgotten.push(code);
    },
  });
  assert.deepEqual(
    released,
    ["AAAA1111"],
    "a reserved pair shard must be dropped if device startup throws",
  );
  assert.deepEqual(
    forgotten,
    ["AAAA1111"],
    "an ambiguous startDevice throw must drop the device offer so heartbeat cannot advertise it",
  );
  assert.equal(result.pairing_code, "BBBB2222");
});

test("startDevice throw keeps the pair shard when device forget fails", async () => {
  const released = [];
  const result = await publishReservedPairing({
    generateCode: () => "AAAA1111",
    reserve: async () => ({ ok: true }),
    startDevice: async () => {
      throw new Error("setAlarm after persist");
    },
    release: async (code) => {
      released.push(code);
    },
    forgetDevice: async () => {
      throw new Error("device durable object down");
    },
  });
  assert.deepEqual(
    released,
    [],
    "do not drop the pair shard while the device offer may still be advertised",
  );
  assert.equal(result.ok, false);
  assert.equal(result.error, "pair_busy");
  assert.equal(result.status, 503);
});

test("confirmLive failure during pair reservation releases the shard", async () => {
  const released = [];
  const forgotten = [];
  const result = await publishReservedPairing({
    generateCode: () => "AAAA1111",
    reserve: async () => ({ ok: true }),
    startDevice: async (code) => ({
      ok: true,
      pairing_code: code,
      status: 200,
    }),
    confirmLive: async () => {
      throw new Error("live-pairing durable object down");
    },
    release: async (code) => {
      released.push(code);
    },
    forgetDevice: async (code) => {
      forgotten.push(code);
    },
  });
  assert.deepEqual(
    released,
    ["AAAA1111"],
    "a reserved pair shard must be dropped if live confirmation throws",
  );
  assert.deepEqual(
    forgotten,
    ["AAAA1111"],
    "the device offer must be dropped so heartbeat cannot advertise an unclaimable code",
  );
  assert.equal(result.ok, false);
  assert.equal(result.error, "pair_busy");
  assert.equal(result.status, 503);
});

test("dropping an unconfirmed device offer keeps heartbeat from advertising it", () => {
  const board = new Board(() => 1_000);
  const id = identity();
  const started = board.startPairing({
    deviceId: id.device_id,
    deviceToken: id.device_token,
    displayName: id.display_name,
  });
  assert.equal(started.ok, true);
  board.dropOffer(started.pairing_code);
  const beat = board.heartbeat({
    deviceId: id.device_id,
    deviceToken: id.device_token,
    displayName: id.display_name,
    status: sampleStatus(),
  });
  assert.equal(
    beat.ok,
    false,
    "an unverified device whose last offer was dropped must not stay around for heartbeat",
  );
  assert.equal(beat.error, "unauthorized");
});

test("dropping the last unconfirmed offer removes an unverified device", () => {
  const board = new Board(() => 1_000);
  const id = identity();
  const started = board.startPairing({
    deviceId: id.device_id,
    deviceToken: id.device_token,
    displayName: id.display_name,
  });
  assert.equal(board.devices.get(id.device_id)?.lastSeen, null);
  board.dropOffer(started.pairing_code);
  assert.equal(
    board.devices.has(id.device_id),
    false,
    "transient confirmLive failures must not strand a lastSeen=null device shard",
  );
});

test("dropping an offer keeps a device that has already heartbeated", () => {
  const board = new Board(() => 1_000);
  const id = identity();
  const started = board.startPairing({
    deviceId: id.device_id,
    deviceToken: id.device_token,
    displayName: id.display_name,
  });
  const beat = board.heartbeat({
    deviceId: id.device_id,
    deviceToken: id.device_token,
    displayName: id.display_name,
    status: sampleStatus(),
  });
  assert.equal(beat.ok, true);
  board.dropOffer(started.pairing_code);
  assert.equal(board.devices.has(id.device_id), true);
  assert.notEqual(board.devices.get(id.device_id).lastSeen, null);
});

test("forgetDevice failure keeps the pair shard so a later claim is not 404", async () => {
  const released = [];
  const result = await publishReservedPairing({
    generateCode: () => "AAAA1111",
    reserve: async () => ({ ok: true }),
    startDevice: async (code) => ({
      ok: true,
      pairing_code: code,
      status: 200,
    }),
    confirmLive: async () => {
      throw new Error("live-pairing durable object down");
    },
    release: async (code) => {
      released.push(code);
    },
    forgetDevice: async () => {
      throw new Error("device durable object down");
    },
  });
  assert.deepEqual(
    released,
    [],
    "do not drop the pair shard while the device offer may still be advertised",
  );
  assert.equal(result.ok, false);
  assert.equal(result.error, "pair_busy");
  assert.equal(result.status, 503);
});

test("pair start forgets the device offer when live confirmation fails", () => {
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const index = fs.readFileSync(path.join(root, "worker/src/index.js"), "utf8");
  const startAt = index.indexOf('path === "/api/pair/start"');
  const nextAt = index.indexOf("path !== \"/api/heartbeat\"", startAt);
  assert.ok(startAt >= 0 && nextAt > startAt);
  const block = index.slice(startAt, nextAt);
  const forgetAt = block.indexOf("forgetDevice");
  assert.ok(forgetAt >= 0, "unconfirmed pair/start must drop the device offer");
  const forgetFn = block.slice(forgetAt, forgetAt + 350);
  assert.ok(
    forgetFn.includes("internal/pair-drop") && forgetFn.includes("name"),
    "device-offer drop must target the device shard, not pair:{code}",
  );
  assert.equal(
    forgetFn.includes("`pair:"),
    false,
    "do not delete only the pair shard when abandoning an unconfirmed start",
  );
});

test("release failure after startDevice throw still retries the next code", async () => {
  const released = [];
  const result = await publishReservedPairing({
    generateCode: () => (released.length ? "BBBB2222" : "AAAA1111"),
    reserve: async () => ({ ok: true }),
    startDevice: async (code) => {
      if (code === "AAAA1111") throw new Error("durable object put failed");
      return {
        ok: true,
        pairing_code: code,
        status: 200,
      };
    },
    release: async (code) => {
      released.push(code);
      if (code === "AAAA1111") throw new Error("drop pair shard failed");
    },
  });
  assert.deepEqual(released, ["AAAA1111"]);
  assert.equal(result.pairing_code, "BBBB2222");
});

test("pair start still returns the new code when replaced-shard cleanup fails", () => {
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const index = fs.readFileSync(path.join(root, "worker/src/index.js"), "utf8");
  assert.match(
    index,
    /bestEffortCleanup\([\s\S]*replaced_codes/,
    "replaced pair-shard drops must not block returning a live code",
  );
});

test("explicit display sleep is authenticated, delivered and acknowledged", async () => {
  let now = 1000;
  const board = new Board(() => now);
  const id = identity();
  await post(board, "/api/pair/start", id);
  await post(board, "/api/heartbeat", { ...id, status: sampleStatus() });
  const denied = await json(await post(board, "/api/command", {
    ...id, device_token: "00".repeat(32), cmd: "sleep_display",
  }));
  assert.equal(denied.status, 401);
  const sent = await json(await post(board, "/api/command", { ...id, cmd: "sleep_display" }));
  assert.equal(sent.body.accepted, true);
  const beat = await json(await post(board, "/api/heartbeat", { ...id, status: sampleStatus() }));
  assert.equal(beat.body.commands[0].cmd, "sleep_display");
  const ack = await json(await post(board, "/api/heartbeat", {
    ...id, status: sampleStatus(), ack_command_ids: [sent.body.command_id],
  }));
  assert.deepEqual(ack.body.commands, []);
  now += HEARTBEAT_TTL_SECS + 1;
  const offline = await json(await post(board, "/api/command", { ...id, cmd: "sleep_display" }));
  assert.equal(offline.status, 409);
});

test("board polling preserves the coin and only standby offers display sleep", () => {
  class Node {
    constructor(tag) { this.tag = tag; this.children = []; this.attrs = {}; }
    appendChild(node) { return this.insertBefore(node, null); }
    insertBefore(node, before) {
      node.remove();
      const index = before ? this.children.indexOf(before) : this.children.length;
      this.children.splice(index, 0, node); node.parentNode = this; return node;
    }
    remove() {
      if (this.parentNode) {
        const nodes = this.parentNode.children;
        nodes.splice(nodes.indexOf(this), 1); this.parentNode = null;
      }
    }
    replaceChildren(...nodes) { for (const n of [...this.children]) n.remove(); nodes.forEach(n => this.appendChild(n)); }
    setAttribute(key, value) { this.attrs[key] = value; }
    addEventListener() {}
    querySelector(selector) { return this.children.find(n => selector === ".device-coin" && n.className === "device-coin"); }
  }
  const list = new Node("div"), empty = new Node("div");
  const document = {
    createElement: tag => new Node(tag),
    getElementById: id => id === "device-list" ? list : empty,
    querySelector: () => ({ href: "https://example.com/assets/favicon.png" }),
  };
  const { src } = boardClientHelpers();
  const render = new Function("document", `const zh = false, copy = {}, lastStatuses = [];
    ${src.slice(src.indexOf("  function formatLastSeen"), src.indexOf("  let pendingByDevice"))}
    return render;`)(document);
  const devices = [{ device_id: "mac", display_name: "Mac" }];
  const status = active => [{ device_id: "mac", online: true, active }];
  render(devices, status(false), {});
  const card = list.children[0], coin = card.querySelector(".device-coin");
  render(devices, status(false), {});
  assert.equal(list.children[0], card, "polling must preserve the card");
  assert.equal(card.querySelector(".device-coin"), coin, "polling must preserve the decoded image");
  const sleepButton = () => list.children[0].children.find(n => n.className === "device-actions").children.find(n => n.attrs["data-cmd"] === "sleep_display");
  assert.equal(sleepButton().hidden, true, "sun mode hides Sleep Display Now");
  render(devices, status(true), {});
  assert.equal(card.querySelector(".device-coin"), coin);
  assert.match(coin.src, /moon.png$/);
  assert.equal(sleepButton().hidden, false);
  render([], [], {});
  assert.equal(list.children.length, 0);
});
