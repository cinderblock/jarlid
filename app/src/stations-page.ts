// The Stations page: browse and play the whole collection, or select stations to
// export their preferences (and, later, import them back).
//
// This is a full page rather than the dropdown it started as. An export walks the
// collection one station at a time with a deliberate gap between each, so it needs
// somewhere to show a list, live progress and a result — and it must not disappear
// because a click landed outside a popover.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

export interface StationInfo {
  name: string;
  token: string;
  isQuickMix: boolean;
  isGenreStation: boolean;
  isThumbprint: boolean;
}

interface ExportProgress {
  done: number;
  total: number;
  station: string;
}
interface StationPlan {
  name: string;
  exists: boolean;
  seedsToAdd: number;
  seedsAlreadyThere: number;
  seedsUnusable: number;
  thumbsNotRestorable: number;
  blocked?: string;
}
interface ImportPlan {
  exportedAt: string;
  exportedBy: string;
  stations: StationPlan[];
  notes: string[];
}

interface ExportResult {
  path: string | null; // null = the save dialog was dismissed
  stations: number;
  thumbs: number;
  seeds: number;
  skipped: string[];
  stoppedReason: string | null;
}

const page = $("stations-page");
const search = $<HTMLInputElement>("sp-search");
const selectBtn = $<HTMLButtonElement>("sp-select");
const closeBtn = $<HTMLButtonElement>("sp-close");
const bulk = $("sp-bulk");
const allBox = $<HTMLInputElement>("sp-all");
const countEl = $("sp-count");
const listEl = $("sp-list");
const statusEl = $("sp-status");
const exportBtn = $<HTMLButtonElement>("sp-export");
const importBtn = $<HTMLButtonElement>("sp-import");
const cancelBtn = $<HTMLButtonElement>("sp-cancel");

let stations: StationInfo[] = [];
let activeName = "";
let selectMode = false;
let busy = false;
const selected = new Set<string>();

/** Called by main.ts when the engine publishes a new station list. */
export function setStations(next: StationInfo[], active: string) {
  stations = next;
  if (previewing) return; // an import preview owns the list until dismissed
  if (active) activeName = active;
  // Forget selections for stations that no longer exist — but never mid-run, when
  // the export is already working from its own copy of the list.
  if (!busy) {
    const live = new Set(next.map((s) => s.token));
    for (const t of [...selected]) if (!live.has(t)) selected.delete(t);
  }
  if (!page.hidden) render();
}

export function setActiveStation(name: string) {
  activeName = name;
  if (!page.hidden) render();
}

export function isOpen() {
  return !page.hidden;
}

const matches = (s: StationInfo, f: string) => !f || s.name.toLowerCase().includes(f);

function visible(): StationInfo[] {
  const f = search.value.trim().toLowerCase();
  return stations.filter((s) => matches(s, f));
}

function tag(text: string) {
  const el = document.createElement("span");
  el.className = "sp-tag";
  el.textContent = text;
  return el;
}

function render() {
  const rows = visible();
  listEl.innerHTML = "";

  if (!rows.length) {
    const empty = document.createElement("div");
    empty.className = "sp-name";
    empty.style.color = "var(--faint)";
    empty.style.padding = "10px";
    empty.textContent = stations.length
      ? "No stations match that search."
      : "No stations loaded yet.";
    listEl.appendChild(empty);
    refreshSelectionUi();
    return;
  }

  for (const st of rows) {
    const row = document.createElement("button");
    row.className = "sp-row" + (st.name === activeName ? " active" : "");
    row.type = "button";

    if (selectMode) {
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = selected.has(st.token);
      row.appendChild(box);
    }

    const name = document.createElement("span");
    name.className = "sp-name";
    name.textContent = st.name;
    row.appendChild(name);

    // QuickMix has no seeds or thumbs of its own — it is a shuffle over other
    // stations — so flagging it explains an otherwise-surprising empty export.
    if (st.isQuickMix) row.appendChild(tag("Mix"));
    else if (st.isThumbprint) row.appendChild(tag("Thumbprint"));
    else if (st.isGenreStation) row.appendChild(tag("Genre"));

    row.addEventListener("click", () => {
      if (busy) return;
      if (selectMode) {
        if (selected.has(st.token)) selected.delete(st.token);
        else selected.add(st.token);
        render();
      } else {
        invoke("native_play_station", { name: st.name, token: st.token }).catch((e) =>
          setStatus(String(e), "err")
        );
        activeName = st.name;
        close();
      }
    });
    listEl.appendChild(row);
  }
  refreshSelectionUi();
}

function refreshSelectionUi() {
  const vis = visible();
  const hit = vis.filter((s) => selected.has(s.token)).length;
  allBox.checked = vis.length > 0 && hit === vis.length;
  allBox.indeterminate = hit > 0 && hit < vis.length;
  allBox.nextElementSibling!.textContent = search.value.trim()
    ? `Select all ${vis.length} matching`
    : `Select all ${vis.length}`;
  countEl.textContent = selected.size ? `${selected.size} selected` : "";

  exportBtn.disabled = busy || selected.size === 0;
  exportBtn.textContent = busy
    ? "Exporting…"
    : selected.size
      ? `Export ${selected.size}…`
      : "Export…";
  importBtn.disabled = busy;
}

function setStatus(text: string, kind: "" | "ok" | "err" = "") {
  statusEl.textContent = text;
  statusEl.classList.toggle("ok", kind === "ok");
  statusEl.classList.toggle("err", kind === "err");
}

