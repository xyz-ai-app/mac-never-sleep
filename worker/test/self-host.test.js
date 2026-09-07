import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";

test("self-host deployment serves its own board and API without the project gateway", () => {
  assert.ok(existsSync(new URL("../../wrangler.self-host.json", import.meta.url)), "a standalone self-host deployment config must be provided");
  const config = JSON.parse(readFileSync(new URL("../../wrangler.self-host.json", import.meta.url), "utf8"));
  assert.notEqual(config.name, "mac-never-sleep");
  assert.equal(config.workers_dev, true);
  assert.equal(config.preview_urls, false);
  assert.equal(config.main, "worker/src/index.js");
  assert.equal(config.assets.directory, "./site");
  assert.deepEqual(config.assets.run_worker_first, ["/api/*"]);
  assert.deepEqual(config.durable_objects.bindings, [{ name: "BOARD", class_name: "BoardHub" }]);
  assert.deepEqual(config.migrations, [{ tag: "v1", new_sqlite_classes: ["BoardHub"] }]);
  assert.deepEqual(config.ratelimits.map(binding => binding.name).sort(), ["DEVICE_EDGE_RATE", "DEVICE_IP_RATE"]);
  assert.equal(config.routes, undefined);
  assert.equal(config.services, undefined);
  assert.equal(config.vars?.PUBLIC_SITE_ORIGIN, undefined);
  assert.equal(config.observability.enabled, false);
});
