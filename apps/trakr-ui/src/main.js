// trakr-ui frontend. Talks to the embedded daemon exactly the way the CLI or
// an agent would: plain fetch() and EventSource against the documented HTTP
// + SSE API (docs/api.md). No Tauri IPC is used for app logic on purpose.
const API = "http://127.0.0.1:7011";

const $ = (id) => document.getElementById(id);

const el = {
  daemonDot: $("daemon-dot"),
  daemonLabel: $("daemon-label"),
  deviceList: $("device-list"),
  btnScan: $("btn-scan"),
  btnDisconnect: $("btn-disconnect"),
  sessionEmpty: $("session-empty"),
  sessionActive: $("session-active"),
  statDevice: $("stat-device"),
  statFirmware: $("stat-firmware"),
  statArmed: $("stat-armed"),
  statBattery: $("stat-battery"),
  statRssi: $("stat-rssi"),
  statMode: $("stat-mode"),
  btnArm: $("btn-arm"),
  btnDisarm: $("btn-disarm"),
  selectMode: $("select-mode"),
  selectHand: $("select-hand"),
  shotEmpty: $("shot-empty"),
  shotData: $("shot-data"),
  shotSpeed: $("shot-speed"),
  shotVla: $("shot-vla"),
  shotHla: $("shot-hla"),
  shotSpin: $("shot-spin"),
  shotAxis: $("shot-axis"),
  eventLog: $("event-log"),
  btnClearLog: $("btn-clear-log"),
};

function logEvent(kind, detail) {
  const line = document.createElement("div");
  line.className = "event-line";
  const time = document.createElement("span");
  time.className = "event-time";
  time.textContent = new Date().toLocaleTimeString();
  const kindEl = document.createElement("span");
  kindEl.className = `event-kind kind-${kind}`;
  kindEl.textContent = kind;
  const detailEl = document.createElement("span");
  detailEl.className = "event-detail";
  detailEl.textContent = detail;
  line.append(time, kindEl, detailEl);
  el.eventLog.prepend(line);
  while (el.eventLog.children.length > 300) {
    el.eventLog.removeChild(el.eventLog.lastChild);
  }
}

async function api(path, opts = {}) {
  const res = await fetch(`${API}${path}`, {
    headers: opts.body ? { "content-type": "application/json" } : undefined,
    ...opts,
  });
  let body = null;
  try {
    body = await res.json();
  } catch {
    /* no body */
  }
  return { ok: res.ok, status: res.status, body };
}

// --- Daemon health -----------------------------------------------------

async function pollHealth() {
  try {
    const { ok } = await api("/v1/health");
    setDaemonStatus(ok);
  } catch {
    setDaemonStatus(false);
  }
}

function setDaemonStatus(ok) {
  el.daemonDot.className = `dot ${ok ? "dot-ok" : "dot-error"}`;
  el.daemonLabel.textContent = ok ? "daemon connected" : "daemon unreachable";
}

// --- Devices -------------------------------------------------------------

async function scanDevices() {
  el.btnScan.disabled = true;
  el.btnScan.textContent = "Scanning…";
  el.deviceList.innerHTML = '<li class="empty">Scanning…</li>';
  const broadcast = el.inputBroadcast.value.trim();
  const qs = broadcast ? `?window_ms=3000&broadcast=${encodeURIComponent(broadcast)}` : "?window_ms=3000";
  const { ok, body } = await api(`/v1/devices${qs}`);
  el.btnScan.disabled = false;
  el.btnScan.textContent = "Scan";
  if (!ok || !body?.devices?.length) {
    el.deviceList.innerHTML = '<li class="empty">No devices found. Make sure the unit is powered on.</li>';
    return;
  }
  el.deviceList.innerHTML = "";
  for (const d of body.devices) {
    const li = document.createElement("li");
    const info = document.createElement("div");
    info.innerHTML = `<div class="device-name">${d.name}</div><div class="device-address">${d.address}${d.direct_mode ? " · direct mode" : ""}</div>`;
    const btn = document.createElement("button");
    btn.className = "primary";
    btn.textContent = "Connect";
    btn.onclick = () => connect(d.name, d.address);
    li.append(info, btn);
    el.deviceList.appendChild(li);
  }
}

async function connect(name, address) {
  logEvent("status", `connecting to ${name} (${address})…`);
  await api("/v1/session", { method: "POST", body: JSON.stringify({ name, address }) });
  refreshStatus();
}

async function disconnect() {
  await api("/v1/session", { method: "DELETE" });
  showNoSession();
  logEvent("status", "disconnected");
}

// --- Session / status ------------------------------------------------------