function setSelectMode(on: boolean) {
  selectMode = on;
  selectBtn.setAttribute("aria-pressed", String(on));
  selectBtn.textContent = on ? "Done" : "Select";
  bulk.hidden = !on;
  if (!on) selected.clear();
  render();
}

function setBusy(on: boolean) {
  busy = on;
  cancelBtn.hidden = !on;
  search.disabled = on;
  selectBtn.disabled = on;
  allBox.disabled = on;
  closeBtn.disabled = on;
  refreshSelectionUi();
}

export function open() {
  page.hidden = false;
  search.value = "";
  setStatus("");
  render();
  search.focus();
}

function close() {
  if (busy) return; // never pull the page out from under a running export
  page.hidden = true;
  if (selectMode) setSelectMode(false);
}

closeBtn.addEventListener("click", close);
selectBtn.addEventListener("click", () => !busy && setSelectMode(!selectMode));
search.addEventListener("input", render);

allBox.addEventListener("click", (e) => {
  e.stopPropagation();
  if (busy) return;
  const vis = visible();
  const allOn = vis.every((s) => selected.has(s.token));
  for (const s of vis) {
    if (allOn) selected.delete(s.token);
    else selected.add(s.token);
  }
  render();
});

listen<ExportProgress>("export://progress", (e) => {
  const { done, total, station } = e.payload;
  setStatus(`${done}/${total} — ${station}`);
});

exportBtn.addEventListener("click", async () => {
  if (busy || !selected.size) return;
  // Collection order, so the file reads the same way the list does.
  const picked: [string, string][] = stations
    .filter((s) => selected.has(s.token))
    .map((s) => [s.name, s.token]);

  setBusy(true);
  setStatus(`Starting — ${picked.length} station${picked.length === 1 ? "" : "s"}…`);
  try {
    const r = await invoke<ExportResult>("export_stations", { stations: picked });
    // A run that stopped early still produced something worth keeping, so report
    // what was saved AND why it is short rather than claiming plain success.
    const parts: string[] = [];
    if (r.path) parts.push(`Saved ${r.stations} stations — ${r.thumbs} thumbs, ${r.seeds} seeds.`);
    else parts.push(`Not saved (${r.stations} stations were collected).`);
    if (r.stoppedReason) parts.push(`Stopped early: ${r.stoppedReason}.`);
    if (r.skipped.length) parts.push(`${r.skipped.length} station(s) failed and were skipped.`);
    const bad = !!r.stoppedReason || r.skipped.length > 0;
    setStatus(parts.join(" "), bad ? "err" : r.path ? "ok" : "");
  } catch (err) {
    setStatus(String(err), "err");
  } finally {
    setBusy(false);
  }
});

cancelBtn.addEventListener("click", () => {
  setStatus("Cancelling…");
  invoke("cancel_export").catch(() => {});
});

// Previewing an import takes over the list. Nothing is written to the account by this —
// it reads the file and says what applying it *would* do.
let previewing = false;

function showPlan(plan: ImportPlan) {
  previewing = true;
  selectBtn.disabled = true;
  search.disabled = true;
  importBtn.textContent = "Back to stations";
  listEl.innerHTML = "";

  const create = plan.stations.filter((s) => !s.exists && !s.blocked).length;
  const seeds = plan.stations
    .filter((s) => !s.blocked)
    .reduce((n, s) => n + s.seedsToAdd, 0);
  const thumbs = plan.stations.reduce((n, s) => n + s.thumbsNotRestorable, 0);

  for (const st of plan.stations) {
    const row = document.createElement("div");
    row.className = "sp-row";
    const name = document.createElement("span");
    name.className = "sp-name";
    name.textContent = st.name;
    row.appendChild(name);

    const what = document.createElement("span");
    what.className = "sp-tag";
    what.textContent = st.blocked
      ? "skipped"
      : st.exists
        ? st.seedsToAdd
          ? `+${st.seedsToAdd} seeds`
          : "up to date"
        : "create";
    row.appendChild(what);
    // The reason a station is skipped matters more than the fact — put it on the row.
    if (st.blocked) {
      const why = document.createElement("span");
      why.className = "sp-why";
      why.textContent = st.blocked;
      row.appendChild(why);
    }
    listEl.appendChild(row);
  }

  setStatus(
    [
      `From ${plan.exportedBy || "an export"}${plan.exportedAt ? ` (${plan.exportedAt.slice(0, 10)})` : ""}:`,
      `${create} station(s) to create, ${seeds} seed(s) to add.`,
      thumbs ? `${thumbs} thumbs cannot be restored.` : "",
      "Nothing has been changed — applying is not built yet.",
    ]
      .filter(Boolean)
      .join(" ")
  );
}

function endPreview() {
  previewing = false;
  selectBtn.disabled = false;
  search.disabled = false;
  importBtn.textContent = "Import…";
  setStatus("");
  render();
}

importBtn.addEventListener("click", async () => {
  if (busy) return;
  if (previewing) {
    endPreview();
    return;
  }
  importBtn.disabled = true;
  try {
    const plan = await invoke<ImportPlan | null>("import_preview");
    if (plan) showPlan(plan);
  } catch (err) {
    setStatus(String(err), "err");
  } finally {
    importBtn.disabled = false;
  }
});

// Escape closes the page, unless an export is mid-flight.
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || page.hidden || busy) return;
  if (selectMode) setSelectMode(false);
  else close();
});
