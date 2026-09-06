//! Phone-board Worker: pairing, heartbeats, and authenticated standby and display control.
//!
//! The Durable Object wrapper lives in `index.js`. This module is the policy
//! the tests lock — no Cloudflare runtime required.

export const HEARTBEAT_TTL_SECS = 35;
export const PAIRING_TTL_SECS = 10 * 60;
export const PAIRING_CODE_LEN = 8;
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const PUBLIC_SITE_ORIGIN = "https://xyz-ai.app/never-sleep";

export function tokensMatch(left, right) {
  if (typeof left !== "string" || typeof right !== "string") return false;
  if (left.length !== right.length) return false;
  let acc = 0;
  for (let i = 0; i < left.length; i += 1) {
    acc |= left.charCodeAt(i) ^ right.charCodeAt(i);
  }
  return acc === 0;
}

export function normalizePairingCode(raw) {
  if (typeof raw !== "string") return null;
  let out = "";
  for (const ch of raw) {
    if (ch === "-" || ch.trim() === "") continue;
    const up = ch.toUpperCase();
    const mapped = up === "I" || up === "L" ? "1" : up === "O" ? "0" : up;
    if (!CROCKFORD.includes(mapped)) return null;
    out += mapped;
    if (out.length > PAIRING_CODE_LEN) return null;
  }
  return out.length === PAIRING_CODE_LEN ? out : null;
}

export function formatPairingCode(code) {
  if (code.length === PAIRING_CODE_LEN) {
    return `${code.slice(0, 4)}-${code.slice(4)}`;
  }
  return code;
}

export function isChineseLang(lang) {
  if (typeof lang !== "string") return false;
  const primary = lang
    .trim()
    .toLowerCase()
    .replaceAll("_", "-")
    .split(/[-.@]/)[0];
  return (
    primary === "zh" ||
    primary === "chi" ||
    primary === "chinese" ||
    primary === "cn"
  );
}

export function publicSiteOrigin(requestUrl, env = {}) {
  const fromEnv = env.PUBLIC_SITE_ORIGIN || env.NEVER_SLEEP_CLOUD_URL;
  if (typeof fromEnv === "string" && fromEnv.trim()) {
    return fromEnv.trim().replace(/\/+$/, "");
  }
  let url;
  try {
    url = new URL(requestUrl);
  } catch {
    return PUBLIC_SITE_ORIGIN;
  }
  const host = url.hostname;
  if (host === "xyz-ai.app" || host === "www.xyz-ai.app") {
    return `${url.protocol}//${host}/never-sleep`;
  }
  return `${url.protocol}//${url.host}`;
}

export function pairingUrl(code, chinese, origin) {
  const base = (origin || PUBLIC_SITE_ORIGIN).replace(/\/+$/, "");
  const path = chinese ? "/zh/board/" : "/board/";
  return `${base}${path}?code=${formatPairingCode(code)}`;
}

export function pairingCodeIsLive(liveCode, candidateCode) {
  const live = normalizePairingCode(liveCode);
  const cand = normalizePairingCode(candidateCode);
  return Boolean(live && cand && live === cand);
}

export function deviceIsOnline(lastSeenUnix, nowUnix) {
  if (nowUnix < lastSeenUnix) return true;
  return nowUnix - lastSeenUnix <= HEARTBEAT_TTL_SECS;
}

export function shardName(path, body) {
  const p = (path || "").replace(/\/+$/, "") || "/";
  if (p === "/api/pair/claim") {
    const code = normalizePairingCode(body?.pairing_code);
    return code ? `pair:${code}` : null;
  }
  if (p === "/api/list") return null;
  if (!deviceCredentialsAreValid(body?.device_id, body?.device_token)) return null;
  return `device:${body.device_id}`;
}

export const LIST_MAX_DEVICES = 32;
/** Phone-board cards and localStorage reservations share this cap. */
export const MAX_DISPLAY_NAME_CHARS = 128;
export const DEVICE_ID_LEN = 32;
export const DEVICE_TOKEN_LEN = 64;

export function isHexLen(value, len) {
  return typeof value === "string" && value.length === len && /^[0-9a-f]+$/i.test(value);
}

/** Reject attacker-controlled credentials before `idFromName` or DO storage. */
export function deviceCredentialsAreValid(deviceId, deviceToken) {
  return isHexLen(deviceId, DEVICE_ID_LEN) && isHexLen(deviceToken, DEVICE_TOKEN_LEN);
}

export function boundDisplayName(name) {
  if (typeof name !== "string") return "Mac";
  const trimmed = name.trim();
  if (!trimmed) return "Mac";
  return [...trimmed].slice(0, MAX_DISPLAY_NAME_CHARS).join("");
}