function showNoSession() {
  el.sessionEmpty.classList.remove("hidden");
  el.sessionActive.classList.add("hidden");
  el.btnDisconnect.disabled = true;
}

function renderSession(body) {
  const { device, status } = body;
  el.sessionEmpty.classList.add("hidden");
  el.sessionActive.classList.remove("hidden");
  el.btnDisconnect.disabled = false;

  el.statDevice.textContent = device?.name ?? "—";
  el.statFirmware.textContent = device?.firmware ?? "—";
  el.statArmed.textContent = status?.armed ? "yes" : "no";
  el.statArmed.className = `stat-value armed-${!!status?.armed}`;
  el.statBattery.textContent = status?.battery_pct != null ? `${status.battery_pct}%` : "—";
  el.statRssi.textContent = status?.rssi != null ? `${status.rssi} dBm` : "—";
  el.statMode.textContent = status?.shot_mode ?? "—";
  if (status?.shot_mode) el.selectMode.value = status.shot_mode;
  if (status?.handedness) el.selectHand.value = status.handedness;

  if (body.last_shot) renderShot(body.last_shot);
}

async function refreshStatus() {
  const { ok, body } = await api("/v1/session");
  if (ok) renderSession(body);
  else showNoSession();
}

function renderShot(shot) {
  el.shotEmpty.classList.add("hidden");
  el.shotData.classList.remove("hidden");
  const mph = (shot.ball.speed_mps * 2.236936).toFixed(1);
  el.shotSpeed.textContent = `${mph} mph`;
  el.shotVla.textContent = `${shot.ball.launch_angle_deg.toFixed(1)}°`;
  el.shotHla.textContent = `${shot.ball.horizontal_angle_deg.toFixed(1)}°`;
  el.shotSpin.textContent = shot.ball.total_spin_rpm != null ? `${Math.round(shot.ball.total_spin_rpm)} rpm` : "—";
  el.shotAxis.textContent = shot.ball.spin_axis_deg != null ? `${shot.ball.spin_axis_deg.toFixed(1)}°` : "—";
}

// --- Controls --------------------------------------------------------------

el.btnScan.onclick = scanDevices;
el.btnDisconnect.onclick = disconnect;
el.btnArm.onclick = () => api("/v1/session/arm", { method: "POST" }).then(refreshStatus);
el.btnDisarm.onclick = () => api("/v1/session/disarm", { method: "POST" }).then(refreshStatus);
el.selectMode.onchange = () =>
  api("/v1/session/mode", { method: "POST", body: JSON.stringify({ mode: el.selectMode.value }) }).then(refreshStatus);
el.selectHand.onchange = () =>
  api("/v1/session/hand", { method: "POST", body: JSON.stringify({ hand: el.selectHand.value }) }).then(refreshStatus);
el.btnClearLog.onclick = () => {
  el.eventLog.innerHTML = "";
};

// --- Live events (SSE) ------------------------------------------------------

function connectEvents() {
  const source = new EventSource(`${API}/v1/events`);
  const kinds = ["discovered", "connected", "disconnected", "status", "ready", "shot_started", "shot", "misread", "error"];
  for (const kind of kinds) {
    source.addEventListener(kind, (ev) => {
      let data = {};
      try {
        data = JSON.parse(ev.data);
      } catch {
        /* ignore */
      }
      handleEvent(kind, data);
    });
  }
  source.onerror = () => {
    // EventSource auto-reconnects; just note it in the log once in a while.
  };
}

function handleEvent(kind, data) {
  switch (kind) {
    case "connected":
      logEvent("status", `connected: ${data.name ?? ""} (${data.address ?? ""})`);
      refreshStatus();
      break;
    case "disconnected":
      logEvent("status", `disconnected: ${data.reason ?? ""}`);
      showNoSession();
      break;
    case "status":
      logEvent("status", `battery ${data.battery_pct ?? "?"}% · armed ${!!data.armed} · rssi ${data.rssi ?? "?"}`);
      refreshStatus();
      break;
    case "ready":
      logEvent("status", "armed and ready");
      refreshStatus();
      break;
    case "shot_started":
      logEvent("status", "shot detected…");
      break;
    case "shot":
      logEvent("shot", JSON.stringify(data));
      renderShot(data);
      break;
    case "misread":
      logEvent("misread", data.reason ?? "shot could not be read");
      break;
    case "error":
      logEvent("error", data.message ?? JSON.stringify(data));
      break;
    default:
      logEvent(kind, JSON.stringify(data));
  }
}

// --- Boot --------------------------------------------------------------------

pollHealth();
setInterval(pollHealth, 3000);
refreshStatus();
connectEvents();
