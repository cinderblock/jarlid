// The Stations page: browse and play the whole collection, send a station to the speakers,
// or select stations to export their preferences (and import them back).
//
// This is a full page rather than the dropdown it started as. An export walks the
// collection one station at a time with a deliberate gap between each, so it needs
// somewhere to show a list, live progress and a result — and it must not disappear
// because a click landed outside a popover.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import * as stationStats from "./station-stats";
import { createSelect } from "./select";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

export interface StationInfo {
  name: string;
  token: string;
  isQuickMix: boolean;
  isGenreStation: boolean;
  isThumbprint: boolean;
  /// Pandora's cover for the station. Empty when it has none — rows fall back to the last
  /// track played from the station, then to a tile drawn from the name.
  artUrl?: string;
  /// Epoch milliseconds, or null when Pandora's `dateCreated` was absent or unreadable.
  dateCreated?: number | null;
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
const sortRoot = $("sp-sort");
const exportBtn = $<HTMLButtonElement>("sp-export");
const importBtn = $<HTMLButtonElement>("sp-import");
const cancelBtn = $<HTMLButtonElement>("sp-cancel");

let stations: StationInfo[] = [];
let activeName = "";

// ---- casting -------------------------------------------------------------
// Jarlid never streams to the speakers; a WiiM plays Pandora itself. "Cast this station" asks
// the device to start it by its tuner token, over the proprietary PlayQueue service — so any
// station casts, not just ones set up as presets in the WiiM Home app. The button shows only
// when a device that can do this is on the LAN.
/// The network player's name, or empty when none is on the LAN.
let remoteDevice = "";
/// Whether that device can start a station itself (a WiiM with a PlayQueue service).
let canCast = false;

/** Called by main.ts whenever the remote-player state arrives; cheap unless something changed. */
export function setRemoteDevice(name: string, cast: boolean) {
  if (name === remoteDevice && cast === canCast) return;
  remoteDevice = name;
  canCast = cast;
  if (!page.hidden) render();
}

const castable = () => !!remoteDevice && canCast && !selectMode;

function cast(st: StationInfo) {
  // Moving playback, not adding a second source: the local engine stops, and the display
  // follows the speakers once they report what they are playing.
  invoke("remote_play_station", { name: st.name, token: st.token })
    .then(() => invoke("player_cmd", { cmd: "pause" }).catch(() => {}))
    .catch((e) => setStatus(String(e), "err"));
  close();
}

const CAST_ICON =
  '<svg viewBox="0 0 24 24"><path d="M12 19h7a2 2 0 0 0 2-2V6a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v1"/><path d="M3 11a9 9 0 0 1 9 9M3 15a5 5 0 0 1 5 5M3 20h.01"/></svg>';

function castButton(st: StationInfo) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "sp-cast";
  btn.setAttribute("aria-label", `Cast ${st.name} to ${remoteDevice}`);
  btn.innerHTML = CAST_ICON;
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    if (!busy) cast(st);
  });
  return btn;
}

/// The playing station's token. Preferred over the name for deciding which row is the
/// active one, because a name is not unique.
let activeToken = "";
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

export function setActiveStation(name: string, token = "") {
  activeName = name;
  if (token) activeToken = token;
  if (!page.hidden) render();
}

/// Is this the station currently playing? By token once we have been told one, and by name
/// only until then — on a cold start the list can arrive before the active-station event.
const isActive = (st: StationInfo) =>
  activeToken ? st.token === activeToken : st.name === activeName;

export function isOpen() {
  return !page.hidden;
}

/// The row of the station currently playing, if it is on screen. main.ts needs it as the
/// landing site for the station spine's flight into the list.
export function activeRow(): HTMLElement | null {
  return listEl.querySelector<HTMLElement>(".sp-row.active");
}

const matches = (s: StationInfo, f: string) => !f || s.name.toLowerCase().includes(f);