export const PAIR_START_IP_LIMIT = 8;
export const PAIR_START_IP_WINDOW_SECS = 60;
export const PAIR_START_GLOBAL_LIMIT = 60;
export const PAIR_START_GLOBAL_WINDOW_SECS = 60;
export const LIST_IP_WINDOW_SECS = 60;
/** Legacy boards still poll at this rate during a rolling upgrade. */
export const LIST_POLL_INTERVAL_MS = 2500;
/** Concurrent open boards one household/corporate NAT is sized to survive. */
export const LIST_IP_MIN_BOARDS = 8;
export const LIST_IP_LIMIT =
  Math.ceil(60_000 / LIST_POLL_INTERVAL_MS) * (LIST_IP_MIN_BOARDS + 1);
/** Concurrent open boards the global list cap is sized to survive. */
export const LIST_GLOBAL_MIN_BOARDS = 200;
export const LIST_GLOBAL_LIMIT =
  Math.ceil(60_000 / LIST_POLL_INTERVAL_MS) * (LIST_GLOBAL_MIN_BOARDS + 1);
export const LIST_GLOBAL_WINDOW_SECS = 60;
export const PAIR_CLAIM_IP_LIMIT = 20;
export const PAIR_CLAIM_IP_WINDOW_SECS = 60;
export const PAIR_CLAIM_GLOBAL_LIMIT = 180;
export const PAIR_CLAIM_GLOBAL_WINDOW_SECS = 60;
/** Legacy clients still send on the one-second panel clock; retain headroom. */
export const DEVICE_HEARTBEAT_INTERVAL_MS = 1000;
export const DEVICE_IP_MIN_MACS = 8;
const DEVICE_BEATS_PER_MAC_PER_MIN = Math.ceil(
  60_000 / DEVICE_HEARTBEAT_INTERVAL_MS,
);
export const DEVICE_IP_LIMIT =
  DEVICE_BEATS_PER_MAC_PER_MIN * (DEVICE_IP_MIN_MACS + 1);
export const DEVICE_IP_WINDOW_SECS = 60;
export const DEVICE_GLOBAL_MIN_MACS = 200;
export const DEVICE_GLOBAL_LIMIT =
  DEVICE_BEATS_PER_MAC_PER_MIN * (DEVICE_GLOBAL_MIN_MACS + 1);
export const DEVICE_GLOBAL_WINDOW_SECS = 60;
export const RATE_IP_MAP_MAX = 1024;

/**
 * Fixed-window bucket: { count, windowStart }.
 * O(1) memory per bucket regardless of request volume; safe to persist in DO storage.
 */
function bucketTake(bucket, nowSecs, windowSecs, limit) {
  const start = typeof bucket?.windowStart === "number" ? bucket.windowStart : 0;
  if (nowSecs - start >= windowSecs) {
    return { ok: true, bucket: { count: 1, windowStart: nowSecs } };
  }
  const count = (typeof bucket?.count === "number" ? bucket.count : 0) + 1;
  if (count > limit) {
    return { ok: false, bucket: { count: bucket.count, windowStart: start } };
  }
  return { ok: true, bucket: { count, windowStart: start } };
}

// Shared across heartbeat and commands, including across IPs and DO reloads.
// Leave room for old one-second clients during a rolling upgrade.
function takeAuthenticatedDeviceSlot(device, now) {
  const result = bucketTake(device.requestRate, now, 60, 120);
  if (result.ok) device.requestRate = result.bucket;
  return result.ok;
}

function pruneExpiredIpBuckets(ips, nowSecs, windowSecs) {
  for (const ip of Object.keys(ips)) {
    const bucket = ips[ip];
    if (
      !bucket ||
      typeof bucket.windowStart !== "number" ||
      nowSecs - bucket.windowStart >= windowSecs
    ) {
      delete ips[ip];
    }
  }
}

