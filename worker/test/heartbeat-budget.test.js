import assert from "node:assert/strict";
import test from "node:test";
import worker from "../src/index.js";
import { Board, handleApi, HEARTBEAT_TTL_SECS, COMMAND_TTL_SECS } from "../src/board.js";

const id = { device_id: "a".repeat(32), device_token: "b".repeat(64) };
const request = (body = id, path = "/api/heartbeat") => new Request(`https://test${path}`, {
  method: "POST", headers: { "Content-Type": "application/json", "CF-Connecting-IP": "192.0.2.1" }, body: JSON.stringify(body),
});
function environment(allowed = true) {
  const calls = [];
  const limits = [];
  return { calls, limits, env: {
    DEVICE_IP_RATE: { async limit({ key }) { limits.push(key); return { success: allowed }; } },
    DEVICE_EDGE_RATE: { async limit({ key }) { limits.push(key); return { success: allowed }; } },
    BOARD: { idFromName(name) { calls.push(name); return name; }, get() { return { async fetch() { return Response.json({ ok: true, expired_codes: [] }); } }; } },
  } };
}
test("ten-second heartbeats tolerate two missed beats without extending command lifetime", () => {
  assert.equal(HEARTBEAT_TTL_SECS, 35);
  assert.equal(COMMAND_TTL_SECS, 60);
});
test("normal heartbeat and command use just one device DO with edge limiting", async () => {
  for (const path of ["/api/heartbeat", "/api/command"]) {
    const { env, calls, limits } = environment();
    assert.equal((await worker.fetch(request(id, path), env)).status, 200);
    assert.deepEqual(calls, [`device:${id.device_id}`]);
    assert.equal(limits.length, 2);
  }
});
test("edge rejection and malformed credentials cannot allocate a DO", async () => {
  for (const [allowed, body, status] of [[false, id, 429], [true, {}, 400]]) {
    const { env, calls } = environment(allowed);
    assert.equal((await worker.fetch(request(body), env)).status, status);
    assert.deepEqual(calls, []);
  }
});
test("missing edge bindings retain the legacy rate gate", async () => {
  const { env, calls } = environment();
  delete env.DEVICE_IP_RATE;
  assert.equal((await worker.fetch(request(), env)).status, 200);
  assert.deepEqual(calls, ["rate:device", `device:${id.device_id}`]);
});
test("authenticated device rate survives reload and wrong tokens cannot consume it", async () => {
  let now = 1000;
  let board = new Board(() => now);
  await handleApi(board, request(id, "/api/pair/start"));
  for (let i = 0; i < 130; i++) {
    assert.equal((await handleApi(board, request({ ...id, device_token: "c".repeat(64) }))).status, 401);
  }
  for (let i = 0; i < 120; i++) {
    assert.equal((await handleApi(board, request())).status, 200);
  }
  board = Board.fromJSON(board.toJSON());
  board.nowSecs = () => now;
  assert.equal((await handleApi(board, request())).status, 429);
  assert.equal((await handleApi(board, request({ ...id, cmd: "on" }, "/api/command"))).status, 429);
  // Quit must still clear online state even when the device has exhausted its budget.
  assert.equal((await handleApi(board, request({ ...id, offline: true }))).status, 200);
  now += 60;
  assert.equal((await handleApi(board, request())).status, 200);
});


test("aggregate edge rejection prevents shard allocation after IP acceptance", async () => {
  const { env, calls } = environment();
  env.DEVICE_EDGE_RATE.limit = async () => ({ success: false });
  assert.equal((await worker.fetch(request(), env)).status, 429);
  assert.deepEqual(calls, []);
});

test("unknown API paths cannot allocate shards or spend rate tokens", async () => {
  const { env, calls, limits } = environment();
  assert.equal((await worker.fetch(request(id, "/api/unknown"), env)).status, 404);
  assert.deepEqual(calls, []);
  assert.deepEqual(limits, []);
});

test("command remains deliverable after two missed beats and is acknowledged", async () => {
  let now = 1000;
  const board = new Board(() => now);
  await handleApi(board, request(id, "/api/pair/start"));
  await handleApi(board, request());
  now += 29;
  const accepted = await handleApi(board, request({ ...id, cmd: "on" }, "/api/command"));
  assert.equal(accepted.status, 200);
  const { command_id } = await accepted.json();
  now += 1;
  const beat = await (await handleApi(board, request())).json();
  assert.equal(beat.commands[0].id, command_id);
  now += 10;
  const ack = await (await handleApi(board, request({ ...id, ack_command_ids: [command_id] }))).json();
  assert.deepEqual(ack.commands, []);
});