// ---- sorting -------------------------------------------------------------
// Sorting happens *inside* the groups, never across them. The groups answer "where did this
// station come from", which is a different question from "which of these do I want first",
// and flattening them to sort would throw away the more useful of the two.

type SortKey = "pandora" | "name" | "recent" | "plays" | "created";

const SORTS: { value: SortKey; label: string }[] = [
  { value: "pandora", label: "Pandora's order" },
  { value: "name", label: "A–Z" },
  { value: "recent", label: "Recently played" },
  { value: "plays", label: "Most played" },
  { value: "created", label: "Newest first" },
];

const SORT_PREF = "stations-sort";
let sortKey: SortKey =
  (SORTS.find((s) => s.value === localStorage.getItem(SORT_PREF))?.value as SortKey) ?? "pandora";

/// Stations with nothing to sort by go last, in Pandora's order, rather than being scattered
/// through the middle as if they scored zero. "Never played" is not "played least recently",
/// and a station whose creation date we could not read is not the oldest one you have.
function sorted(rows: StationInfo[]): StationInfo[] {
  if (sortKey === "pandora") return rows;
  const rank = (st: StationInfo): number | null => {
    switch (sortKey) {
      case "recent":
        return stationStats.statFor(st.token)?.last ?? null;
      case "plays":
        return stationStats.statFor(st.token)?.plays || null;
      case "created":
        return st.dateCreated ?? null;
      default:
        return null;
    }
  };
  if (sortKey === "name") {
    const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
    return [...rows].sort((a, b) => collator.compare(a.name, b.name));
  }
  return [...rows]
    .map((st, i) => ({ st, i, r: rank(st) }))
    .sort((a, b) => {
      if (a.r === null && b.r === null) return a.i - b.i; // both unknown: leave them be
      if (a.r === null) return 1;
      if (b.r === null) return -1;
      return b.r - a.r; // most recent, most played, newest — all "bigger first"
    })
    .map((x) => x.st);
}

function visible(): StationInfo[] {
  const f = search.value.trim().toLowerCase();
  return stations.filter((s) => matches(s, f));
}

interface Group {
  title: string;
  note: string;
  items: StationInfo[];
  special: boolean;
}

/// Split the collection by *where a station came from*, which is the distinction that
/// actually matters: a QuickMix or Thumbprint is assembled by Pandora out of everything
/// you have, and a genre station is theirs rather than yours. Only the middle group is
/// stations you built, and that group is the one you are usually looking through.
///
/// Grouping also retires the per-row tags. A heading that says it once beats a badge
/// repeated down the right-hand edge of every row.
function groups(rows: StationInfo[]): Group[] {
  const auto = (s: StationInfo) => s.isQuickMix || s.isThumbprint;
  return [
    {
      title: "Mixes",
      note: "Built by Pandora out of your whole collection",
      items: rows.filter(auto),
      special: true,
    },
    {
      title: "Your stations",
      note: "",
      items: rows.filter((s) => !auto(s) && !s.isGenreStation),
      special: false,
    },
    {
      title: "Genre stations",
      note: "Pandora's own, not built from your thumbs",
      items: rows.filter((s) => !auto(s) && s.isGenreStation),
      special: true,
    },
  ].filter((g) => g.items.length > 0);
}

/// Up to two letters to stand in for a cover. Two words give their initials, one word gives
/// its first two letters — "Alt Nation" reads better as AN than as AL.
function initials(name: string) {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (!words.length) return "?";
  const letters = words.length > 1 ? words[0][0] + words[1][0] : words[0].slice(0, 2);
  return letters.toUpperCase();
}

/// A stable hue per station name, so the same station is the same colour every launch and
/// two stations next to each other are rarely the same one. Plain FNV-ish string hash.
function hueOf(name: string) {
  let h = 2166136261;
  for (let i = 0; i < name.length; i++) {
    h ^= name.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return Math.abs(h) % 360;
}

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 31557600],
  ["month", 2629800],
  ["week", 604800],
  ["day", 86400],
  ["hour", 3600],
  ["minute", 60],
];

