# Live transport

`GET /api/socket?device_id=<32 hex>&role=mac|viewer` upgrades to WebSocket.
Offer subprotocols `never-sleep-v1` and `auth.<64 hex device token>`; the server
selects only `never-sleep-v1`. Tokens must not appear in URLs. The public Worker
applies entry limits before routing; the device DO authenticates before accepting.
Browser Origin must match the public site. Attachments retain the authenticated
identity across hibernation and are checked against the current stored credential.

A Mac sends the existing heartbeat body plus `type: "status"`. The server binds
identity to the connection, ignores body credentials, persists status/ack changes,
then sends the existing heartbeat response. HTTP commands are persisted before
being pushed to the current Mac. Command IDs, 60-second expiry, and acknowledgments
remain the source of truth; reconnecting must not blindly replay held commands.
A replacement Mac becomes authoritative only after its first status is committed.
Explicit quit uses the existing offline HTTP flush; a network close keeps the
last verified contact for the 35-second reconnect/handoff grace period.

`ping` is a literal text message (not JSON). Cloudflare's auto-response sends
`pong` without running a handler or writing a storage row. Online checks use
`getWebSocketAutoResponseTimestamp()` plus the last committed status contact.
There are no server-side keepalive timers, outbound sockets, or periodic alarms
for presence. The existing pairing-expiry alarm remains in use.

Viewers send `{"type":"presence"}` on connection and every 20 seconds while
visible. Replies/pushes have `type: "view"`, `devices`, and `observed_unix`.
Presence checks are read-only. Each device DO allows up to eight viewer sockets
and two Mac sockets during replacement. Malformed/oversized frames, invalid
operations, stale credentials, and abusive message rates close the connection.

Mac status changes coalesce at one second, with a five-minute full resync;
clock-only ticks stay local. Mac keepalives run every ten seconds. WebSocket
failures back off from one to five minutes while ten-second HTTP polling remains
available. Viewer retries back off from 30 seconds to five minutes, with
30-second HTTP list fallback only for disconnected devices. Hidden pages close
subscriptions and stop polling.

## Local verification

Run `npm install --no-package-lock`, then `REQUIRE_WORKER_RUNTIME=1 npm test`.
The runtime suite uses Wrangler's Miniflare dependency and explicitly evicts a
DO with `webSockets: "hibernate"` before verifying command delivery and acks.
`cargo test --workspace` includes the dependency-free Worker/browser policy tests
and a local Rust WebSocket server test; neither touches IOKit or a real account.

Deploy Worker/config/site before distributing the Mac build. Preserve the HTTP
routes for old clients. Compare Worker requests, DO requests, DO duration and
storage writes separately: WebSocket message counts are not HTTP request counts,
and reduced message billing alone does not establish total cost savings.