/** Decide whether /api/pair/start may allocate Durable Object shards. */
export function takePairStartSlot(state, ip, nowSecs, limits = {}) {
  const ipLimit = limits.ipLimit ?? PAIR_START_IP_LIMIT;
  const ipWindow = limits.ipWindowSecs ?? PAIR_START_IP_WINDOW_SECS;
  const globalLimit = limits.globalLimit ?? PAIR_START_GLOBAL_LIMIT;
  const globalWindow = limits.globalWindowSecs ?? PAIR_START_GLOBAL_WINDOW_SECS;
  const key = typeof ip === "string" && ip.trim() ? ip.trim() : "unknown";
  const globalResult = bucketTake(state?.global, nowSecs, globalWindow, globalLimit);
  if (!globalResult.ok) {
    return { ok: false, error: "rate_limited", status: 429, state };
  }
  const ips = state?.ips && typeof state.ips === "object" ? { ...state.ips } : {};
  pruneExpiredIpBuckets(ips, nowSecs, ipWindow);
  const ipResult = bucketTake(ips[key], nowSecs, ipWindow, ipLimit);
  if (!ipResult.ok) {
    return {
      ok: false,
      error: "rate_limited",
      status: 429,
      state: { global: state?.global ?? null, ips },
    };
  }
  ips[key] = ipResult.bucket;
  const names = Object.keys(ips);
  if (names.length > RATE_IP_MAP_MAX) {
    names.sort((a, b) => (ips[a]?.windowStart ?? 0) - (ips[b]?.windowStart ?? 0));
    for (const drop of names.slice(0, names.length - RATE_IP_MAP_MAX)) {
      delete ips[drop];
    }
  }
  return { ok: true, status: 200, state: { global: globalResult.bucket, ips } };
}

export function takeListSlot(state, ip, nowSecs, limits = {}) {
  return takePairStartSlot(state, ip, nowSecs, {
    ipLimit: limits.ipLimit ?? LIST_IP_LIMIT,
    ipWindowSecs: limits.ipWindowSecs ?? LIST_IP_WINDOW_SECS,
    globalLimit: limits.globalLimit ?? LIST_GLOBAL_LIMIT,
    globalWindowSecs: limits.globalWindowSecs ?? LIST_GLOBAL_WINDOW_SECS,
  });
}

export function takeClaimSlot(state, ip, nowSecs, limits = {}) {
  return takePairStartSlot(state, ip, nowSecs, {
    ipLimit: limits.ipLimit ?? PAIR_CLAIM_IP_LIMIT,
    ipWindowSecs: limits.ipWindowSecs ?? PAIR_CLAIM_IP_WINDOW_SECS,
    globalLimit: limits.globalLimit ?? PAIR_CLAIM_GLOBAL_LIMIT,
    globalWindowSecs: limits.globalWindowSecs ?? PAIR_CLAIM_GLOBAL_WINDOW_SECS,
  });
}

export function takeDeviceSlot(state, ip, nowSecs, limits = {}) {
  return takePairStartSlot(state, ip, nowSecs, {
    ipLimit: limits.ipLimit ?? DEVICE_IP_LIMIT,
    ipWindowSecs: limits.ipWindowSecs ?? DEVICE_IP_WINDOW_SECS,
    globalLimit: limits.globalLimit ?? DEVICE_GLOBAL_LIMIT,
    globalWindowSecs: limits.globalWindowSecs ?? DEVICE_GLOBAL_WINDOW_SECS,
  });
}

export function clientIp(request) {
  const cf = request?.headers?.get?.("cf-connecting-ip");
  if (typeof cf === "string" && cf.trim()) return cf.trim();
  const forwarded = request?.headers?.get?.("x-forwarded-for");
  if (typeof forwarded === "string" && forwarded.trim()) {
    return forwarded.split(",")[0].trim() || "unknown";
  }
  return "unknown";
}

export const PAIR_RESERVE_ATTEMPTS = 8;
/** Drop undelivered commands after a few missed heartbeats, not hours later. */
export const COMMAND_TTL_SECS = 60;
const U32_MAX = 0xffffffff;
/** `JsonStatus` counters are `Option<u64>`; JSON numbers stay exact through 2^53-1. */
const MAX_STATUS_SECS = Number.MAX_SAFE_INTEGER;

export function capListEntries(entries) {
  if (!Array.isArray(entries)) return [];
  const seen = new Set();
  const out = [];
  for (const entry of entries) {
    if (!deviceCredentialsAreValid(entry?.device_id, entry?.device_token)) continue;
    const id = entry.device_id;
    if (seen.has(id)) continue;
    seen.add(id);
    out.push(entry);
    if (out.length >= LIST_MAX_DEVICES) break;
  }
  return out;
}

export function fitStoredDevices(devices, entry, max = LIST_MAX_DEVICES) {
  const next = (Array.isArray(devices) ? devices : []).filter(
    (item) => item?.device_id !== entry.device_id,
  );
  next.push(entry);
  while (next.length > max) next.shift();
  return next;
}

export async function collectListParts(fetchEntry, entries) {
  const settled = await Promise.allSettled(
    (Array.isArray(entries) ? entries : []).map((entry) => fetchEntry(entry)),
  );
  const devices = [];
  for (const item of settled) {
    if (item.status !== "fulfilled") continue;
    if (Array.isArray(item.value)) devices.push(...item.value);
  }
  return devices;
}

function pairStartHasHits(pairStart) {
  if (!pairStart || typeof pairStart !== "object") return false;
  if (pairStart.global && typeof pairStart.global === "object" && pairStart.global.count > 0) return true;
  return Object.keys(pairStart.ips || {}).length > 0;
}