/// "yesterday", "3 weeks ago" — the largest unit that fits, because nobody wants a station
/// last played in March described in hours.
function ago(ms: number) {
  const sec = Math.round((ms - Date.now()) / 1000);
  for (const [unit, size] of UNITS) {
    if (Math.abs(sec) >= size) return rtf.format(Math.round(sec / size), unit);
  }
  return "just now";
}

const monthYear = (ms: number) =>
  new Date(ms).toLocaleDateString(undefined, { month: "short", year: "numeric" });

/// The row's second line. What Jarlid has actually watched you do comes first, because it is
/// the more useful answer to "have I been here?"; the creation date is the fallback for a
/// station this client has never played, and there is simply no second line for one with
/// neither. An empty line is better than a row of "—".
function subtitle(st: StationInfo) {
  const stat = stationStats.statFor(st.token);
  const bits: string[] = [];
  if (stat?.last) bits.push(ago(stat.last));
  if (stat?.plays) bits.push(`${stat.plays} play${stat.plays === 1 ? "" : "s"}`);
  if (!bits.length && st.dateCreated) bits.push(`Added ${monthYear(st.dateCreated)}`);
  return bits.join(" · ");
}

/// Pandora's cover, else the art of the last track played from the station, else a tile drawn
/// from the name. The tile is always underneath rather than a separate case, so a cover URL
/// that 404s falls back to it instead of leaving a hole.
function stationArt(st: StationInfo) {
  const wrap = document.createElement("span");
  wrap.className = "sp-art";
  wrap.style.setProperty("--tile-hue", String(hueOf(st.name)));
  wrap.textContent = initials(st.name);

  const url = st.artUrl || stationStats.statFor(st.token)?.art;
  if (url) {
    const img = document.createElement("img");
    img.alt = "";
    img.loading = "lazy";
    img.decoding = "async";
    img.addEventListener("load", () => img.classList.add("ready"));
    img.addEventListener("error", () => img.remove());
    img.src = url;
    wrap.appendChild(img);
  }
  return wrap;
}

/// The columns the wide layout adds. Each is one fact the narrow layout squeezes into the
/// row's second line, given a column of its own so a run of them can be compared down the
/// page instead of read one row at a time. Always built; the container query decides which
/// of the two shapes is on screen, and building both costs three spans.
const CELLS: { cls: string; label: string; of: (st: StationInfo) => string }[] = [
  {
    cls: "sp-plays",
    label: "Plays",
    of: (st) => String(stationStats.statFor(st.token)?.plays || ""),
  },
  {
    cls: "sp-last",
    label: "Last played",
    of: (st) => {
      const last = stationStats.statFor(st.token)?.last;
      return last ? ago(last) : "";
    },
  },
  {
    cls: "sp-added",
    label: "Added",
    of: (st) => (st.dateCreated ? monthYear(st.dateCreated) : ""),
  },
];

function stationRow(st: StationInfo, special: boolean) {
  // A div acting as a button, because the row holds a real button (cast) and a button may
  // not contain another. Enter and Space do what a click does, below.
  const row = document.createElement("div");
  row.className = "sp-row" + (isActive(st) ? " active" : "") + (special ? " special" : "");
  row.setAttribute("role", "button");
  row.tabIndex = 0;
  row.dataset.token = st.token;

  if (selectMode) {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = selected.has(st.token);
    row.appendChild(box);
  }

  row.appendChild(stationArt(st));

  const text = document.createElement("span");
  text.className = "sp-text";
  const name = document.createElement("span");
  name.className = "sp-name";
  name.textContent = st.name;
  text.appendChild(name);
  const meta = subtitle(st);
  if (meta) {
    const sub = document.createElement("span");
    sub.className = "sp-meta";
    sub.textContent = meta;
    text.appendChild(sub);
  }
  row.appendChild(text);

  for (const c of CELLS) {
    const cell = document.createElement("span");
    cell.className = `sp-cell ${c.cls}`;
    cell.textContent = c.of(st);
    row.appendChild(cell);
  }

  if (castable()) row.appendChild(castButton(st));

  row.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    row.click();
  });
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
      activeToken = st.token;
      close();
    }
  });
  return row;
}

