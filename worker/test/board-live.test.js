import assert from "node:assert/strict";
import test from "node:test";
import * as module from "../../site/assets/board-live.js";
test("visible boards subscribe once, hidden boards disconnect, failed sockets back off", () => {
  assert.equal(typeof module.BoardConnections, "function", "board subscriptions are required");
  let now = 0;
  const opened = [];
  class FakeSocket {
    constructor(url, protocols) { this.url = url; this.protocols = protocols; opened.push(this); }
    send(value) { this.sent = value; }
    close() { this.onclose?.(); }
  }
  const updates = [];
  const live = new module.BoardConnections("https://test/api", d => updates.push(d), FakeSocket, () => now);
  const devices = [{ device_id: "a".repeat(32), device_token: "b".repeat(64) }];
  live.update(devices, true);
  assert.equal(opened.length, 1);
  assert.ok(!opened[0].url.includes(devices[0].device_token));
  opened[0].onopen();
  opened[0].onmessage({ data: JSON.stringify({ type: "view", devices: [{ device_id: devices[0].device_id, online: true }] }) });
  assert.equal(live.missing(devices).length, 0);
  now = 1000; live.update(devices, true);
  assert.equal(opened.length, 1);
  opened[0].onclose();
  live.update(devices, true);
  assert.equal(opened.length, 1, "do not reconnect in a tight loop");
  now = 31000; live.update(devices, true);
  assert.equal(opened.length, 2);
  live.update(devices, false);
  assert.equal(live.connections.size, 0);
  assert.equal(updates.length, 1);
});

test("a per-device push preserves other online devices", () => {
  assert.equal(typeof module.mergeViews, "function");
  const merged = module.mergeViews([{ device_id: "a", online: true }, { device_id: "b", online: true }], [{ device_id: "a", online: true, active: true }]);
  assert.equal(merged.find(d => d.device_id === "b").online, true);
});