export function boardHasState(data) {
  if (!data || typeof data !== "object") return false;
  const devices = data.devices || {};
  const codes = data.codes || {};
  if (Object.keys(devices).length > 0 || Object.keys(codes).length > 0) {
    return true;
  }
  return (
    pairStartHasHits(data.pairStart) ||
    pairStartHasHits(data.listRate) ||
    pairStartHasHits(data.claimRate) ||
    pairStartHasHits(data.deviceRate)
  );
}

export function persistBoardAction(previous, next) {
  if (!boardHasState(next)) {
    if (previous == null) return "skip";
    return "delete";
  }
  if (JSON.stringify(previous) === JSON.stringify(next)) return "skip";
  return "put";
}

export function alarmNeedsUpdate(scheduledUnix, nextUnix) {
  if (scheduledUnix === undefined) return true;
  return scheduledUnix !== nextUnix;
}

export async function commitAlarmUnix(scheduledUnix, nextUnix, apply) {
  if (!alarmNeedsUpdate(scheduledUnix, nextUnix)) return scheduledUnix;
  await apply(nextUnix);
  return nextUnix;
}

/** After a board write, dropping a now-obsolete alarm must not fail the request. */
export async function commitPersistedAlarm(scheduledUnix, nextUnix, apply) {
  try {
    return await commitAlarmUnix(scheduledUnix, nextUnix, apply);
  } catch (err) {
    if (nextUnix == null) return scheduledUnix;
    throw err;
  }
}

export async function bestEffortCleanup(result, cleanup) {
  try {
    await cleanup();
  } catch {
    // Pair-shard drop is best-effort; the successful device response is already in hand.
  }
  return result;
}

/** Run async work one-at-a-time so overlapping callers cannot share a snapshot. */
export function createSerialQueue() {
  let tail = Promise.resolve();
  return (work) => {
    const run = tail.then(work);
    tail = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  };
}

function isUntilClock(raw) {
  const m = /^(\d{1,2}):(\d{2})$/.exec(raw);
  if (!m) return false;
  const hour = Number(m[1]);
  const minute = Number(m[2]);
  return hour <= 23 && minute <= 59;
}

function parseU32Hours(num) {
  if (typeof num !== "string" || !/^\d+$/.test(num) || num.length > 10) {
    return null;
  }
  const hours = Number(num);
  if (!Number.isInteger(hours) || hours < 1 || hours > U32_MAX) return null;
  return hours;
}

export function isAllowedDuration(raw) {
  if (typeof raw !== "string") return false;
  const s = raw.trim().toLowerCase();
  if (!s) return false;
  if (s === "indefinite" || s === "inf" || s === "forever" || s === "无限") {
    return true;
  }
  let clock = null;
  if (s.startsWith("until=")) clock = s.slice(6);
  else if (s.startsWith("until:")) clock = s.slice(6);
  else if (isUntilClock(s)) clock = s;
  if (clock != null) return isUntilClock(clock);
  if (s.endsWith("h")) return parseU32Hours(s.slice(0, -1)) != null;
  if (s.endsWith("小时")) return parseU32Hours(s.slice(0, -2).trim()) != null;
  return false;
}

export async function publishReservedPairing({
  generateCode,
  reserve,
  startDevice,
  release,
  confirmLive,
  forgetDevice,
  maxAttempts = PAIR_RESERVE_ATTEMPTS,
}) {
  const drop = (code) =>
    release ? bestEffortCleanup(undefined, () => release(code)) : Promise.resolve();
  const forget = async (code) => {
    if (!forgetDevice) return true;
    try {
      await forgetDevice(code);
      return true;
    } catch {
      return false;
    }
  };
  for (let i = 0; i < maxAttempts; i++) {
    const code = generateCode();
    let reserved;
    try {
      reserved = await reserve(code);
    } catch {
      await drop(code);
      continue;
    }
    if (!reserved?.ok) continue;
    let started;
    try {
      started = await startDevice(code);
    } catch {
      const forgotten = await forget(code);
      if (forgotten) {
        await drop(code);
      }
      continue;
    }
    if (started?.ok) {
      if (confirmLive) {
        let live = false;
        try {
          live = await confirmLive(code, started);
        } catch {
          live = false;
        }
        if (!live) {
          const forgotten = await forget(code);
          if (forgotten) {
            await drop(code);
          }
          return { ok: false, error: "pair_busy", status: 503 };
        }
      }
      return started;
    }
    await drop(code);
  }
  return { ok: false, error: "pair_busy", status: 503 };
}

function asBool(value) {
  return value === true;
}

