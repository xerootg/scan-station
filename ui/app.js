"use strict";

// Tauri v2 exposes the API at window.__TAURI__ because withGlobalTauri is on.
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

const state = {
  source: "simplex",
  mode: "color",
  resolution: 300,
  device: null,
  destinations: [], // [{id,label,available}]
  selected: new Set(),
  pageCount: 0,
};

const $ = (sel) => document.querySelector(sel);

function toast(msg, isErr) {
  const t = $("#toast");
  t.textContent = msg;
  t.classList.toggle("err", !!isErr);
  t.classList.remove("hidden");
  clearTimeout(toast._t);
  toast._t = setTimeout(() => t.classList.add("hidden"), 3500);
}

function busy(on, text) {
  $("#busyText").textContent = text || "Working…";
  $("#busy").classList.toggle("hidden", !on);
}

/* ---- segmented controls ---- */
function wireSegment(groupId, onPick) {
  const group = document.getElementById(groupId);
  group.querySelectorAll("button").forEach((btn) => {
    btn.addEventListener("click", () => {
      group.querySelectorAll("button").forEach((b) => b.classList.remove("on"));
      btn.classList.add("on");
      onPick(btn.dataset.val);
    });
  });
}

/* ---- scanner status ---- */
async function refreshScanners() {
  const pill = $("#scanner");
  pill.className = "pill pill-wait";
  pill.textContent = "Looking for scanner…";
  try {
    const list = await invoke("list_scanners");
    if (list.length === 0) {
      pill.className = "pill pill-err";
      pill.textContent = "No scanner — is it powered on? Tap to retry";
      state.device = null;
      return;
    }
    state.device = list[0].device;
    pill.className = "pill pill-ok";
    pill.textContent = list[0].description || list[0].device;
  } catch (e) {
    pill.className = "pill pill-err";
    pill.textContent = "Scanner error — tap to retry";
    state.device = null;
  }
}

/* ---- page strip ---- */
function renderPages(pages) {
  state.pageCount = pages.length;
  $("#pageCount").textContent = String(pages.length);
  const strip = $("#strip");
  strip.innerHTML = "";
  pages.forEach((p, i) => {
    const el = document.createElement("div");
    el.className = "thumb";
    el.innerHTML =
      `<span class="num">${i + 1}</span>` +
      `<button class="del" aria-label="Delete page ${i + 1}">&times;</button>` +
      `<img alt="Page ${i + 1}" src="${p.thumbnail}" />`;
    el.querySelector(".del").addEventListener("click", () => deletePage(p.id));
    strip.appendChild(el);
  });

  const has = pages.length > 0;
  $("#previewHint").classList.toggle("hidden", has);
  $("#clearBtn").disabled = !has;
  $("#sendBtn").disabled = !has;
  $("#scanBtn").textContent = has ? "Scan More" : "Scan";
}

async function refreshPages() {
  try {
    renderPages(await invoke("pages"));
  } catch (e) {
    toast(String(e), true);
  }
}

async function deletePage(id) {
  try {
    await invoke("delete_page", { id });
    await refreshPages();
  } catch (e) {
    toast(String(e), true);
  }
}

/* ---- scan ---- */
async function doScan() {
  if (!state.device) {
    await refreshScanners();
    if (!state.device) {
      toast("No scanner found — check that it's powered on and connected", true);
      return;
    }
  }
  busy(true, "Scanning…");
  try {
    const res = await invoke("scan", {
      opts: {
        device: state.device,
        source: state.source,
        mode: state.mode,
        resolution: state.resolution,
      },
    });
    await refreshPages();
    toast(`Added ${res.added.length} page(s)`);
  } catch (e) {
    toast(String(e), true);
  } finally {
    busy(false);
  }
}

async function doClear() {
  try {
    await invoke("clear");
    await refreshPages();
  } catch (e) {
    toast(String(e), true);
  }
}

/* ---- send dialog ---- */
function renderDestinations() {
  const list = $("#destList");
  list.innerHTML = "";
  state.destinations.forEach((d) => {
    const b = document.createElement("button");
    b.className = "dest" + (state.selected.has(d.id) ? " on" : "");
    b.textContent = d.available ? d.label : `${d.label} (not configured)`;
    b.disabled = !d.available;
    b.addEventListener("click", () => {
      if (state.selected.has(d.id)) state.selected.delete(d.id);
      else state.selected.add(d.id);
      $("#recipientField").hidden = !state.selected.has("email");
      renderDestinations();
    });
    list.appendChild(b);
  });
}

function openSend() {
  $("#sendResults").innerHTML = "";
  $("#title").value = defaultTitle();
  $("#recipientField").hidden = !state.selected.has("email");
  renderDestinations();
  $("#sendOverlay").classList.remove("hidden");
}

function closeSend() {
  $("#sendOverlay").classList.add("hidden");
}

function defaultTitle() {
  const d = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  return `Scan ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}${pad(d.getMinutes())}`;
}

async function doSend() {
  const targets = [...state.selected];
  if (targets.length === 0) {
    toast("Pick at least one destination", true);
    return;
  }
  const title = $("#title").value;
  const recipient = $("#recipient").value.trim() || null;
  busy(true, "Sending…");
  try {
    const results = await invoke("upload", { targets, title, recipient });
    busy(false);
    const box = $("#sendResults");
    box.innerHTML = "";
    let allOk = true;
    results.forEach((r) => {
      if (!r.ok) allOk = false;
      const row = document.createElement("div");
      row.className = "row " + (r.ok ? "ok" : "bad");
      const label = (state.destinations.find((d) => d.id === r.target) || {}).label || r.target;
      row.textContent = `${label}: ${r.detail}`;
      box.appendChild(row);
    });
    if (allOk) {
      toast("Sent");
      await doClear();
      setTimeout(closeSend, 1200);
    } else {
      toast("Some destinations failed", true);
    }
  } catch (e) {
    busy(false);
    toast(String(e), true);
  }
}

/* ---- init ---- */
async function init() {
  wireSegment("source", (v) => (state.source = v));
  wireSegment("mode", (v) => (state.mode = v));
  wireSegment("resolution", (v) => (state.resolution = parseInt(v, 10)));

  $("#scanBtn").addEventListener("click", doScan);
  $("#clearBtn").addEventListener("click", doClear);
  $("#sendBtn").addEventListener("click", openSend);
  $("#sendCancel").addEventListener("click", closeSend);
  $("#sendGo").addEventListener("click", doSend);
  $("#scanner").addEventListener("click", refreshScanners);

  try {
    const cfg = await invoke("config_info");
    state.destinations = cfg.destinations || [];
    // preselect the first available destination
    const firstAvail = state.destinations.find((d) => d.available);
    if (firstAvail) state.selected.add(firstAvail.id);
    // apply configured scan defaults to the segmented controls
    applyDefault("source", cfg.source, (v) => (state.source = v));
    applyDefault("mode", cfg.mode, (v) => (state.mode = v));
    applyDefault("resolution", String(cfg.resolution), (v) => (state.resolution = parseInt(v, 10)));
  } catch (e) {
    toast("Config load failed: " + e, true);
  }

  await refreshScanners();
  await refreshPages();
}

function applyDefault(groupId, val, setter) {
  if (val === undefined || val === null) return;
  const group = document.getElementById(groupId);
  const btn = group.querySelector(`button[data-val="${val}"]`);
  if (!btn) return;
  group.querySelectorAll("button").forEach((b) => b.classList.remove("on"));
  btn.classList.add("on");
  setter(String(val));
}

window.addEventListener("DOMContentLoaded", init);
