import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
let runtime;
try { runtime = await import("miniflare"); } catch (error) {
  if (process.env.REQUIRE_WORKER_RUNTIME === "1") throw error;
  // Rust-only test runs do not require npm dependencies.
}
const id = { device_id: "a".repeat(32), device_token: "b".repeat(64) };
function nextMessage(ws, predicate = () => true) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { ws.removeEventListener("message", onMessage); reject(new Error("WebSocket response timed out")); }, 3000);
    function onMessage(event) {
      let data; try { data = JSON.parse(event.data); } catch { data = event.data; }
      if (!predicate(data)) return;
      clearTimeout(timer); ws.removeEventListener("message", onMessage); resolve(data);
    }
    ws.addEventListener("message", onMessage);
  });
}

test("hibernating sockets push commands, ack them, and publish status to viewers", { skip: !runtime }, async () => {
  const modules = fs.readdirSync("worker/src").filter(n => n.endsWith(".js")).sort((a,b) => (a === "index.js" ? -1 : b === "index.js" ? 1 : 0)).map(n => ({ type: "ESModule", path: `worker/src/${n}`, contents: fs.readFileSync(`worker/src/${n}`, "utf8") }));
  const options = { name: "live-test", modules, compatibilityDate: "2026-09-01", durableObjects: { BOARD: { className: "BoardHub", useSQLite: true } } };
  const mf = new runtime.Miniflare(runtime.convertV4MiniflareOptions ? runtime.convertV4MiniflareOptions(options) : options);
  const post = (path, body) => mf.dispatchFetch(`http://test/api/${path}`, { method: "POST", body: JSON.stringify(body) });
  const connect = role => mf.dispatchFetch(`http://test/api/socket?device_id=${id.device_id}&role=${role}`, { headers: { Upgrade: "websocket", "Sec-WebSocket-Protocol": `never-sleep-v1, auth.${id.device_token}` } });
  const sockets = [];
  try {
    assert.equal((await post("pair/start", id)).status, 200);
    const macRes = await connect("mac");
    assert.equal(macRes.status, 101);
    const mac = macRes.webSocket; sockets.push(mac); mac.accept();
    const initial = nextMessage(mac, d => d.ok === true);
    mac.send(JSON.stringify({ type: "status", ...id, status: { active: false } }));
    assert.deepEqual((await initial).commands, []);
    const viewRes = await connect("viewer");
    assert.equal(viewRes.status, 101);
    const viewer = viewRes.webSocket; sockets.push(viewer); viewer.accept();
    const view = nextMessage(viewer, d => d.type === "view");
    viewer.send(JSON.stringify({ type: "presence" }));
    assert.equal((await view).devices[0].online, true);
    await mf.unsafeEvictDurableObject("live-test", "BoardHub", { name: `device:${id.device_id}`, webSockets: "hibernate" });
    const pushed = nextMessage(mac, d => d.commands?.length);
    const accepted = await (await post("command", { ...id, cmd: "on" })).json();
    assert.equal((await pushed).commands[0].id, accepted.command_id);
    const acked = nextMessage(mac, d => d.commands?.length === 0);
    const updated = nextMessage(viewer, d => d.devices?.[0]?.active === true);
    mac.send(JSON.stringify({ type: "status", ...id, status: { active: true }, ack_command_ids: [accepted.command_id] }));
    await acked; await updated;
    const pong = nextMessage(mac, d => d === "pong");
    mac.send("ping"); assert.equal(await pong, "pong");
    const off = nextMessage(viewer, d => d.devices?.[0]?.online === false);
    mac.send(JSON.stringify({ type: "status", ...id, offline: true, status: { active: false } }));
    await off;
  } finally {
    for (const ws of sockets) ws.close();
    await mf.dispose();
  }
});