function asInt(value, min, max) {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  const n = Math.round(value);
  if (n < min || n > max) return null;
  return n;
}

function asStopReasonCode(value) {
  if (typeof value !== "string") return null;
  return /^[a-z][a-z0-9_]{0,31}$/.test(value) ? value : null;
}

export function sanitizeStatus(raw) {
  const src = raw && typeof raw === "object" ? raw : {};
  return {
    active: asBool(src.active),
    display: src.display === "asleep" ? "asleep" : "awake",
    lid: src.lid === "closed" ? "closed" : "open",
    on_ac: asBool(src.on_ac),
    battery: asInt(src.battery, 0, 100),
    remaining_secs: asInt(src.remaining_secs, 0, MAX_STATUS_SECS),
    user_present: asBool(src.user_present),
    elapsed_secs: asInt(src.elapsed_secs, 0, MAX_STATUS_SECS),
    stop_reason: null,
    stop_reason_code: asStopReasonCode(src.stop_reason_code),
    screen_off_enabled: asBool(src.screen_off_enabled),
    lid_awake_enabled: asBool(src.lid_awake_enabled),
  };
}

function randomHex(bytes) {
  const buf = new Uint8Array(bytes);
  crypto.getRandomValues(buf);
  return [...buf].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function proposePairingCode() {
  const buf = new Uint8Array(5);
  crypto.getRandomValues(buf);
  let bits = 0n;
  for (const b of buf) bits = (bits << 8n) | BigInt(b);
  let out = "";
  for (let i = 0; i < PAIRING_CODE_LEN; i += 1) {
    out = CROCKFORD[Number(bits & 31n)] + out;
    bits >>= 5n;
  }
  return out;
}

export function jsonResponse(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });
}

function emptyStatus() {
  return sanitizeStatus({
    active: false,
    display: "awake",
    lid: "open",
    on_ac: true,
    screen_off_enabled: true,
    lid_awake_enabled: true,
  });
}

export class Board {
  constructor(nowSecs = () => Math.floor(Date.now() / 1000)) {
    this.nowSecs = nowSecs;
    /** @type {Map<string, object>} */
    this.devices = new Map();
    /** @type {Map<string, { deviceId: string, expires: number, token?: string, displayName?: string }>} */
    this.codes = new Map();
    this.pairStart = { global: null, ips: {} };
    this.listRate = { global: null, ips: {} };
    this.claimRate = { global: null, ips: {} };
    this.deviceRate = { global: null, ips: {} };
  }

  static fromJSON(data) {
    const board = new Board();
    if (!data) return board;
    for (const [id, device] of Object.entries(data.devices || {})) {
      board.devices.set(id, structuredClone(device));
    }
    for (const [code, offer] of Object.entries(data.codes || {})) {
      board.codes.set(code, structuredClone(offer));
    }
    function loadRate(src) {
      if (!src || typeof src !== "object") return { global: null, ips: {} };
      return {
        global: src.global && typeof src.global === "object" ? structuredClone(src.global) : null,
        ips: src.ips && typeof src.ips === "object" ? structuredClone(src.ips) : {},
      };
    }
    if (data.pairStart) board.pairStart = loadRate(data.pairStart);
    if (data.listRate) board.listRate = loadRate(data.listRate);
    if (data.claimRate) board.claimRate = loadRate(data.claimRate);
    if (data.deviceRate) board.deviceRate = loadRate(data.deviceRate);
    return board;
  }

  toJSON() {
    const devices = {};
    for (const [id, device] of this.devices) {
      devices[id] = structuredClone(device);
    }
    const codes = {};
    for (const [code, offer] of this.codes) {
      codes[code] = structuredClone(offer);
    }
    const out = { devices, codes };
    if (pairStartHasHits(this.pairStart)) {
      out.pairStart = structuredClone(this.pairStart);
    }
    if (pairStartHasHits(this.listRate)) {
      out.listRate = structuredClone(this.listRate);
    }
    if (pairStartHasHits(this.claimRate)) {
      out.claimRate = structuredClone(this.claimRate);
    }
    if (pairStartHasHits(this.deviceRate)) {
      out.deviceRate = structuredClone(this.deviceRate);
    }
    return out;
  }

  takePairStart(ip) {
    const result = takePairStartSlot(this.pairStart, ip, this.nowSecs());
    this.pairStart = result.state;
    return {
      ok: result.ok,
      error: result.error,
      status: result.status,
    };
  }

  takeList(ip) {
    const result = takeListSlot(this.listRate, ip, this.nowSecs());
    this.listRate = result.state;
    return {
      ok: result.ok,
      error: result.error,
      status: result.status,
    };
  }

