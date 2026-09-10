// trakr-ui frontend. Talks to the embedded daemon exactly the way the CLI or
// an agent would: plain fetch() and EventSource against the documented HTTP
// + SSE API (docs/api.md). No Tauri IPC is used for app logic on purpose.
const API = "http://127.0.0.1:7011";

let lastDevice = null;
let lastShot = null;

const $ = (id) => document.getElementById(id);

const el = {
  inputBroadcast: $("input-broadcast"),
  daemonDot: $("daemon-dot"),
  daemonLabel: $("daemon-label"),
  deviceList: $("device-list"),
  btnScan: $("btn-scan"),
  btnConnectSimulated: $("btn-connect-simulated"),
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
  fireShot: $("fire-shot"),
  firePlayer: $("fire-player"),
  fireClub: $("fire-club"),
  fireNumbers: $("fire-numbers"),
  fireClubNote: $("fire-club-note"),
  fireSpeed: $("fire-speed"),
  fireVla: $("fire-vla"),
  fireHla: $("fire-hla"),
  fireSpin: $("fire-spin"),
  fireAxis: $("fire-axis"),
  btnFireShot: $("btn-fire-shot"),
  inputPlayerName: $("input-player-name"),
  inputPlayerHand: $("input-player-hand"),
  btnPlayerAdd: $("btn-player-add"),
  playerList: $("player-list"),
  playerBag: $("player-bag"),
  playerBagName: $("player-bag-name"),
  btnPlayerDeselect: $("btn-player-deselect"),
  playerBagList: $("player-bag-list"),
  inputBagClub: $("input-bag-club"),
  inputBagCarry: $("input-bag-carry"),
  btnBagSetClub: $("btn-bag-set-club"),
  shotEmpty: $("shot-empty"),
  shotData: $("shot-data"),
  shotSpeed: $("shot-speed"),
  shotVla: $("shot-vla"),
  shotHla: $("shot-hla"),
  shotSpin: $("shot-spin"),
  shotAxis: $("shot-axis"),
  eventLog: $("event-log"),
  btnClearLog: $("btn-clear-log"),
  btnUpdate: $("btn-update"),
  btnSettings: $("btn-settings"),
  btnSettingsClose: $("btn-settings-close"),
  btnSettingsSave: $("btn-settings-save"),
  settingsBackdrop: $("settings-backdrop"),
  settingsSaveStatus: $("settings-save-status"),
  settingChipOnLobWedge: $("setting-chip-on-lob-wedge"),
  settingForceChipDistance: $("setting-force-chip-distance"),
  settingChipVia: $("setting-chip-via"),
  settingDebugMode: $("setting-debug-mode"),
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

async function connectSimulated() {
  logEvent("status", "connecting to a simulated device…");
  await api("/v1/session", { method: "POST", body: JSON.stringify({ kind: "simulated" }) });
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
  el.fireShot.classList.toggle("hidden", device?.kind !== "simulated");

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

// --- Players / clubs ---------------------------------------------------------
// Reference data for "Fire test shot": GET /v1/clubs is the typical
// launch/spin table trakr-daemon uses to turn a club + carry distance into
// plausible ball data; players' bags override the carry distance per club.

let clubProfiles = [];
let players = [];
let activePlayerName = null; // which player's bag is open for editing below

async function loadClubs() {
  const { ok, body } = await api("/v1/clubs");
  clubProfiles = ok ? body.clubs ?? [] : [];
  el.inputBagClub.innerHTML = clubProfiles.map((c) => `<option value="${c.club}">${c.club}</option>`).join("");
  populateFireClubOptions();
}

async function loadPlayers() {
  const { ok, body } = await api("/v1/players");
  players = ok ? body.players ?? [] : [];
  renderPlayerList();
  const previousPlayer = el.firePlayer.value;
  el.firePlayer.innerHTML = ['<option value="">(none)</option>']
    .concat(players.map((p) => `<option value="${p.name}">${p.name}</option>`))
    .join("");
  if ([...el.firePlayer.options].some((o) => o.value === previousPlayer)) {
    el.firePlayer.value = previousPlayer;
  }
  populateFireClubOptions();
  updateFireClubNote();
}

/// The "Club" dropdown in Fire Shot shows the selected player's own bag
/// (their real carry distances) when they have one, falling back to the
/// generic reference table (see GET /v1/clubs) otherwise.
function populateFireClubOptions() {
  const player = players.find((p) => p.name === el.firePlayer.value);
  const source =
    player && player.bag.length > 0
      ? player.bag
      : clubProfiles.map((c) => ({ club: c.club, carry_yd: c.carry_yd }));
  const previousClub = el.fireClub.value;
  el.fireClub.innerHTML = ['<option value="">Custom numbers</option>']
    .concat(source.map((c) => `<option value="${c.club}">${c.club} (~${Math.round(c.carry_yd)}yd)</option>`))
    .join("");
  if ([...el.fireClub.options].some((o) => o.value === previousClub)) {
    el.fireClub.value = previousClub;
  }
}

function renderPlayerList() {
  if (players.length === 0) {
    el.playerList.innerHTML = '<li class="empty">No players yet. Add one to give "Fire test shot" realistic per-club carry distances.</li>';
    return;
  }
  el.playerList.innerHTML = "";
  for (const p of players) {
    const li = document.createElement("li");
    li.className = `bag-row${p.name === activePlayerName ? " active" : ""}`;
    const info = document.createElement("div");
    info.innerHTML = `<strong>${p.name}</strong><span class="player-meta">${p.right_handed ? "right" : "left"} · ${p.bag.length} club${p.bag.length === 1 ? "" : "s"}</span>`;
    const actions = document.createElement("div");
    actions.className = "controls";
    const selectBtn = document.createElement("button");
    selectBtn.textContent = p.name === activePlayerName ? "Editing bag" : "Edit bag";
    selectBtn.disabled = p.name === activePlayerName;
    selectBtn.onclick = () => selectPlayer(p.name);
    const removeBtn = document.createElement("button");
    removeBtn.textContent = "Remove";
    removeBtn.onclick = () => deletePlayer(p.name);
    actions.append(selectBtn, removeBtn);
    li.append(info, actions);
    el.playerList.appendChild(li);
  }
}

function selectPlayer(name) {
  activePlayerName = name;
  el.playerBag.classList.remove("hidden");
  el.playerBagName.textContent = name;
  renderPlayerList();
  renderBag();
}

function deselectPlayer() {
  activePlayerName = null;
  el.playerBag.classList.add("hidden");
  renderPlayerList();
}

function renderBag() {
  const player = players.find((p) => p.name === activePlayerName);
  const bag = player?.bag ?? [];
  if (bag.length === 0) {
    el.playerBagList.innerHTML = '<li class="empty">No clubs yet.</li>';
    return;
  }
  el.playerBagList.innerHTML = "";
  for (const c of bag) {
    const li = document.createElement("li");
    li.className = "bag-row";
    const info = document.createElement("div");
    info.textContent = `${c.club} — ${Math.round(c.carry_yd)}yd`;
    const removeBtn = document.createElement("button");
    removeBtn.textContent = "Remove";
    removeBtn.onclick = () => removeBagClub(c.club);
    li.append(info, removeBtn);
    el.playerBagList.appendChild(li);
  }
}

async function addPlayer() {
  const name = el.inputPlayerName.value.trim();
  if (!name) return;
  const { ok } = await api("/v1/players", {
    method: "POST",
    body: JSON.stringify({ name, right_handed: el.inputPlayerHand.value === "right" }),
  });
  if (ok) {
    el.inputPlayerName.value = "";
    logEvent("status", `player added: ${name}`);
    await loadPlayers();
  }
}

async function deletePlayer(name) {
  await api(`/v1/players/${encodeURIComponent(name)}`, { method: "DELETE" });
  if (name === activePlayerName) deselectPlayer();
  logEvent("status", `player removed: ${name}`);
  await loadPlayers();
}

async function setBagClub() {
  if (!activePlayerName) return;
  const club = el.inputBagClub.value;
  const carryYd = Number(el.inputBagCarry.value);
  if (!club || !carryYd) return;
  await api(`/v1/players/${encodeURIComponent(activePlayerName)}/clubs/${encodeURIComponent(club)}`, {
    method: "PUT",
    body: JSON.stringify({ carry_yd: carryYd }),
  });
  el.inputBagCarry.value = "";
  await loadPlayers();
  renderBag();
}

async function removeBagClub(club) {
  if (!activePlayerName) return;
  await api(`/v1/players/${encodeURIComponent(activePlayerName)}/clubs/${encodeURIComponent(club)}`, {
    method: "DELETE",
  });
  await loadPlayers();
  renderBag();
}

function updateFireClubNote() {
  const club = el.fireClub.value;
  const isClubMode = !!club;
  el.fireNumbers.classList.toggle("hidden", isClubMode);
  el.fireClubNote.classList.toggle("hidden", !isClubMode);
  if (!isClubMode) return;
  const profile = clubProfiles.find((c) => c.club === club);
  const playerCarry = el.firePlayer.value
    ? players.find((p) => p.name === el.firePlayer.value)?.bag.find((c) => c.club.toUpperCase() === club.toUpperCase())?.carry_yd
    : null;
  const carryYd = playerCarry ?? profile?.carry_yd;
  el.fireClubNote.textContent =
    playerCarry != null
      ? `Using ${el.firePlayer.value}'s ${club}: ${Math.round(carryYd)}yd carry`
      : `Using reference ${club}: ${Math.round(carryYd)}yd carry (set this club in a player's bag for their real distance)`;
}

el.btnPlayerAdd.onclick = addPlayer;
el.btnPlayerDeselect.onclick = deselectPlayer;
el.btnBagSetClub.onclick = setBagClub;
el.fireClub.onchange = updateFireClubNote;
el.firePlayer.onchange = () => {
  populateFireClubOptions();
  updateFireClubNote();
};

// --- Controls --------------------------------------------------------------

el.btnScan.onclick = scanDevices;
el.btnConnectSimulated.onclick = connectSimulated;
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
el.btnFireShot.onclick = () => {
  const club = el.fireClub.value;
  const body = club
    ? { club, ...(el.firePlayer.value ? { player: el.firePlayer.value } : {}) }
    : {
        speed_mph: Number(el.fireSpeed.value),
        vla_deg: Number(el.fireVla.value),
        hla_deg: Number(el.fireHla.value),
        spin_rpm: Number(el.fireSpin.value),
        axis_deg: Number(el.fireAxis.value),
      };
  api("/v1/session/shot", { method: "POST", body: JSON.stringify(body) });
};

// --- Developer / debug mode --------------------------------------------------
// Local to this machine only (localStorage), unrelated to the daemon-backed
// chip settings below -- reveals "Connect (Simulated)" for testing without
// real hardware. Applies immediately, no Save needed.

const DEBUG_MODE_KEY = "trakr.debugMode";

function loadDebugMode() {
  try {
    return localStorage.getItem(DEBUG_MODE_KEY) === "true";
  } catch {
    return false;
  }
}

function applyDebugMode(on) {
  el.btnConnectSimulated.classList.toggle("hidden", !on);
}

el.settingDebugMode.onchange = () => {
  const on = el.settingDebugMode.checked;
  try {
    localStorage.setItem(DEBUG_MODE_KEY, on ? "true" : "false");
  } catch {
    /* ignore */
  }
  applyDebugMode(on);
  logEvent("status", `debug mode ${on ? "enabled" : "disabled"}`);
};

// --- Settings ----------------------------------------------------------------

async function openSettings() {
  el.settingsSaveStatus.textContent = "";
  const { ok, body } = await api("/v1/settings");
  if (ok) {
    el.settingChipOnLobWedge.checked = !!body.chip_on_lob_wedge;
    el.settingForceChipDistance.value = body.force_chip_distance_yd ?? "";
    el.settingChipVia.value = body.chip_via_putting ? "true" : "false";
  }
  el.settingDebugMode.checked = loadDebugMode();
  el.settingsBackdrop.classList.remove("hidden");
}

function closeSettings() {
  el.settingsBackdrop.classList.add("hidden");
}

async function saveSettings() {
  const distanceRaw = el.settingForceChipDistance.value.trim();
  const body = {
    chip_on_lob_wedge: el.settingChipOnLobWedge.checked,
    force_chip_distance_yd: distanceRaw === "" ? null : Number(distanceRaw),
    chip_via_putting: el.settingChipVia.value === "true",
  };
  el.settingsSaveStatus.textContent = "Saving…";
  const { ok } = await api("/v1/settings", { method: "POST", body: JSON.stringify(body) });
  el.settingsSaveStatus.textContent = ok ? "Saved" : "Failed to save";
  if (ok) {
    logEvent("status", "settings saved");
    setTimeout(closeSettings, 500);
  }
}

el.btnSettings.onclick = openSettings;
el.btnSettingsClose.onclick = closeSettings;
el.btnSettingsSave.onclick = saveSettings;
el.settingsBackdrop.onclick = (ev) => {
  if (ev.target === el.settingsBackdrop) closeSettings();
};
document.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape" && !el.settingsBackdrop.classList.contains("hidden")) closeSettings();
});