function render() {
  const rows = visible();
  listEl.innerHTML = "";

  // One centred column. A responsive grid meant scanning across *and* down at once to
  // find a name, which is the wrong shape for a list you read rather than browse.
  const col = document.createElement("div");
  col.className = "sp-col" + (selectMode ? " selecting" : "") + (castable() ? " cast" : "");
  listEl.appendChild(col);

  if (!rows.length) {
    const empty = document.createElement("div");
    empty.className = "sp-empty";
    empty.textContent = stations.length
      ? "No stations match that search."
      : "No stations loaded yet.";
    col.appendChild(empty);
    refreshSelectionUi();
    return;
  }

  // Column headings for the wide layout. Hidden by CSS in the narrow one, where there are
  // no columns to head, and in select mode, where the page is about picking rather than
  // comparing and the checkbox has shifted every column along by one.
  const headRow = document.createElement("div");
  headRow.className = "sp-head-row";
  headRow.appendChild(document.createElement("span"));
  const nameHead = document.createElement("span");
  nameHead.textContent = "Station";
  headRow.appendChild(nameHead);
  for (const c of CELLS) {
    const h = document.createElement("span");
    h.className = `sp-cell ${c.cls}`;
    h.textContent = c.label;
    headRow.appendChild(h);
  }
  // The cast column heads itself with its icon; a word over a column of the same icon
  // would say it twice.
  if (castable()) headRow.appendChild(document.createElement("span"));
  col.appendChild(headRow);

  for (const g of groups(rows)) {
    const head = document.createElement("div");
    head.className = "sp-group";
    const h = document.createElement("h3");
    h.textContent = g.title;
    head.appendChild(h);
    // The count belongs next to the heading, not on every row.
    const count = document.createElement("span");
    count.className = "sp-group-count";
    count.textContent = String(g.items.length);
    head.appendChild(count);
    col.appendChild(head);

    if (g.note) {
      const note = document.createElement("div");
      note.className = "sp-group-note";
      note.textContent = g.note;
      col.appendChild(note);
    }

    for (const st of sorted(g.items)) col.appendChild(stationRow(st, g.special));
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

/// How long `.closing` is left on before the page is actually hidden. Must not be shorter
/// than the closing animation in styles.css, or the panel vanishes mid-retreat.
const CLOSE_MS = 200;
let closeTimer: ReturnType<typeof setTimeout> | undefined;

export function open() {
  clearTimeout(closeTimer);
  page.classList.remove("closing");
  page.hidden = false;
  search.value = "";
  setStatus("");
  render();
  // Added after the render, so the panel animates in around contents that are already
  // laid out — the station name flying in from the player measures its landing row now.
  page.classList.add("opening");
  page.addEventListener("animationend", () => page.classList.remove("opening"), { once: true });
  search.focus();
}

function close() {
  if (busy) return; // never pull the page out from under a running export
  page.classList.remove("opening");
  page.classList.add("closing");
  // A timer rather than `animationend`: that event is dispatched on a frame, and a window
  // that is not on screen barely gets any — the page would sit there, closing, until you
  // came back to watch it.
  clearTimeout(closeTimer);
  closeTimer = setTimeout(() => {
    page.classList.remove("closing");
    page.hidden = true;
  }, CLOSE_MS);
  if (selectMode) setSelectMode(false);
}

closeBtn.addEventListener("click", close);
selectBtn.addEventListener("click", () => !busy && setSelectMode(!selectMode));
const sortSel = createSelect(sortRoot, SORTS, {
  label: "Sort stations",
  onChange: (v) => {
    sortKey = v as SortKey;
    localStorage.setItem(SORT_PREF, sortKey);
    render();
  },
});
sortSel.value = sortKey;

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