  takeClaim(ip) {
    const result = takeClaimSlot(this.claimRate, ip, this.nowSecs());
    this.claimRate = result.state;
    return {
      ok: result.ok,
      error: result.error,
      status: result.status,
    };
  }

  takeDevice(ip) {
    const result = takeDeviceSlot(this.deviceRate, ip, this.nowSecs());
    this.deviceRate = result.state;
    return {
      ok: result.ok,
      error: result.error,
      status: result.status,
    };
  }

  #purgeCodes(now) {
    const expired = [];
    for (const [code, offer] of this.codes) {
      if (offer.expires <= now) {
        expired.push(code);
        this.codes.delete(code);
      }
    }
    return expired;
  }

  expireOffers() {
    const now = this.nowSecs();
    const expired = this.#purgeCodes(now);
    this.#dropUnverifiedDevices(now);
    return expired;
  }

  #dropUnverifiedDevices(now) {
    for (const [id, device] of this.devices) {
      if (device.lastSeen != null) continue;
      let liveOffer = false;
      for (const offer of this.codes.values()) {
        if (offer.deviceId === id && offer.expires > now) {
          liveOffer = true;
          break;
        }
      }
      if (!liveOffer) this.devices.delete(id);
    }
  }

  nextAlarmUnix() {
    let next = null;
    for (const offer of this.codes.values()) {
      if (next == null || offer.expires < next) next = offer.expires;
    }
    return next;
  }

  livePairing(deviceId) {
    const now = this.nowSecs();
    for (const [code, offer] of this.codes) {
      if (offer.deviceId === deviceId && offer.expires > now) {
        return {
          ok: true,
          pairing_code: formatPairingCode(code),
          status: 200,
        };
      }
    }
    return { ok: true, pairing_code: null, status: 200 };
  }

  peekOffer(rawCode) {
    const now = this.nowSecs();
    const code = normalizePairingCode(rawCode);
    if (!code) {
      return { ok: false, error: "unknown_code", status: 404 };
    }
    const offer = this.codes.get(code);
    if (!offer || offer.expires <= now) {
      if (code) this.codes.delete(code);
      return { ok: false, error: "unknown_code", status: 404 };
    }
    return { ok: true, device_id: offer.deviceId, status: 200 };
  }

  startPairing({
    deviceId,
    deviceToken,
    displayName,
    lang,
    origin,
    pairingCode,
    expiresUnix,
  }) {
    const now = this.nowSecs();
    if (!deviceCredentialsAreValid(deviceId, deviceToken)) {
      return { ok: false, error: "bad_identity", status: 400 };
    }
    const existing = this.devices.get(deviceId);
    if (existing && !tokensMatch(existing.token, deviceToken)) {
      return { ok: false, error: "unauthorized", status: 401 };
    }
    const name = boundDisplayName(displayName || existing?.displayName || "Mac");
    if (!existing) {
      this.devices.set(deviceId, {
        token: deviceToken,
        displayName: name,
        lastSeen: null,
        status: emptyStatus(),
        commands: [],
      });
    } else if (displayName) {
      existing.displayName = boundDisplayName(displayName);
    }
    const replacedCodes = this.#purgeCodes(now);
    for (const [code, offer] of this.codes) {
      if (offer.deviceId === deviceId) {
        replacedCodes.push(code);
        this.codes.delete(code);
      }
    }
    let code;
    if (pairingCode != null && pairingCode !== "") {
      code = normalizePairingCode(pairingCode);
      if (!code) {
        return { ok: false, error: "bad_code", status: 400 };
      }
    } else {
      code = proposePairingCode();
    }
    const expires =
      typeof expiresUnix === "number" && Number.isFinite(expiresUnix)
        ? expiresUnix
        : now + PAIRING_TTL_SECS;
    this.codes.set(code, {
      deviceId,
      token: deviceToken,
      displayName: name,
      expires,
    });
    const chinese = isChineseLang(lang);
    return {
      ok: true,
      pairing_code: formatPairingCode(code),
      pairing_url: pairingUrl(code, chinese, origin),
      expires_unix: expires,
      replaced_codes: replacedCodes,
      status: 200,
    };
  }

  rememberOffer({ pairingCode, deviceId, deviceToken, displayName, expiresUnix }) {
    const code = normalizePairingCode(pairingCode);
    if (
      !code ||
      !deviceCredentialsAreValid(deviceId, deviceToken)
    ) {
      return { ok: false, error: "bad_offer", status: 400 };
    }
    const existing = this.codes.get(code);
    if (existing) {
      if (
        existing.deviceId === deviceId &&
        tokensMatch(existing.token, deviceToken)
      ) {
        existing.displayName = boundDisplayName(displayName || existing.displayName);
        existing.expires = expiresUnix || existing.expires;
        return { ok: true, created: false, status: 200 };
      }
      return { ok: false, error: "taken", status: 409 };
    }
    this.codes.set(code, {
      deviceId,
      token: deviceToken,
      displayName: boundDisplayName(displayName || "Mac"),
      expires: expiresUnix || this.nowSecs() + PAIRING_TTL_SECS,
    });
    return { ok: true, created: true, status: 200 };
  }

  dropOffer(rawCode) {
    const code = normalizePairingCode(rawCode);
    if (code) this.codes.delete(code);
    this.#dropUnverifiedDevices(this.nowSecs());
    return { ok: true, status: 200 };
  }

  claim(rawCode) {
    const now = this.nowSecs();
    this.#purgeCodes(now);
    const code = normalizePairingCode(rawCode);
    if (!code) {
      return { ok: false, error: "unknown_code", status: 404 };
    }
    const offer = this.codes.get(code);
    if (!offer) {
      return { ok: false, error: "unknown_code", status: 404 };
    }
    this.codes.delete(code);
    const device = this.devices.get(offer.deviceId);
    const token = offer.token || device?.token;
    const displayName = offer.displayName || device?.displayName || "Mac";
    if (!token) {
      return { ok: false, error: "unknown_code", status: 404 };
    }
    return {
      ok: true,
      device_id: offer.deviceId,
      device_token: token,
      display_name: displayName,
      status: 200,
    };
  }

  heartbeat({
    deviceId,
    deviceToken,
    displayName,
    status,
    ackCommandIds,
    lang,
    origin,
    offline,
  }) {
    const now = this.nowSecs();
    const device = this.devices.get(deviceId);
    if (!device || !tokensMatch(device.token, deviceToken)) {
      return { ok: false, error: "unauthorized", status: 401 };
    }
    // Offline flushes must work even after the normal budget is exhausted.
    if (offline !== true && !takeAuthenticatedDeviceSlot(device, now)) {
      return { ok: false, error: "rate_limited", status: 429 };
    }
    if (displayName) device.displayName = boundDisplayName(displayName);
    if (status && typeof status === "object") {
      device.status = sanitizeStatus(status);
      device.statusAt = now;
    }
    if (offline === true) {
      device.lastSeen = now - HEARTBEAT_TTL_SECS - 1;
      device.commands = [];
    } else {
      device.lastSeen = now;
    }
    const acks = new Set(
      Array.isArray(ackCommandIds)
        ? ackCommandIds.filter((id) => typeof id === "string")
        : [],
    );
    device.commands = (device.commands || []).filter((item) => {
      if (acks.has(item.id)) return false;
      const queued = typeof item.queued_at === "number" ? item.queued_at : 0;
      return now - queued <= COMMAND_TTL_SECS;
    });
    const pending = device.commands.map((item) => {
      const out = { id: item.id, cmd: item.cmd };
      if (item.duration) out.duration = item.duration;
      return out;
    });
    const expiredCodes = this.#purgeCodes(now);
    let pairing = null;
    const chinese = isChineseLang(lang);
    for (const [code, offer] of this.codes) {
      if (offer.deviceId === deviceId) {
        pairing = {
          pairing_code: formatPairingCode(code),
          pairing_url: pairingUrl(code, chinese, origin),
          expires_unix: offer.expires,
        };
        break;
      }
    }
    return {
      ok: true,
      commands: pending,
      pairing_code: pairing?.pairing_code || null,
      pairing_url: pairing?.pairing_url || null,
      expires_unix: pairing?.expires_unix || null,
      expired_codes: expiredCodes,
      status: 200,
    };
  }

  effectiveLastSeen(deviceId, device) {
    const live = this.liveLastSeen?.(deviceId);
    return live == null ? device.lastSeen : Math.max(device.lastSeen || 0, live);
  }

  list(entriesIn) {
    if (!Array.isArray(entriesIn)) {
      return { ok: false, error: "bad_request", status: 400 };
    }
    const now = this.nowSecs();
    const devices = [];
    for (const entry of capListEntries(entriesIn)) {
      const deviceId = entry?.device_id;
      const token = entry?.device_token;
      const device = this.devices.get(deviceId);
      if (!device || !tokensMatch(device.token, token)) {
        continue;
      }
      const lastSeen = this.effectiveLastSeen(deviceId, device);
      const online = lastSeen != null && deviceIsOnline(lastSeen, now);
      const status = sanitizeStatus(device.status);
      // A live connection reports changes, not ticking counters. Extrapolate
      // from the last full sample without mutating or persisting the sample.
      if (online && status.active && typeof device.statusAt === "number") {
        const delta = Math.max(0, now - device.statusAt);
        if (status.elapsed_secs != null) status.elapsed_secs += delta;
        if (status.remaining_secs != null) status.remaining_secs = Math.max(0, status.remaining_secs - delta);
      }
      devices.push({
        device_id: deviceId,
        display_name: device.displayName,
        online,
        last_seen_unix: lastSeen,
        ...status,
      });
    }
    return { ok: true, devices, status: 200 };
  }

  command({ deviceId, deviceToken, cmd, duration }) {
    const now = this.nowSecs();
    const device = this.devices.get(deviceId);
    if (!device || !tokensMatch(device.token, deviceToken)) {
      return { ok: false, error: "unauthorized", status: 401 };
    }
    if (!takeAuthenticatedDeviceSlot(device, now)) {
      return { ok: false, error: "rate_limited", status: 429 };
    }
    if (cmd !== "on" && cmd !== "off" && cmd !== "sleep_display") {
      return { ok: false, error: "bad_cmd", status: 400 };
    }
    const lastSeen = this.effectiveLastSeen(deviceId, device);
    const online = lastSeen != null && deviceIsOnline(lastSeen, now);
    if (!online) {
      return { ok: false, error: "offline", accepted: false, status: 409 };
    }
    const item = {
      id: randomHex(8),
      cmd,
      queued_at: now,
    };
    if (cmd === "on" && duration != null && duration !== "") {
      if (typeof duration !== "string" || !isAllowedDuration(duration)) {
        return { ok: false, error: "bad_duration", status: 400 };
      }
      item.duration = duration.trim();
    }
    device.commands = device.commands || [];
    device.commands.push(item);
    return { ok: true, accepted: true, command_id: item.id, status: 200 };
  }
}

