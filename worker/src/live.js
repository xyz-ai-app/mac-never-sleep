import { COMMAND_TTL_SECS, HEARTBEAT_TTL_SECS, formatPairingCode, deviceCredentialsAreValid, jsonResponse, pairingUrl, tokensMatch } from "./board.js";

export function socketIdentity(request) {
  const url = new URL(request.url);
  const protocols = (request.headers.get("Sec-WebSocket-Protocol") || "").split(",").map(s => s.trim());
  const token = protocols.find(s => s.startsWith("auth."))?.slice(5);
  const id = url.searchParams.get("device_id");
  const role = url.searchParams.get("role");
  if (!protocols.includes("never-sleep-v1") || !deviceCredentialsAreValid(id, token) || !["mac", "viewer"].includes(role)) return null;
  return { id, token, role };
}

// All calls are serialized by BoardHub. No timers or outgoing sockets: idle
// connections remain eligible for hibernation, including during automatic pings.
export class LiveChannel {
  constructor(ctx, board, persist) {
    this.ctx = ctx;
    this.board = board;
    this.persist = persist;
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  sockets() { return this.ctx.getWebSockets().filter(ws => ws.readyState === 1); }

  authorized(a) {
    return a && tokensMatch(a.token, this.board().devices.get(a.id)?.token);
  }

  lastSeen(id) {
    let seen = null;
    for (const ws of this.sockets()) {
      const a = ws.deserializeAttachment();
      if (a?.role !== "mac" || a.id !== id || !a.ready || !this.authorized(a)) continue;
      const ping = this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() / 1000;
      const at = Math.max(a.seen, Number.isFinite(ping) ? ping : 0);
      if (Date.now() / 1000 - at <= HEARTBEAT_TTL_SECS) seen = Math.max(seen || 0, at);
    }
    return seen == null ? null : Math.floor(seen);
  }

  connect(request) {
    const identity = socketIdentity(request);
    const board = this.board();
    const device = identity && board.devices.get(identity.id);
    if (!device || !tokensMatch(device.token, identity.token)) return jsonResponse({ ok: false, error: "unauthorized" }, 401);
    // Reclaim half-open connections before enforcing the per-device cap.
    for (const ws of this.sockets()) {
      const a = ws.deserializeAttachment();
      const ping = this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() / 1000;
      const seen = Math.max(a.seen, Number.isFinite(ping) ? ping : 0);
      if (Date.now() / 1000 - seen > 35) {
        ws.serializeAttachment({ ...a, ready: false });
        ws.close(1001, "stale connection");
      }
    }
    if (this.sockets().filter(ws => ws.deserializeAttachment()?.role === identity.role).length >= (identity.role === "mac" ? 2 : 8)) {
      return jsonResponse({ ok: false, error: "rate_limited" }, 429);
    }
    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server);
    server.serializeAttachment({ id: identity.id, token: identity.token, role: identity.role, seen: Math.floor(Date.now() / 1000), ready: false, origin: request.headers.get("x-public-origin"), lang: "en" });
    return new Response(null, { status: 101, webSocket: client, headers: { "Sec-WebSocket-Protocol": "never-sleep-v1" } });
  }

  send(ws, value) {
    try { ws.send(JSON.stringify(value)); } catch { ws.close(1011, "send failed"); }
  }

  outcome(a) {
    const board = this.board();
    const device = board.devices.get(a.id);
    const now = board.nowSecs();
    const offer = [...board.codes].find(([, v]) => v.deviceId === a.id && v.expires > now);
    return {
      ok: true,
      commands: (device?.commands || []).filter(c => now - c.queued_at <= COMMAND_TTL_SECS).map(({ queued_at, ...command }) => command),
      pairing_code: offer ? formatPairingCode(offer[0]) : null,
      pairing_url: offer ? pairingUrl(offer[0], a.lang === "zh", a.origin) : null,
      expires_unix: offer?.[1].expires || null,
    };
  }

  view(ws, a) {
    const board = this.board();
    if (!this.authorized(a)) { ws.close(1008, "unauthorized"); return; }
    this.send(ws, { type: "view", ...board.list([{ device_id: a.id, device_token: a.token }]), observed_unix: board.nowSecs() });
  }

  publish() {
    for (const ws of this.sockets()) {
      const a = ws.deserializeAttachment();
      if (!this.authorized(a)) { ws.close(1008, "unauthorized"); continue; }
      if (a.role === "viewer") this.view(ws, a);
      else if (a.ready) this.send(ws, this.outcome(a));
    }
  }

  invalidateMacs(except = null) {
    for (const ws of this.sockets()) {
      if (ws === except) continue;
      const a = ws.deserializeAttachment();
      if (a.role !== "mac") continue;
      ws.serializeAttachment({ ...a, ready: false });
      ws.close(1000, "session replaced");
    }
  }

  async message(ws, raw) {
    if (typeof raw !== "string" || raw.length > 16384) { ws.close(1009, "message too large"); return; }
    let body;
    try { body = JSON.parse(raw); } catch { ws.close(1008, "invalid message"); return; }
    if (!body || typeof body !== "object" || Array.isArray(body)) { ws.close(1008, "invalid message"); return; }
    const a = ws.deserializeAttachment();
    if (!a || ws.readyState !== 1) return;
    if (!this.authorized(a)) { ws.close(1008, "unauthorized"); return; }
    const now = Math.floor(Date.now() / 1000);
    // Message limits live in the connection attachment, not a storage row.
    const count = now - (a.window || 0) >= 60 ? 1 : (a.count || 0) + 1;
    if (count > 120) { ws.close(1008, "rate limited"); return; }
    const attachment = { ...a, count, window: count === 1 ? now : a.window };
    ws.serializeAttachment(attachment);
    if (a.role === "viewer" && body.type === "presence") {
      ws.serializeAttachment({ ...attachment, seen: now });
      this.view(ws, a);
      return;
    }
    if (a.role !== "mac" || body.type !== "status") { ws.close(1008, "invalid operation"); return; }
    const board = this.board();
    const result = board.heartbeat({ deviceId: a.id, deviceToken: a.token, displayName: body.display_name, status: body.status, ackCommandIds: body.ack_command_ids, lang: body.lang, origin: a.origin, offline: body.offline });
    await this.persist();
    if (!result.ok) { this.send(ws, result); return; }
    // Publish only after storage commits. Old connections cannot mark a new
    // owner's session offline or acknowledge its commands after replacement.
    this.invalidateMacs(ws);
    ws.serializeAttachment({ ...attachment, ready: body.offline !== true, seen: now, lang: body.lang || "en" });
    this.send(ws, result);
    for (const viewer of this.sockets()) {
      const v = viewer.deserializeAttachment();
      if (v.role === "viewer") this.view(viewer, v);
    }
  }

  async close(ws) {
    const a = ws.deserializeAttachment();
    if (!a?.ready || a.role !== "mac" || !this.authorized(a)) return;
    // Abrupt disconnects keep a short grace period for a network reconnect or
    // foreground-to-menu handoff. Explicit quit is the offline status message.
    ws.serializeAttachment({ ...a, ready: false });
    const board = this.board();
    const device = board.devices.get(a.id);
    const ping = this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() / 1000;
    if (device) device.lastSeen = Math.floor(Math.max(device.lastSeen || 0, a.seen, Number.isFinite(ping) ? ping : 0));
    await this.persist();
    this.publish();
  }
}
