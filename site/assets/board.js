(() => {
  const STORAGE_KEY = "never-sleep-devices";
  const LIST_MAX_DEVICES = 32;
  const MAX_DISPLAY_NAME_CHARS = 128;
  const zh = document.documentElement.lang.startsWith("zh");

  const copy = zh
    ? {
        add: "添加 Mac",
        start: "开始关屏待命",
        end: "结束待命",
        online: "在线",
        offline: "离线",
        lastSeen: "上次见到",
        standbyOn: "待命中",
        standbyOff: "未待命",
        displayAsleep: "屏幕已关",
        displayAwake: "屏幕亮着",
        lidOpen: "开盖",
        lidClosed: "合盖",
        ac: "电源适配器",
        battery: "电池",
        remaining: "剩余",
        forget: "从本机列表移除",
        badCode: "配对码无效或已过期。",
        retryLater: "尝试过于频繁，请稍后再试。",
        offlineCmd: "这台 Mac 当前离线，指令没有生效。",
        badCmd: "无法发送指令。",
        networkError: "网络出错，请检查连接后重试。",
        storageError: "此浏览器无法保存配对。请允许网站数据后重试。",
        starting: "正在开始…",
        ending: "正在结束…",
      }
    : {
        add: "Add Mac",
        start: "Start Screen-Off Standby",
        end: "End Standby",
        online: "Online",
        offline: "Offline",
        lastSeen: "Last seen",
        standbyOn: "Standby on",
        standbyOff: "Standby off",
        displayAsleep: "Display asleep",
        displayAwake: "Display awake",
        lidOpen: "Lid open",
        lidClosed: "Lid closed",
        ac: "Power adapter",
        battery: "Battery",
        remaining: "left",
        forget: "Remove from this phone",
        badCode: "That pairing code is invalid or expired.",
        retryLater: "Too many attempts. Please try again in a moment.",
        offlineCmd: "This Mac is offline; the command did not apply.",
        badCmd: "Could not send the command.",
        networkError: "Network error. Check the connection and try again.",
        storageError: "This browser cannot save pairing. Allow site data and try again.",
        starting: "Starting…",
        ending: "Ending…",
      };

  function apiBase() {
    const path = location.pathname;
    if (path.startsWith("/never-sleep/") || path.includes("/never-sleep/")) {
      return "/never-sleep/api";
    }
    return "/api";
  }

  function loadDevices() {
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      const parsed = raw ? JSON.parse(raw) : [];
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  }

  function saveDevices(devices) {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(devices));
  }

  function withDevices(devices, entry) {
    const next = (devices || []).filter((d) => d.device_id !== entry.device_id);
    next.push(entry);
    while (next.length > LIST_MAX_DEVICES) next.shift();
    return next;
  }

  function boundDisplayName(name) {
    if (typeof name !== "string") return "Mac";
    const trimmed = name.trim();
    if (!trimmed) return "Mac";
    return [...trimmed].slice(0, MAX_DISPLAY_NAME_CHARS).join("");
  }

  function claimReservation() {
    return {
      device_id: "f".repeat(64),
      device_token: "f".repeat(128),
      display_name: "\u0000".repeat(MAX_DISPLAY_NAME_CHARS),
    };
  }

  function withStorageLock(locks, key, fn) {
    const lock = locks || globalThis.navigator?.locks;
    if (lock && typeof lock.request === "function") {
      return lock.request(key, fn);
    }
    return Promise.resolve(fn());
  }

  function storageCanHold(storage, key, value) {
    const raw = typeof value === "string" ? value : JSON.stringify(value);
    try {
      const prev = storage.getItem(key);
      try {
        storage.setItem(key, raw);
        const ok = storage.getItem(key) === raw;
        if (prev === null) {
          storage.removeItem(key);
        } else {
          storage.setItem(key, prev);
        }
        return ok;
      } catch {
        try {
          if (prev === null) {
            storage.removeItem(key);
          } else {
            storage.setItem(key, prev);
          }
        } catch {
          // quota restore is best-effort
        }
        return false;
      }
    } catch {
      return false;
    }
  }

  function commitClaimedDevice(storage, key, entry, locks) {
    return withStorageLock(locks, key, () => {
      const next = withDevices(parseStoredDevices(storage, key), entry);
      storage.setItem(key, JSON.stringify(next));
      return next;
    });
  }

  function parseStoredDevices(storage, key) {
    try {
      const raw = storage.getItem(key);
      const parsed = raw ? JSON.parse(raw) : [];
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  }

  function forgetDevice(storage, key, deviceId, locks) {
    return withStorageLock(locks, key, () => {
      const next = parseStoredDevices(storage, key).filter(
        (d) => d.device_id !== deviceId,
      );
      storage.setItem(key, JSON.stringify(next));
      return next;
    });
  }

  function runExclusiveClaim(storage, key, locks, acquire) {
    return withStorageLock(locks, key, async () => {
      const devices = parseStoredDevices(storage, key);
      if (!storageCanHold(storage, key, withDevices(devices, claimReservation()))) {
        return { stored: false };
      }
      const acquired = await acquire();
      if (!acquired || acquired.ok === false) {
        return acquired || { ok: false };
      }
      const next = withDevices(parseStoredDevices(storage, key), acquired.entry);
      storage.setItem(key, JSON.stringify(next));
      return { ok: true, devices: next };
    });
  }

  function canStoreClaim(storage, key, devices, entry, locks) {
    return withStorageLock(locks, key, () =>
      storageCanHold(storage, key, withDevices(devices, entry)),
    );
  }

  async function post(path, body) {
    const res = await fetch(`${apiBase()}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    const json = await res.json().catch(() => ({}));
    return { res, json };
  }

  function formatLastSeen(unix) {
    if (!unix) return "—";
    const delta = Math.max(0, Math.floor(Date.now() / 1000) - unix);
    if (delta < 5) return zh ? "刚刚" : "just now";
    if (delta < 60) return zh ? `${delta} 秒前` : `${delta}s ago`;
    const min = Math.floor(delta / 60);
    if (min < 60) return zh ? `${min} 分钟前` : `${min}m ago`;
    const hr = Math.floor(min / 60);
    return zh ? `${hr} 小时前` : `${hr}h ago`;
  }

  function formatRemaining(secs) {
    if (secs == null) return "—";
    const s = Number(secs);
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    if (h > 0) {
      return `${h}:${String(m).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
    }
    return `${m}:${String(s % 60).padStart(2, "0")}`;
  }

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  }

  function withListFailure(statuses) {
    return (statuses || []).map((st) => Object.assign({}, st, { online: false }));
  }

  function mergeListStatuses(previous, incoming) {
    const seen = new Set();
    const byId = new Map();
    for (const st of previous || []) {
      if (st && st.device_id) byId.set(st.device_id, st);
    }
    for (const st of incoming || []) {
      if (st && st.device_id) {
        seen.add(st.device_id);
        byId.set(st.device_id, st);
      }
    }
    return Array.from(byId.values()).map((st) =>
      seen.has(st.device_id) ? st : Object.assign({}, st, { online: false }),
    );
  }

  function devicePendingCmd(pendingByDevice, deviceId) {
    if (!pendingByDevice || deviceId == null) return null;
    const cmd = pendingByDevice[deviceId];
    return cmd === "on" || cmd === "off" ? cmd : null;
  }

  function withDevicePending(pendingByDevice, deviceId, cmd) {
    const next = Object.assign({}, pendingByDevice);
    next[deviceId] = cmd;
    return next;
  }

  function withoutDevicePending(pendingByDevice, deviceId) {
    const next = Object.assign({}, pendingByDevice);
    delete next[deviceId];
    return next;
  }

  function nextRefreshGeneration(current) {
    return (Number(current) || 0) + 1;
  }

  function isCurrentRefresh(startedGeneration, latestGeneration) {
    return startedGeneration === latestGeneration;
  }

  function hrefWithPairingCode(href, code) {
    const path = String(href || "").split("?")[0];
    if (!code) return path;
    return `${path}?code=${encodeURIComponent(code)}`;
  }

  function languageHrefAfterClaim(href, code, claimSaved) {
    const path = String(href || "").split("?")[0];
    return claimSaved ? path : hrefWithPairingCode(path, code);
  }

  async function waitForClaimThenLanguageHref(href, code, claimPromise) {
    let saved = false;
    if (claimPromise) {
      try {
        saved = (await claimPromise) === true;
      } catch {
        saved = false;
      }
    }
    return languageHrefAfterClaim(href, code, saved);
  }

  function syncLanguageLinks(code) {
    document.querySelectorAll(".language-row a[hreflang]").forEach((link) => {
      link.setAttribute("href", hrefWithPairingCode(link.getAttribute("href"), code));
    });
  }

  function clearPairingQuery() {
    if (!new URLSearchParams(location.search).get("code")) return;
    history.replaceState({}, "", location.pathname + location.hash);
    syncLanguageLinks("");
  }

  function trustedView(stored, incoming, previous) {
    const src = incoming || {};
    const prev = previous || {};
    const from = incoming ? src : prev;
    return {
      device_id: stored.device_id,
      display_name:
        (typeof from.display_name === "string" && from.display_name) ||
        stored.display_name ||
        "Mac",
      online: incoming ? src.online === true : false,
      last_seen_unix:
        typeof from.last_seen_unix === "number" ? from.last_seen_unix : null,
      active: from.active === true,
      display: from.display === "asleep" ? "asleep" : "awake",
      lid: from.lid === "closed" ? "closed" : "open",
      on_ac: from.on_ac === true,
      battery: Number.isFinite(from.battery) ? from.battery : null,
      remaining_secs: Number.isFinite(from.remaining_secs)
        ? from.remaining_secs
        : null,
    };
  }

  function render(devices, statuses, pending) {
    const list = document.getElementById("device-list");
    const empty = document.getElementById("board-empty");
    list.replaceChildren();
    empty.hidden = devices.length > 0;
    const byId = new Map((statuses || []).map((d) => [d.device_id, d]));
    const prevById = new Map((lastStatuses || []).map((d) => [d.device_id, d]));
    for (const stored of devices) {
      const st = trustedView(
        stored,
        byId.get(stored.device_id),
        prevById.get(stored.device_id),
      );
      const card = el("article", "device-card" + (st.active ? " active" : "") + (st.online ? "" : " offline"));
      const power = st.on_ac
        ? copy.ac
        : `${copy.battery}${st.battery != null ? ` ${st.battery}%` : ""}`;
      const remain =
        st.active && st.remaining_secs != null
          ? `${formatRemaining(st.remaining_secs)} ${copy.remaining}`
          : "—";
      const pendingCmd = devicePendingCmd(pending, stored.device_id);
      const busy = pendingCmd != null;

      const head = el("div", "device-head");
      head.appendChild(el("div", "device-name", st.display_name));
      head.appendChild(
        el(
          "div",
          "device-online" + (st.online ? "" : " off"),
          st.online ? copy.online : copy.offline,
        ),
      );
      card.appendChild(head);
      const coin = el("img", "device-coin");
      const assets = document.querySelector('link[rel="icon"]').href.replace(/favicon\.png$/, "");
      coin.src = assets + (st.active ? "moon.png" : "sun.png");
      coin.alt = "";
      coin.width = 104;
      coin.height = 104;
      card.appendChild(coin);
      card.appendChild(el("h2", "device-status", st.active ? copy.standbyOn : copy.standbyOff));


      const meta = el("dl", "device-meta");
      const standby = el("div");
      standby.appendChild(
        el("strong", "", st.active ? copy.standbyOn : copy.standbyOff),
      );
      meta.appendChild(standby);
      meta.appendChild(
        el(
          "div",
          "",
          st.display === "asleep" ? copy.displayAsleep : copy.displayAwake,
        ),
      );
      meta.appendChild(
        el("div", "", st.lid === "closed" ? copy.lidClosed : copy.lidOpen),
      );
      meta.appendChild(el("div", "", power));
      meta.appendChild(el("div", "", remain));
      meta.appendChild(
        el("div", "", `${copy.lastSeen} ${formatLastSeen(st.last_seen_unix)}`),
      );
      card.appendChild(meta);

      const actions = el("div", "device-actions");
      const startBtn = el("button", "btn btn-primary", copy.start);
      startBtn.type = "button";
      startBtn.setAttribute("data-cmd", "on");
      const endBtn = el("button", "btn btn-danger", copy.end);
      endBtn.type = "button";
      endBtn.setAttribute("data-cmd", "off");
      if (!st.online || busy) {
        startBtn.disabled = true;
        endBtn.disabled = true;
      }
      if (busy && pendingCmd === "on") startBtn.textContent = copy.starting;
      if (busy && pendingCmd === "off") endBtn.textContent = copy.ending;
      startBtn.addEventListener("click", () => sendCommand(stored, "on"));
      endBtn.addEventListener("click", () => sendCommand(stored, "off"));
      startBtn.hidden = st.active;
      endBtn.hidden = !st.active;
      actions.appendChild(startBtn);
      actions.appendChild(endBtn);
      card.appendChild(actions);

      const forget = el("button", "forget", copy.forget);
      forget.type = "button";
      forget.addEventListener("click", () => {
        void forgetDevice(localStorage, STORAGE_KEY, stored.device_id).then(() =>
          refresh(),
        );
      });
      card.appendChild(forget);
      list.appendChild(card);
    }
  }

  let pendingByDevice = {};
  let lastStatuses = [];
  let refreshGen = 0;
  let refreshInFlight = null;
  let refreshQueued = false;

  function showError(message) {
    const elErr = document.getElementById("board-error");
    if (!message) {
      elErr.hidden = true;
      elErr.textContent = "";
      return;
    }
    elErr.hidden = false;
    elErr.textContent = message;
  }

  async function runRefresh() {
    const devices = loadDevices();
    if (!devices.length) {
      refreshGen = nextRefreshGeneration(refreshGen);
      render([], lastStatuses, pendingByDevice);
      return;
    }
    const started = (refreshGen = nextRefreshGeneration(refreshGen));
    try {
      const { res, json } = await post("/list", { devices });
      if (!isCurrentRefresh(started, refreshGen)) return;
      if (!res.ok || !Array.isArray(json.devices)) {
        render(devices, withListFailure(lastStatuses), pendingByDevice);
        return;
      }
      lastStatuses = mergeListStatuses(lastStatuses, json.devices);
      render(devices, lastStatuses, pendingByDevice);
    } catch {
      if (!isCurrentRefresh(started, refreshGen)) return;
      render(devices, withListFailure(lastStatuses), pendingByDevice);
    }
  }

  async function refresh() {
    if (refreshInFlight) {
      refreshQueued = true;
      return refreshInFlight;
    }
    refreshInFlight = runRefresh();
    try {
      await refreshInFlight;
    } finally {
      refreshInFlight = null;
      if (refreshQueued) {
        refreshQueued = false;
        void refresh();
      }
    }
  }

  async function sendCommand(device, cmd) {
    if (devicePendingCmd(pendingByDevice, device.device_id)) return;
    showError("");
    pendingByDevice = withDevicePending(pendingByDevice, device.device_id, cmd);
    render(loadDevices(), lastStatuses, pendingByDevice);
    try {
      const { res, json } = await post("/command", {
        device_id: device.device_id,
        device_token: device.device_token,
        cmd,
      });
      if (res.status === 409 || json.error === "offline") {
        showError(copy.offlineCmd);
      } else if (!res.ok || json.ok === false) {
        showError(copy.badCmd);
      }
    } catch {
      showError(copy.networkError);
    }
    pendingByDevice = withoutDevicePending(pendingByDevice, device.device_id);
    await refresh();
  }

  function claimFailureMessage(res, json, copy) {
    const status = Number(res?.status) || 0;
    const error = json?.error;
    if (status === 429 || error === "rate_limited" || status >= 500) {
      return copy.retryLater;
    }
    return copy.badCode;
  }

  async function claim(code) {
    showError("");
    const input = document.getElementById("pair-code");
    try {
      const result = await runExclusiveClaim(
        localStorage,
        STORAGE_KEY,
        globalThis.navigator?.locks,
        async () => {
          const { res, json } = await post("/pair/claim", { pairing_code: code });
          if (!res.ok || !json.ok) {
            return { ok: false, res, json };
          }
          return {
            ok: true,
            entry: {
              device_id: json.device_id,
              device_token: json.device_token,
              display_name: boundDisplayName(json.display_name),
            },
          };
        },
      );
      if (result && result.stored === false) {
        showError(copy.storageError);
        return false;
      }
      if (!result || result.ok === false) {
        showError(claimFailureMessage(result.res, result.json, copy));
        return false;
      }
      input.value = "";
      clearPairingQuery();
      await refresh();
      return true;
    } catch {
      showError(copy.networkError);
      return false;
    }
  }

  let claimPromise = null;
  let claimInFlight = false;

  function beginClaim(code) {
    if (claimInFlight) return claimPromise;
    claimInFlight = true;
    const submit = document.querySelector('#pair-form button');
    submit.disabled = true;
    submit.textContent = zh ? "正在配对…" : "Pairing…";
    claimPromise = claim(code);
    const pending = claimPromise;
    pending.finally(() => {
      if (claimPromise === pending) {
        claimInFlight = false;
        submit.disabled = false;
        submit.textContent = copy.add;
      }
    });
    return claimPromise;
  }

  document.querySelectorAll(".language-row a[hreflang]").forEach((link) => {
    link.addEventListener("click", (event) => {
      if (!claimPromise) return;
      event.preventDefault();
      const href = link.getAttribute("href") || "";
      const code = new URLSearchParams(location.search).get("code") || "";
      waitForClaimThenLanguageHref(href, code, claimPromise).then((next) => {
        location.assign(next);
      });
    });
  });

  document.getElementById("pair-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const code = document.getElementById("pair-code").value;
    if (code.trim()) beginClaim(code);
  });

  const params = new URLSearchParams(location.search);
  const initial = params.get("code");
  if (initial) {
    syncLanguageLinks(initial);
    document.getElementById("pair-code").value = initial;
    beginClaim(initial);
  } else {
    refresh();
  }
  window.setInterval(refresh, 2500);
})();