function requestOrigin(request, env = {}) {
  return (
    request.headers.get("x-public-origin") ||
    publicSiteOrigin(request.url, env)
  );
}

export async function handleInternal(board, request) {
  const url = new URL(request.url);
  const path = url.pathname.replace(/\/+$/, "") || "/";
  let body = {};
  try {
    body = await request.json();
  } catch {
    body = {};
  }
  let result;
  if (path === "/internal/pair-offer") {
    result = board.rememberOffer({
      pairingCode: body.pairing_code,
      deviceId: body.device_id,
      deviceToken: body.device_token,
      displayName: body.display_name,
      expiresUnix: body.expires_unix,
    });
  } else if (path === "/internal/pair-drop") {
    result = board.dropOffer(body.pairing_code);
  } else if (path === "/internal/pair-peek") {
    result = board.peekOffer(body.pairing_code);
  } else if (path === "/internal/live-pairing") {
    result = board.livePairing(body.device_id);
  } else if (path === "/internal/pair-rate") {
    result = board.takePairStart(body.ip);
  } else if (path === "/internal/list-rate") {
    result = board.takeList(body.ip);
  } else if (path === "/internal/claim-rate") {
    result = board.takeClaim(body.ip);
  } else if (path === "/internal/device-rate") {
    result = board.takeDevice(body.ip);
  } else {
    return jsonResponse({ ok: false, error: "not_found" }, 404);
  }
  const { status, ...payload } = result;
  return jsonResponse(payload, status);
}