// --- Auto-update -------------------------------------------------------------
// Uses the plugin's global JS bindings (window.__TAURI__.updater/process, via
// `withGlobalTauri` in tauri.conf.json) instead of an npm import, matching
// this app's no-bundler setup. Guarded so it's a no-op outside the Tauri
// shell (e.g. loading src/index.html directly in a browser for UI dev).

let pendingUpdate = null;

async function checkForUpdate() {
  const updater = window.__TAURI__?.updater;
  if (!updater) return;
  try {
    const update = await updater.check();
    if (update?.available) {
      pendingUpdate = update;
      el.btnUpdate.textContent = `Update to v${update.version}`;
      el.btnUpdate.classList.remove("hidden");
      logEvent("status", `update available: v${update.version}`);
    }
  } catch (e) {
    logEvent("error", `update check failed: ${e}`);
  }
}

async function installUpdate() {
  if (!pendingUpdate) return;
  el.btnUpdate.disabled = true;
  el.btnUpdate.textContent = "Downloading…";
  try {
    await pendingUpdate.downloadAndInstall((event) => {
      if (event.event === "Progress") el.btnUpdate.textContent = "Installing…";
    });
    logEvent("status", "update installed, relaunching…");
    await window.__TAURI__.process.relaunch();
  } catch (e) {
    logEvent("error", `update install failed: ${e}`);
    el.btnUpdate.disabled = false;
    el.btnUpdate.textContent = `Update to v${pendingUpdate.version}`;
  }
}

el.btnUpdate.onclick = installUpdate;

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
      lastDevice = data;
      logEvent("status", `connected: ${data.name ?? ""} (${data.address ?? ""})`);
      // One fetch here to pick up anything a bare status/shot event can't
      // carry (e.g. a shot from before this window opened); every
      // subsequent status/shot update below renders straight from the SSE
      // payload with no extra round trip.
      refreshStatus();
      break;
    case "disconnected":
      lastDevice = null;
      lastShot = null;
      logEvent("status", `disconnected: ${data.reason ?? ""}`);
      showNoSession();
      break;
    case "status":
      // The box pushes these every ~second; they're already reflected live
      // in the Session stats panel, so logging each one would drown out the
      // actually interesting events (connect, arm, shot, error) in the log.
      renderSession({ device: lastDevice, status: data, last_shot: lastShot });
      break;
    case "ready":
      logEvent("status", "armed and ready");
      break;
    case "shot_started":
      logEvent("status", "shot detected…");
      break;
    case "shot":
      lastShot = data;
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

applyDebugMode(loadDebugMode());
pollHealth();
setInterval(pollHealth, 3000);
refreshStatus();
connectEvents();
loadClubs();
loadPlayers();
checkForUpdate();
