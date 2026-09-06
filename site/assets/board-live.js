// One hibernating subscription per visible device. HTTP commands retain their
// existing contract; healthy subscriptions replace all recurring list requests.
export class BoardConnections {
  constructor(api, onView, Socket = globalThis.WebSocket, now = () => Date.now()) {
    this.api = api;
    this.onView = onView;
    this.Socket = Socket;
    this.now = now;
    this.connections = new Map();
  }

  update(devices, visible) {
    const wanted = new Map((visible ? devices.slice(0, 32) : []).map(d => [d.device_id, d]));
    for (const [id, entry] of this.connections) {
      if (!wanted.has(id) || wanted.get(id).device_token !== entry.token) {
        this.connections.delete(id);
        const ws = entry.ws; entry.ws = null;
        ws?.close();
      }
    }
    for (const [id, device] of wanted) {
      if (!/^[a-f0-9]{32}$/i.test(id) || !/^[a-f0-9]{64}$/i.test(device.device_token)) continue;
      let entry = this.connections.get(id);
      if (!entry) {
        entry = { token: device.device_token, ws: null, ready: false, retry: 0, failures: 0, seen: this.now(), presence: 0 };
        this.connections.set(id, entry);
      }
      if (entry.ws && this.now() - entry.seen > 35000) {
        const ws = entry.ws;
        this.failed(entry);
        ws.close();
      }
      if (!entry.ws && this.now() >= entry.retry && this.Socket) this.connect(id, entry);
      if (entry.ready && this.now() - entry.presence >= 20000) this.presence(entry);
    }
  }

  missing(devices) { return devices.filter(d => !this.connections.get(d.device_id)?.ready); }

  failed(entry) {
    entry.ws = null;
    entry.ready = false;
    entry.failures += 1;
    entry.retry = this.now() + Math.min(300000, 30000 * 2 ** Math.min(entry.failures - 1, 4));
  }

  connect(id, entry) {
    try {
      const url = new URL(`${this.api}/socket`);
      url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
      url.searchParams.set("device_id", id);
      url.searchParams.set("role", "viewer");
      const ws = new this.Socket(url.href, ["never-sleep-v1", `auth.${entry.token}`]);
      entry.ws = ws;
      entry.seen = this.now();
      ws.onopen = () => { if (entry.ws === ws) this.presence(entry); };
      ws.onmessage = event => {
        if (entry.ws !== ws) return;
        let data; try { data = JSON.parse(event.data); } catch { return; }
        if (data.type !== "view" || !Array.isArray(data.devices)) return;
        const view = data.devices.find(d => d.device_id === id);
        if (!view) return;
        entry.ready = true;
        entry.seen = this.now();
        // Reset after a sustained connection, not an open/close loop.
        if (entry.seen - (entry.opened || 0) > 60000) entry.failures = 0;
        this.onView([view]);
      };
      entry.opened = this.now();
      ws.onclose = () => { if (entry.ws === ws) this.failed(entry); };
      ws.onerror = () => { if (entry.ws === ws) { this.failed(entry); ws.close(); } };
    } catch { this.failed(entry); }
  }

  presence(entry) {
    try { entry.ws.send(JSON.stringify({ type: "presence" })); entry.presence = this.now(); }
    catch { const ws = entry.ws; this.failed(entry); ws?.close(); }
  }
}

export function mergeViews(previous, incoming) {
  const byId = new Map((previous || []).map(d => [d.device_id, d]));
  for (const view of incoming || []) if (view?.device_id) byId.set(view.device_id, view);
  return [...byId.values()];
}