export async function handleApi(board, request, env = {}) {
  const url = new URL(request.url);
  const path = url.pathname.replace(/\/+$/, "") || "/";
  if (request.method !== "POST") {
    return jsonResponse({ ok: false, error: "method" }, 405);
  }
  let body = {};
  try {
    body = await request.json();
  } catch {
    body = {};
  }
  const origin = requestOrigin(request, env);
  let result;
  switch (path) {
    case "/api/pair/start":
      result = board.startPairing({
        deviceId: body.device_id,
        deviceToken: body.device_token,
        displayName: body.display_name,
        lang: body.lang,
        origin,
        pairingCode: body.pairing_code,
        expiresUnix: body.expires_unix,
      });
      break;
    case "/api/pair/claim":
      result = board.claim(body.pairing_code);
      break;
    case "/api/heartbeat":
      result = board.heartbeat({
        deviceId: body.device_id,
        deviceToken: body.device_token,
        displayName: body.display_name,
        status: body.status,
        ackCommandIds: body.ack_command_ids,
        lang: body.lang,
        origin,
        offline: body.offline,
      });
      break;
    case "/api/list":
      result = board.list(body.devices);
      break;
    case "/api/command":
      result = board.command({
        deviceId: body.device_id,
        deviceToken: body.device_token,
        cmd: body.cmd,
        duration: body.duration,
      });
      break;
    default:
      return jsonResponse({ ok: false, error: "not_found" }, 404);
  }
  const { status, ...payload } = result;
  return jsonResponse(payload, status);
}
