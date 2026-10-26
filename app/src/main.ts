import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import * as stationsPage from "./stations-page";
import type { StationInfo } from "./stations-page";
import * as settingsPage from "./settings-page";
import * as lyricEditor from "./lyric-editor";
import type { Lyrics } from "./lyric-editor";
import * as lyricPicker from "./lyric-picker";
import * as lyricFlags from "./lyric-flags";
import * as lyricConfirm from "./lyric-confirm";
import { publicationFor } from "./lyric-publish";
import type { Row } from "./flags";
import { parseLrc, type LyricLine } from "./lrc";
import { detectDual, type Dual } from "./dual-lyrics";
import { fillMarked, fillWords } from "./draft-marks";
import { wordTimesFor } from "./edit-lines";
import { RECENTS_ORDER_CHANGED, recentsOrder } from "./recents";
import * as stationStats from "./station-stats";

// ---- types -------------------------------------------------------------
interface NowPlaying {
  title: string;
  artist: string;
  album: string;
  station: string;
  /// On QuickMix, which contributing station this track came from. Empty on an ordinary station.
  sourceStation: string;
  art: string;
  artFallback: string;
  thumbUp: boolean;
  thumbDown: boolean;
}
interface Playhead {
  position: number;
  duration: number;
  paused: boolean;
  volume: number;
}
interface RemoteState {
  device: string;
  playing: boolean;
  title: string;
  artist: string;
  album: string;
  art: string;
  position: number;
  duration: number;
  volume: number;
  /// The device can start a Pandora station itself (a WiiM whose PlayQueue service was found).
  canCast: boolean;
  /// The Pandora station the device is playing, and its token (= Jarlid's tuner token). Empty
  /// when it is playing something else.
  station: string;
  stationToken: string;
}

// ---- element helpers ---------------------------------------------------
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const player = $("player");
const loginHint = $("login-hint");
const bg = $("bg");
const artEl = $<HTMLImageElement>("art");
const titleEl = $("title");
const titleInner = $("title-inner");
const artistEl = $("artist");
const albumEl = $("album");
const bylineEl = document.querySelector(".byline") as HTMLElement;
const bylineInner = $("byline-inner");
const stationBtn = $("station");
// The station name is the button's label; it falls back to "Stations" (set in index.html)
// until a track names one, so the button is never blank.
const stationNameEl = $("station-name");
const sourceEl = $("source-station");
const histEl = $("history");
const barEl = $("bar");
const tCur = $("t-cur");
const tDur = $("t-dur");
const playIcon = $("play-icon");
const pauseIcon = $("pause-icon");
const thumbUpBtn = $("thumbUp");
const thumbDownBtn = $("thumbDown");
const lyricsEl = $("lyrics");
const lyricsStatus = $("lyrics-status");
const lyricsEditBtn = $("lyrics-edit");
const lyricsVersionsBtn = $("lyrics-versions");
const lyricsFlagBtn = $("lyrics-flag");
const lyricsPublishBtn = $<HTMLButtonElement>("lyrics-publish");

const fmt = (s: number) => {
  if (!isFinite(s) || s < 0) s = 0;
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
};

// ---- state -------------------------------------------------------------
let currentKey = "";
let syncedLines: LyricLine[] | null = null;
// What the lyrics pane is currently showing, and the track naming it was fetched under —
// both needed to edit, and to file the edit against the right track.
let lastLyrics: Lyrics | null = null;
let lastMeta = { title: "", artist: "", album: "" };
// The track length the lookup was made with, for weighing other versions against it.
let lastDuration: number | null = null;
/// The copy painted while autosync is still working. It hasn't passed the quality gate, so it
/// is never offered for publishing; the finished result arrives as a fresh `fetch_lyrics`.
let autoSyncPreview: Lyrics | null = null;
/// Tracks (`title|artist`) whose timings went to LRCLIB this session, so the chip doesn't
/// come back if LRCLIB is slow to serve them.
const publishedTracks = new Set<string>();
let publishing = false;
// A lookup that failed to reach LRCLIB is tried again while the song is still playing:
// after these pauses, in order, and then no more. The backend already retries each
// request a few times, so this covers an outage of a minute or so, not a long one —
// LRCLIB is a volunteer-run service and a song is only a few minutes long anyway.
const LYRICS_RETRY_DELAYS_MS = [20_000, 60_000];
let lyricsRetryTimer: ReturnType<typeof setTimeout> | null = null;
// remote (network player) mode
let remote: RemoteState | null = null;
let remoteAt = 0; // Date.now() when the last remote state arrived
let remoteMode = false;
let lastLocalPlayingAt = 0;
let lastLocalNp: NowPlaying | null = null;
/// `activeLineIdx` before anything has been highlighted. It cannot be -1, because -1 is
/// a real state now — "playing, but the first line hasn't come round yet" — and the
/// highlighter skips work when the index is unchanged.
const NO_LINE = -2;
let activeLineIdx = NO_LINE;
let lastPlayhead: Playhead = { position: 0, duration: 0, paused: true, volume: 1 };

// Songs rarely start singing at 0:00, and until the first timestamp there is no line to
// highlight — so the pane had nothing to scroll to, sat at the top, and then jumped when
// the first line finally landed. This synthetic row is the scroll target for that gap,
// and it doubles as a countdown so the wait doesn't read as "the lyrics didn't load".
// It is deliberately NOT a `.line`: highlightLine indexes `.line` nodes positionally.
const INTRO_MIN = 2;
let introEl: HTMLElement | null = null;
let introBar: HTMLElement | null = null;

// ---- one song into the next --------------------------------------------------
// Wiping the pane for each new track sent it back to its top: a rewind, in a pane that
// otherwise only ever moves down. Instead the last track's rows stay where they are, a
// break naming the new track goes under them, and the new rows go under that, so the
// scroll to the new first line carries on downward. The break is exactly as tall as the
// pane's top padding, so once the old rows and the break have scrolled out of sight they
// can be dropped, and the scroll moved back by their height, without anything on screen
// moving: what's left is the layout the new track would have had on its own.
/// `title|artist` of the track whose rows the pane holds; "" when there is nothing to
/// carry on from (first paint, or the editor's rows).
let paneSong = "";

/// Empty the pane for `meta`'s rows. Returns whether the last track's rows were kept above.
function clearPane(meta: { title: string; artist: string }): boolean {
  const song = `${meta.title}|${meta.artist}`;
  // A carried-over block still on screen is left alone: it is a track further back.
  const rows = [...lyricsEl.children].filter((c) => !c.classList.contains("lyric-outgoing"));
  const carry = !!paneSong && song !== paneSong && !lyricsEl.hidden && rows.length > 0;
  paneSong = song;
  if (!carry) {
    rows.forEach((c) => c.remove());
    return false;
  }
  const out = document.createElement("div");
  out.className = "lyric-outgoing";
  out.setAttribute("aria-hidden", "true");
  for (const row of rows) {
    // No longer a row of this track: the highlighter and the flags address rows by index.
    row.removeAttribute("data-idx");
    row.classList.remove("active");
    out.appendChild(row);
  }
  const brk = document.createElement("div");
  brk.className = "lyric-break";
  const label = document.createElement("span");
  label.textContent = meta.artist ? `${meta.title} · ${meta.artist}` : meta.title;
  brk.appendChild(label);
  out.appendChild(brk);
  lyricsEl.appendChild(out);
  return true;
}

/// Where the pane's own rows begin: below any carried-over blocks. Scrolled to here, the
/// pane looks as it would with them gone.
const songTop = () =>
  [...lyricsEl.querySelectorAll<HTMLElement>(":scope > .lyric-outgoing")].reduce((h, o) => h + o.offsetHeight, 0);

/// Drop carried-over blocks that have scrolled out of sight above, keeping the view still.
function dropOutgoing() {
  const top = lyricsEl.getBoundingClientRect().top;
  const gone = [...lyricsEl.querySelectorAll<HTMLElement>(":scope > .lyric-outgoing")].filter(
    (o) => o.getBoundingClientRect().bottom <= top
  );
  if (gone.length === 0) return;
  const h = gone.reduce((sum, o) => sum + o.offsetHeight, 0);
  // Read before removing: the shorter pane clamps scrollTop the moment they go.
  const was = lyricsEl.scrollTop;
  gone.forEach((o) => o.remove());
  lyricsEl.scrollTo({ top: was - h, behavior: "instant" });
  // The plain-lyrics glide works in the same coordinates.
  plainPos = Math.max(0, plainPos - h);
  if (plainTarget >= 0) plainTarget = Math.max(0, plainTarget - h);
}
// After the scroll has settled, so a smooth scroll in progress is not cut short.
lyricsEl.addEventListener("scrollend", dropOutgoing);

/// A one-line message in place of rows. Nothing drives the scroll for it, so a carry-over
/// from the last track scrolls down to it here.
function paintEmpty(text: string, meta: { title: string; artist: string }) {
  const carried = clearPane(meta);
  const div = document.createElement("div");
  div.className = "line empty";
  div.textContent = text;
  lyricsEl.appendChild(div);
  if (carried) lyricsEl.scrollTo({ top: songTop(), behavior: "smooth" });
}

/// `doubt` is set when the lines are a draft: per line, the words the speech engine was unsure
/// of. Draft rows are styled as such, row by row, so a draft's look never carries over to the
/// next song's rows or back to the last one's.
function renderSyncedLyrics(
  lines: LyricLine[],
  meta: { title: string; artist: string },
  doubt?: number[][],
  wordTimes?: (number[] | undefined)[]
) {
  clearPane(meta);
  stopPlainFollow();
  lyricsEl.classList.add("synced");
  introEl = introBar = null;

  if ((lines[0]?.t ?? 0) >= INTRO_MIN) {
    introEl = document.createElement("div");
    introEl.className = "lyric-intro";
    introBar = document.createElement("i");
    introEl.appendChild(introBar);
    lyricsEl.appendChild(introEl);
  }

  paneLines = lines.map((ln, i) => ({
    ...ln,
    ...(doubt ? { doubt: doubt[i] } : {}),
    ...(wordTimes?.[i] ? { wordTimes: wordTimes[i] } : {}),
  }));
  paneLines.forEach((ln, i) => {
    const div = document.createElement("div");
    div.className = doubt ? "line draft" : "line";
    div.dataset.idx = String(i);
    fillLine(div, ln);
    lyricsEl.appendChild(div);
  });
}

function renderPlainLyrics(lines: PaneLine[], meta: { title: string; artist: string }) {
  lyricsEl.classList.remove("synced");
  const carried = clearPane(meta);
  introEl = introBar = null;
  paneLines = lines;
  lines.forEach((ln, i) => {
    const div = document.createElement("div");
    div.className = "line plain";
    // Not for the highlighter, which leaves plain rows alone: flags point at rows by index.
    div.dataset.idx = String(i);
    fillLine(div, ln);
    lyricsEl.appendChild(div);
  });
  // A new song starts at its first verse, wherever the last one's pane was left. Carried on
  // from the last track, the follow glides down to it instead.
  stopPlainFollow();
  if (!carried) lyricsEl.scrollTo({ top: 0, behavior: "instant" });
}

// ---- two languages in one file ----------------------------------------------
// Some records carry every line in two languages (see dual-lyrics.ts). Each row then shows
// both, the second smaller and dimmer beneath the first, and a chip in the pane's head names
// the two and cycles: both, swapped, the first alone, the second alone. The choice is per
// track, like the sync offset.
type PaneLine = { text: string; alt?: string; doubt?: number[]; wordTimes?: number[] };
type DualMode = "both" | "swap" | "first" | "second";
const DUAL_MODES: DualMode[] = ["both", "swap", "first", "second"];
const lyricsDualBtn = $<HTMLButtonElement>("lyrics-dual");
/// The languages of what's in the pane, when it holds two.
let dualLabels: Dual["labels"] | null = null;
let dualMode: DualMode = "both";
/// The rows in the pane, so a mode change can redraw them in place.
let paneLines: PaneLine[] = [];

function fillLine(div: HTMLElement, ln: PaneLine) {
  div.classList.toggle("dual", false);
  if (!ln.alt || dualMode === "first") {
    if (ln.wordTimes) fillWords(div, ln.text, ln.doubt ?? []);
    else fillMarked(div, ln.text || " ", ln.doubt ?? []);
  } else if (dualMode === "second") {
    div.textContent = ln.alt;
  } else {
    const [main, sub] = dualMode === "swap" ? [ln.alt, ln.text] : [ln.text, ln.alt];
    const a = document.createElement("span");
    a.className = "l-main";
    a.textContent = main;
    const b = document.createElement("span");
    b.className = "l-alt";
    b.textContent = sub;
    div.replaceChildren(a, b);
    div.classList.add("dual");
  }
}

/// Run detection on lyrics about to be painted, and set the chip up for the result.
function noteDual(dual: Dual | null) {
  dualLabels = dual?.labels ?? null;
  const saved = localStorage.getItem(`dual:${currentKey}`) as DualMode | null;
  dualMode = saved && DUAL_MODES.includes(saved) ? saved : "both";
  lyricsDualBtn.hidden = !dual || lyricEditor.isEditing();
  refreshDualChip();
}

const langName = (i: 0 | 1) => dualLabels?.[i] ?? (i ? "2nd language" : "1st language");

function refreshDualChip() {
  if (!dualLabels) return;
  const [a, b] = [langName(0), langName(1)];
  const both = dualLabels[0] && dualLabels[1];
  lyricsDualBtn.textContent = {
    both: both ? `${a} + ${b}` : "2 languages",
    swap: both ? `${b} + ${a}` : "2 languages, swapped",
    first: `${a} only`,
    second: `${b} only`,
  }[dualMode];
  lyricsDualBtn.classList.toggle("on", dualMode !== "both");
}

lyricsDualBtn.addEventListener("click", () => {
  dualMode = DUAL_MODES[(DUAL_MODES.indexOf(dualMode) + 1) % DUAL_MODES.length];
  if (dualMode === "both") localStorage.removeItem(`dual:${currentKey}`);
  else localStorage.setItem(`dual:${currentKey}`, dualMode);
  refreshDualChip();
  // Redraw in place: the rows, their indexes and the highlight all stay put.
  lyricsEl.querySelectorAll<HTMLElement>(":scope > .line").forEach((div, i) => {
    const ln = paneLines[i];
    if (ln) fillLine(div, ln);
  });
  // Rows change height; keep the playing one centred.
  lyricsEl.querySelector<HTMLElement>(":scope > .line.active")?.scrollIntoView({ block: "center", behavior: "smooth" });
});
attachTip(lyricsDualBtn, () => {
  const [a, b] = [langName(0), langName(1)];
  const next = {
    both: `Click to put ${b} on top`,
    swap: `Click to show only ${a}`,
    first: `Click to show only ${b}`,
    second: "Click to show both languages",
  }[dualMode];
  return `These lyrics have every line in two languages. ${next}.`;
});

// ---- unsynced lyrics: follow the song by elapsed fraction -----------------
// Plain lyrics carry no timestamps, so no line can be lit — but the song still moves,
// and a pane parked at the top is wrong for every verse but the first. The estimate:
// the words are spread evenly over the singing, and the singing spans the track less
// a lead-in and a tail, because vocals rarely start at 0:00 or run to the last second.
// The point of the text at that fraction is held at the pane's centre, so the line
// actually being sung is somewhere on screen even when the guess is a verse out.
const PLAIN_LEAD_S = 12;
const PLAIN_TAIL_S = 15;
const PLAIN_EDGE_FRAC = 0.06; // short tracks: lead/tail are capped at this share each
// The reader wins: a wheel or a finger on the pane parks the follow, and it picks up
// again from wherever they left it once they have been still for this long.
const PLAIN_HOLD_MS = 4000;
let plainTarget = -1; // scrollTop the pane is gliding toward; -1 = not following
let plainPos = 0; // our own fractional copy of scrollTop, since the DOM rounds it
let plainRaf = 0;
let plainUserAt = 0;

function stopPlainFollow() {
  plainTarget = -1;
  plainPos = 0;
  if (plainRaf) cancelAnimationFrame(plainRaf);
  plainRaf = 0;
}

function plainFraction(position: number, duration: number): number {
  if (!(duration > 0)) return 0;
  const lead = Math.min(PLAIN_LEAD_S, duration * PLAIN_EDGE_FRAC);
  const tail = Math.min(PLAIN_TAIL_S, duration * PLAIN_EDGE_FRAC);
  const span = duration - lead - tail;
  const f = span > 0 ? (position - lead) / span : position / duration;
  return Math.max(0, Math.min(1, f));
}

function followPlainLyrics(position: number, duration: number) {
  if (lyricEditor.isEditing()) return;
  if (Date.now() - plainUserAt < PLAIN_HOLD_MS) return;
  const lines = lyricsEl.querySelectorAll<HTMLElement>(":scope > .line.plain");
  if (lines.length === 0) return;
  const paneTop = lyricsEl.getBoundingClientRect().top;
  const first = lines[0].getBoundingClientRect().top - paneTop + lyricsEl.scrollTop;
  const last = lines[lines.length - 1].getBoundingClientRect().bottom - paneTop + lyricsEl.scrollTop;
  const want = first + plainFraction(position, duration) * (last - first) - lyricsEl.clientHeight / 2;
  const max = Math.max(0, lyricsEl.scrollHeight - lyricsEl.clientHeight);
  // Never back up into a carried-over track: this song begins below it.
  plainTarget = Math.max(songTop(), Math.min(max, want));
  if (!plainRaf) {
    // The glide was idle, so the pane's real position is the truth — it may have been
    // scrolled by hand while we held off.
    plainPos = lyricsEl.scrollTop;
    plainRaf = requestAnimationFrame(glidePlain);
  }
}

// The playhead ticks a few times a second and each tick moves the target by a couple
// of pixels; stepping there would be visible. Ease toward it per frame instead, from a
// fractional position of our own, so the motion stays continuous however the DOM rounds.
function glidePlain() {
  plainRaf = 0;
  if (plainTarget < 0) return;
  const d = plainTarget - plainPos;
  if (Math.abs(d) < 0.05) {
    plainPos = plainTarget;
    lyricsEl.scrollTo({ top: plainPos, behavior: "instant" });
    return;
  }
  plainPos += d * 0.1;
  lyricsEl.scrollTo({ top: plainPos, behavior: "instant" });
  plainRaf = requestAnimationFrame(glidePlain);
}

for (const ev of ["wheel", "touchstart", "pointerdown"] as const) {
  lyricsEl.addEventListener(
    ev,
    () => {
      if (plainTarget < 0) return;
      plainUserAt = Date.now();
      if (plainRaf) cancelAnimationFrame(plainRaf);
      plainRaf = 0;
    },
    { passive: true }
  );
}

/// Where the song is, by whichever source is on screen: the local playhead or the speaker's.
let lyricsPosition = 0;

/// One entry point for the playhead: synced lyrics light a line, plain ones drift.
function followLyrics(position: number, duration: number) {
  lyricsPosition = position;
  if (syncedLines) highlightLine(position);
  else followPlainLyrics(position, duration);
}

// Manual lyric sync offset (seconds), per-track, persisted. Nudge with [ and ].
let syncOffset = 0;

// ---- word by word -----------------------------------------------------------------------
// Where words are timed (a Lyricsfile from LRCLIB, a draft, or an edit timed on the
// timeline), the playing line lights each word as it is sung. Playhead updates come a few
// times a second, too coarse for words, so between them the position is carried forward by
// the clock, every frame, while the line has words to light.

let playheadAt = performance.now();
let wordFrame = 0;

function startWordFollow() {
  if (!wordFrame) wordFrame = requestAnimationFrame(followWords);
}

function followWords() {
  wordFrame = 0;
  const line = activeLineIdx >= 0 ? paneLines[activeLineIdx] : undefined;
  if (!paneLines.some((l) => l.wordTimes)) return;
  if (line?.wordTimes && !lyricEditor.isEditing()) {
    const ahead = lastPlayhead.paused ? 0 : Math.min(0.5, (performance.now() - playheadAt) / 1000);
    const p = lastPlayhead.position + ahead + syncOffset;
    const row = lyricsEl.querySelector(`:scope > .line[data-idx="${activeLineIdx}"]`);
    row?.querySelectorAll<HTMLElement>(".w").forEach((w, k) => {
      w.classList.toggle("sung", line.wordTimes![k] <= p + 0.05);
    });
  }
  wordFrame = requestAnimationFrame(followWords);
}

function highlightLine(position: number) {
  // Edit mode owns the pane: rows may have no timestamp yet, so the ascending-order
  // assumption below does not hold, and the editor drives its own cursor instead.
  if (lyricEditor.isEditing()) return;
  if (!syncedLines || syncedLines.length === 0) return;
  const p = position + syncOffset;
  let idx = -1;
  for (let i = 0; i < syncedLines.length; i++) {
    if (syncedLines[i].t <= p + 0.15) idx = i;
    else break;
  }

  // The countdown has to move on every tick, so it is updated before the
  // nothing-changed shortcut below.
  if (introEl && introBar) {
    introEl.classList.toggle("past", idx >= 0);
    const until = syncedLines[0].t;
    introBar.style.transform = `scaleX(${Math.max(0, Math.min(1, p / until))})`;
  }

  if (idx === activeLineIdx) return;
  activeLineIdx = idx;
  const nodes = lyricsEl.querySelectorAll<HTMLElement>(":scope > .line");
  nodes.forEach((n) => {
    const i = Number(n.dataset.idx);
    n.classList.toggle("past", i < idx);
    n.classList.toggle("active", i === idx);
  });
  // Before the first line, the countdown row is what the pane centres on — otherwise
  // there is no target and the first line arrives with a jump.
  const active = idx < 0 ? introEl : nodes[idx];
  // In flag mode the listener may be reaching for a row; don't scroll it out from under them.
  if (active && !lyricFlags.holdScroll()) active.scrollIntoView({ block: "center", behavior: "smooth" });
}

// ---- now-playing -------------------------------------------------------
async function onNowPlaying(np: NowPlaying) {
  lastLocalNp = np;
  if (remoteMode) return; // remote overlay owns the screen; re-rendered on exit
  const wasSignedOut = isSignedOut();
  loginHint.hidden = true;
  showPlayer();
  // Signing in changes what the update badge should say, and does it without any playhead
  // event to notice it by. Only on the transition — this runs on every track.
  if (wasSignedOut) renderVersion();

  titleInner.textContent = np.title || "—";
  artistEl.textContent = np.artist || "";
  albumEl.textContent = np.album || "";
  // Measure once they have been laid out: sets the edge fades and restarts the drift
  // from the first word (see the marquee below).
  requestAnimationFrame(measureTitle);
  requestAnimationFrame(measureByline);
  if (np.station) stationNameEl.textContent = np.station;
  // QuickMix blends many stations; without this there's no way to tell which one is playing.
  sourceEl.textContent = np.sourceStation ? `from ${np.sourceStation}` : "";
  sourceEl.hidden = !np.sourceStation;
  setThumbs(np.thumbUp, np.thumbDown);

  setArt(np.art || np.artFallback, np.artFallback);

  // Key on title+artist only. Album flickers empty when Pandora collapses the
  // now-playing view during a long pause; including it would churn the key and
  // wipe the lyrics until the next song.
  const key = `${np.title}|${np.artist}`;
  if (key !== currentKey) {
    currentKey = key;
    syncedLines = null;
    activeLineIdx = NO_LINE;
    syncOffset = parseFloat(localStorage.getItem(`syncoff:${key}`) || "0") || 0;
    // An editor left open is still editing the previous track — which is right — but
    // the playhead it was timing against now belongs to this one.
    lyricEditor.notePlaybackMoved();
    // The lyrics on screen stay until the new track's arrive, but they are the last
    // track's: an edit or a version pick started now would be timed against this track's
    // playhead and filed under that one. `applyLyrics` brings the buttons back. The pencil
    // stays while editing — it is how the open editor is closed.
    if (!lyricEditor.isEditing()) lyricsEditBtn.hidden = true;
    lyricsVersionsBtn.hidden = true;
    refreshPublishChip();
    lyricFlags.trackChanged();
    pushHistory(np);
    await loadLyrics(np);
  }
}

// Wait briefly for the NEW track's duration before looking up lyrics — the
// fetch fires on title change, before the playhead reflects the new track,
// and without a duration LRCLIB version-matching can pick the wrong edit.
async function waitForDuration(key: string): Promise<number | null> {
  const t0 = Date.now();
  while (Date.now() - t0 < 2500) {
    if (currentKey !== key) return null; // track changed under us
    const d = lastPlayhead.duration;
    if (d > 0 && lastPlayhead.position < d && lastPlayhead.position < 30) return d;
    await new Promise((r) => setTimeout(r, 200));
  }
  return lastPlayhead.duration > 0 ? lastPlayhead.duration : null;
}

// ---- recently-played gallery -------------------------------------------
interface HistItem {
  art: string;
  title: string;
  artist: string;
  album?: string;
  at?: number;
}
let history: HistItem[] = [];
try {
  history = JSON.parse(localStorage.getItem("history") || "[]");
} catch {
  history = [];
}
const histModal = $("hist-modal");
const hmArt = $<HTMLImageElement>("hm-art");
const hmTitle = $("hm-title");
const hmArtist = $("hm-artist");
const hmAlbum = $("hm-album");
const hmWhen = $("hm-when");

function openHistModal(h: HistItem) {
  hmArt.src = h.art;
  hmTitle.textContent = h.title;
  hmArtist.textContent = h.artist;
  hmAlbum.textContent = h.album || "";
  hmWhen.textContent = h.at ? `Played ${new Date(h.at).toLocaleString()}` : "";
  histModal.hidden = false;
}
histModal.addEventListener("click", (e) => {
  if (!(e.target as HTMLElement).closest(".hm-card")) histModal.hidden = true;
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") histModal.hidden = true;
});

// Custom tooltip (no native title= tooltips anywhere in this app).
const tooltip = $("tooltip");
function attachTip(el: HTMLElement, text: () => string) {
  el.addEventListener("mouseenter", () => {
    // An empty string means this element has nothing worth saying right now.
    const t = text();
    if (!t) return;
    tooltip.textContent = t;
    tooltip.hidden = false;
    const r = el.getBoundingClientRect();
    const x = Math.max(8, Math.min(r.left + r.width / 2 - tooltip.offsetWidth / 2, innerWidth - tooltip.offsetWidth - 8));
    tooltip.style.left = `${x}px`;
    tooltip.style.top = `${Math.max(4, r.top - tooltip.offsetHeight - 10)}px`;
  });
  el.addEventListener("mouseleave", () => (tooltip.hidden = true));
}

/**
 * Draw the strip, newest end first.
 *
 * `history` is always newest-first; only the direction it is laid out in changes,
 * so the setting can be flipped any number of times without touching the list.
 *
 * The scroll position is part of the answer, not an afterthought. The strip is
 * wider than its column, so whichever end the newest track is at has to be the end
 * that is actually on screen — with the newest on the left that was `scrollLeft: 0`
 * and happened for free, which is exactly why it was never written down.
 */
function renderHistory() {
  histEl.innerHTML = "";
  const newestLeft = recentsOrder() === "newestLeft";
  const order = newestLeft ? history : [...history].reverse();
  for (const h of order) {
    const wrap = document.createElement("div");
    wrap.className = "hist-wrap";
    const img = new Image();
    img.src = h.art;
    img.className = "hist-item";
    img.loading = "lazy";
    img.addEventListener("click", () => openHistModal(h));
    attachTip(img, () => `${h.title} — ${h.artist}`);
    wrap.appendChild(img);
    histEl.appendChild(wrap);
  }
  showNewestEnd();
  requestAnimationFrame(coverflow);
}

/**
 * Park the strip at the end the newest track is on.
 *
 * The covers are a fixed 54px in CSS, so `scrollWidth` is right as soon as the strip
 * has been laid out at all — but at first render it has not been, because the whole
 * player is still `hidden` until a track arrives, and scrolling a zero-width element
 * does nothing and reports no error. That left the strip showing the *oldest* end on
 * startup: the first render's park was silently lost, and the next render only comes
 * with a *new* song, which never arrives when playback resumes on the track already
 * at the top of the list.
 *
 * Hence `showPlayer()` rather than a `ResizeObserver` on the strip: observing an
 * element that only gains a box because an ancestor stopped being `display: none`
 * never delivered a callback at all here, not even the initial one.
 */
function showNewestEnd() {
  if (histEl.clientWidth === 0) return;
  histEl.scrollLeft = recentsOrder() === "newestLeft" ? 0 : histEl.scrollWidth;
}

/**
 * Reveal the player, and put the recently-played strip the right way round.
 *
 * The strip can only be positioned once it has a width, and this is the moment it
 * gets one — so the two belong together rather than being two things a future
 * caller has to remember to do in order.
 */
function showPlayer() {
  player.hidden = false;
  showNewestEnd();
  // The top bar's room is the lyrics column, which has only just got a box.
  fitTopbar();
}

// Cover-Flow-style perspective: items tilt away from the strip's center as
// they approach the edges (the wrap gets the scroll-driven transform so the
// image's own hover zoom still works).
function coverflow() {
  const rect = histEl.getBoundingClientRect();
  if (rect.width === 0) return;
  const cx = rect.left + rect.width / 2;
  for (const el of Array.from(histEl.children) as HTMLElement[]) {
    const r = el.getBoundingClientRect();
    const d = Math.max(-1, Math.min(1, (r.left + r.width / 2 - cx) / (rect.width / 2)));
    el.style.transform = `perspective(420px) rotateY(${(-d * 32).toFixed(1)}deg) scale(${(
      1 - Math.abs(d) * 0.16
    ).toFixed(3)})`;
    el.style.opacity = (1 - Math.abs(d) * 0.45).toFixed(2);
    el.style.zIndex = String(100 - Math.round(Math.abs(d) * 50));
  }
}
histEl.addEventListener("scroll", () => requestAnimationFrame(coverflow));
window.addEventListener("resize", () => requestAnimationFrame(coverflow));
// Turning the strip round is a redraw, not a reload — the setting is judged by
// looking at the strip while the page is open.
window.addEventListener(RECENTS_ORDER_CHANGED, () => renderHistory());
function pushHistory(np: NowPlaying) {
  const art = np.artFallback || np.art;
  if (!art || !np.title) return;
  if (history[0] && history[0].title === np.title && history[0].artist === np.artist) return;
  // The same moment, counted against the station rather than the track: this is where the
  // Stations page's "recently played" and "most played" orders come from.
  stationStats.noteTrack(art);
  history.unshift({ art, title: np.title, artist: np.artist, album: np.album, at: Date.now() });
  history = history.slice(0, 40);
  localStorage.setItem("history", JSON.stringify(history));
  renderHistory();
}
renderHistory();
// Cap accumulated per-track sync offsets (no timestamps to age by; a rare
// full reset beats unbounded growth).
{
  const offKeys = Object.keys(localStorage).filter((k) => k.startsWith("syncoff:"));
  if (offKeys.length > 500) offKeys.forEach((k) => localStorage.removeItem(k));
}
// vertical wheel scrolls the strip horizontally
histEl.addEventListener(
  "wheel",
  (e) => {
    if (e.deltaY) {
      histEl.scrollLeft += e.deltaY;
      e.preventDefault();
    }
  },
  { passive: false }
);

// Preload art off-screen, then fade it in — avoids broken-image flashes and
// softens the stale-art-then-correct-art swap Pandora's DOM produces on load.
let artToken = 0;
function setArt(url: string, fallback: string) {
  if (!url || artEl.src === url) return;
  const token = ++artToken;
  const img = new Image();
  img.onload = () => {
    if (token !== artToken) return; // newer art superseded this one
    artEl.style.opacity = "0";
    setTimeout(() => {
      if (token !== artToken) return;
      artEl.src = url;
      bg.style.backgroundImage = `url("${url}")`;
      artEl.style.opacity = "1";
    }, 180);
  };
  img.onerror = () => {
    if (token === artToken && fallback && fallback !== url) setArt(fallback, "");
  };
  img.src = url;
}

function setThumbs(up: boolean, down: boolean) {
  thumbUpBtn.classList.toggle("active", !!up);
  thumbDownBtn.classList.toggle("active", !!down);
  thumbUpBtn.setAttribute("aria-pressed", up ? "true" : "false");
  thumbDownBtn.setAttribute("aria-pressed", down ? "true" : "false");
}

async function loadLyrics(np: NowPlaying) {
  const key = `${np.title}|${np.artist}`;
  // The versions on offer were the last track's.
  lyricPicker.close();
  lyricsStatus.textContent = "Loading lyrics…";
  // Keep any existing lyrics on screen until replacements arrive (no blank flash).
  const duration = await waitForDuration(key);
  if (currentKey !== key) return;
  await loadLyricsFor({ title: np.title, artist: np.artist, album: np.album }, duration, key);
}

async function loadLyricsFor(
  meta: { title: string; artist: string; album: string },
  duration: number | null,
  key: string,
  attempt = 0
) {
  // A retry queued for the previous track is moot now, whichever track this is for.
  if (lyricsRetryTimer !== null) {
    clearTimeout(lyricsRetryTimer);
    lyricsRetryTimer = null;
  }
  lyricPicker.close();
  lyricsStatus.textContent = attempt ? "Loading lyrics… (retrying)" : "Loading lyrics…";
  try {
    const res = await invoke<Lyrics>("fetch_lyrics", {
      artist: meta.artist,
      track: meta.title,
      album: meta.album || null,
      duration,
    });
    // Ignore if the track changed while we were fetching.
    if (key !== currentKey) return;

    lastMeta = meta;
    lastDuration = duration;
    applyLyrics(res);
    if (res.stale) void revalidateLyrics(meta, duration, key, res);
  } catch (e) {
    if (key !== currentKey) return;
    // The backend only fails when LRCLIB could not be reached (a miss is a result, not
    // an error), so this is a network problem worth logging and worth another go.
    console.warn("lyrics lookup failed", e);
    // Don't leave the pencil pointing at the last track's lyrics: `lastMeta` was not
    // updated, so editing now would file the edit against the wrong song.
    lastLyrics = null;
    lyricsEditBtn.hidden = true;
    lyricsVersionsBtn.hidden = true;
    refreshPublishChip();
    lyricFlags.trackChanged();
    noteDual(null);
    syncedLines = null;
    introEl = introBar = null;
    stopPlainFollow();
    paintEmpty("Lyrics unavailable", meta);
    const delay = LYRICS_RETRY_DELAYS_MS[attempt];
    if (delay === undefined) {
      lyricsStatus.textContent = "Lyrics";
      return;
    }
    lyricsStatus.textContent = "Lyrics · will retry";
    lyricsRetryTimer = setTimeout(() => {
      lyricsRetryTimer = null;
      if (key !== currentKey) return;
      void loadLyricsFor(meta, duration, key, attempt + 1);
    }, delay);
  }
}

/// A stale cached copy is already on screen; ask LRCLIB whether it still serves that, and
/// repaint only if not. Quietly gives up on failure — what's showing is still good.
async function revalidateLyrics(
  meta: { title: string; artist: string; album: string },
  duration: number | null,
  key: string,
  shown: Lyrics
) {
  let res: Lyrics;
  try {
    res = await invoke<Lyrics>("fetch_lyrics", {
      artist: meta.artist,
      track: meta.title,
      album: meta.album || null,
      duration,
      refresh: true,
    });
  } catch (e) {
    console.warn("lyrics refresh failed", e);
    return;
  }
  // The editor holds the record it would publish against; swapping it mid-edit would
  // aim a correction at the wrong record. The picker is comparing against what's shown.
  // The refreshed copy is cached for next time either way.
  if (key !== currentKey || lastLyrics !== shown) return;
  if (lyricEditor.isEditing() || lyricPicker.isOpen()) return;
  if (res.id === shown.id && res.synced === shown.synced && res.plain === shown.plain) {
    lastLyrics = res;
    return;
  }
  applyLyrics(res);
}

/// Paint a set of lyrics into the pane. Separate from fetching because the editor hands
/// back replacements that never went near the network.
function applyLyrics(res: Lyrics) {
  lastLyrics = res;
  // Even "nothing found" is worth an edit button — that is exactly when contributing
  // the words is most useful. The same goes for other versions: the wider search the
  // picker runs can turn up a record the automatic lookup missed.
  lyricsEditBtn.hidden = false;
  lyricsVersionsBtn.hidden = lyricEditor.isEditing();
  const note = res.overridden
    ? " · edited"
    : res.pinned
      ? " · your pick"
      : res.auto
        ? publishedTracks.has(currentKey)
          ? " · auto-synced · published"
          : " · auto-synced"
        : "";
  refreshPublishChip();

  // Edit mode owns the pane's contents. Repainting under it would throw away the rows
  // and the caret mid-edit, so keep the state current and leave the DOM alone.
  if (lyricEditor.isEditing()) {
    const parsed = res.synced ? parseLrc(res.synced) : null;
    syncedLines = parsed && (res.draft ? parsed : (detectDual(parsed)?.lines ?? parsed));
    lyricsStatus.textContent = res.draft
      ? draftStatus(res)
      : `${res.synced ? "Synced lyrics" : "Lyrics"}${note}`;
    return;
  }
  paintLyrics(res, note);
  showFlags(res);
}

/// Tell the flags which lyrics, and which rows, the track now shows. Not while editing: the
/// editor's rows aren't these, and leaving it repaints through here anyway.
function showFlags(res: Lyrics) {
  // A draft is reviewed line by line in the editor; flags are for lyrics someone published.
  const rows: Row[] = res.draft
    ? []
    : (syncedLines ?? (res.plain ? paneLines.map((l) => ({ t: null, ...l })) : []));
  lyricFlags.show(lastMeta, lastDuration ?? res.duration, res, rows);
}

/// Draw a set of lyrics in the pane and drive the highlighter from them. Doesn't adopt
/// them: the picker previews candidates through this without touching `lastLyrics`.
function paintLyrics(res: Lyrics, note: string) {
  if (res.synced) {
    const parsed = parseLrc(res.synced);
    // A draft is the engine's own words in one language: nothing to pair up.
    const dual = res.draft ? null : detectDual(parsed);
    noteDual(dual);
    syncedLines = dual?.lines ?? parsed;
    // Word by word where the lyrics are timed that finely (not for two languages at once).
    const wordTimes = res.timing && !dual ? wordTimesFor(syncedLines, res.timing) : undefined;
    renderSyncedLyrics(syncedLines, lastMeta, res.draft?.doubt, wordTimes);
    if (wordTimes?.some(Boolean)) startWordFollow();
    lyricsStatus.textContent = res.draft ? draftStatus(res) : `Synced lyrics${note}`;
    activeLineIdx = NO_LINE;
    highlightLine(lastPlayhead.position);
  } else if (res.plain) {
    syncedLines = null;
    const lines = res.plain.split(/\r?\n/).map((text) => ({ t: null, text }));
    const dual = detectDual(lines);
    noteDual(dual);
    renderPlainLyrics(dual?.lines ?? lines, lastMeta);
    lyricsStatus.textContent = `Lyrics${note}`;
  } else {
    syncedLines = null;
    introEl = introBar = null;
    stopPlainFollow();
    noteDual(null);
    paintEmpty("No lyrics found", lastMeta);
    lyricsStatus.textContent =
      res.noDraft === "noSinging"
        ? "Lyrics · no singing heard to draft"
        : res.noDraft === "unclear"
          ? "Lyrics · the singing was too unclear to draft"
          : "Lyrics";
  }
}

/// The status line for a draft: whose words they are, and how far the review has got. Only
/// lines with words count; a blank one has nothing to review. The review is indexed by the
/// file's lines as written, which is how the editor reads them.
function draftStatus(res: Lyrics) {
  const reviewed = res.draft?.reviewed ?? [];
  const worded = (res.synced ?? "")
    .split(/\r?\n/)
    .map((raw, i) => ({ words: !!raw.replace(/\[[^\]]*\]/g, "").trim(), ok: !!reviewed[i] }))
    .filter((l) => l.words);
  const done = worded.filter((l) => l.ok).length;
  return done
    ? `Draft lyrics · ${done} of ${worded.length} lines reviewed`
    : "Draft lyrics · heard by the speech engine, not yet reviewed";
}

lyricsVersionsBtn.addEventListener("click", () => {
  if (lyricPicker.isOpen()) {
    lyricPicker.close();
    return;
  }
  if (!lastLyrics || lyricEditor.isEditing()) return;
  const key = currentKey;
  lyricsVersionsBtn.classList.add("on");
  lyricFlags.close();
  void lyricPicker.open({
    meta: lastMeta,
    duration: lastDuration,
    current: lastLyrics,
    preview: (l) => paintLyrics(l, " · trying a version"),
    onChosen: applyLyrics,
    // Back to automatic: the cached copy was the chosen record, so look again.
    onReset: () => {
      if (key === currentKey) void loadLyricsFor(lastMeta, lastDuration, key);
    },
    onExit: () => {
      if (lastLyrics) applyLyrics(lastLyrics);
    },
    onClosed: () => {
      lyricsVersionsBtn.classList.remove("on");
      refreshPublishChip();
      lyricFlags.refresh();
    },
  });
  refreshPublishChip();
  lyricFlags.refresh();
});
attachTip(lyricsVersionsBtn, () =>
  lyricPicker.isOpen()
    ? "Close versions"
    : lastLyrics?.pinned
      ? "Other versions (showing the one you chose)"
      : "Wrong lyrics? See the other versions LRCLIB has"
);

lyricsEditBtn.addEventListener("click", () => {
  if (lyricEditor.isEditing()) {
    lyricEditor.end();
    return;
  }
  if (!lastLyrics) return;
  // Editing works on what the track shows, not on a version being tried.
  lyricPicker.close();
  lyricFlags.close();
  lyricsEditBtn.classList.add("on");
  lyricsVersionsBtn.hidden = true;
  // The editor shows each line as stored, both languages together; the chip means nothing there.
  lyricsDualBtn.hidden = true;
  const key = currentKey;
  // The editor's rows are its own, with controls in them: nothing to carry into the next track.
  paneSong = "";
  lyricEditor.begin({
    meta: lastMeta,
    lyrics: lastLyrics,
    duration: lastLyrics.duration ?? (lastDuration || lastPlayhead.duration || null),
    playhead: () => lastPlayhead,
    transport: (c) => void cmd(c),
    seek: (position) => void invoke("player_seek", { position }).catch(() => {}),
    canTransport: !remoteMode,
    // Once playback has moved on, the pane belongs to the new track. The edit is already
    // filed under its own track; adopting it here would show it on this one, and the next
    // edit or version pick would be made against the wrong song's lyrics.
    onApplied: (l) => {
      if (key === currentKey) applyLyrics(l);
    },
    // Repaint the pane the ordinary way and let the highlighter take over again.
    onExit: () => {
      lyricsEditBtn.classList.remove("on");
      activeLineIdx = NO_LINE;
      if (lastLyrics) applyLyrics(lastLyrics);
      // Opened on lines still being worked out: the finished ones may have landed meanwhile,
      // while the editor kept them off the pane.
      if (lastLyrics && lastLyrics === autoSyncPreview && key === currentKey) {
        void loadLyricsFor(lastMeta, lastDuration, key);
      }
      // Closed after a track change but before that track's lyrics arrived: what's
      // painted is still the last track's, so keep the buttons off it until they land.
      if (`${lastMeta.title}|${lastMeta.artist}` !== currentKey) {
        lyricsEditBtn.hidden = true;
        lyricsVersionsBtn.hidden = true;
        lyricFlags.trackChanged();
      }
    },
  });
  refreshPublishChip();
  lyricFlags.refresh();
});
attachTip(lyricsEditBtn, () =>
  lyricEditor.isEditing()
    ? "Stop editing"
    : lastLyrics?.overridden
      ? "Edit lyrics (showing your local edit)"
      : lastLyrics?.draft
        ? "Review this draft line by line, then publish it to LRCLIB"
        : lastLyrics?.auto
          ? "Check the auto-synced timings, and publish them to LRCLIB if they're right"
          : "Fix or time these lyrics"
);

/// How long the status line says what was just flagged.
const FLAG_NOTE_MS = 3000;
let flagNote: { was: string; text: string; timer: number } | null = null;

lyricFlags.init({
  currentKey: () => currentKey,
  now: () => lyricsPosition + syncOffset,
  offset: () => syncOffset,
  blocked: () => lyricEditor.isEditing() || lyricPicker.isOpen(),
  covered: () => stationsPage.isOpen() || settingsPage.isOpen() || !histModal.hidden,
  // A flag's description goes stale as soon as the flag grows or goes, unlike a sync nudge's
  // "offset +0.25s", so it gives the status line back once it has been read.
  flash: (text) => {
    // Several flags in quick succession all give back what was there before the first.
    if (!flagNote || lyricsStatus.textContent !== flagNote.text) {
      flagNote = { was: lyricsStatus.textContent ?? "", text, timer: 0 };
    }
    flagNote.text = text;
    flashStatus(text);
    clearTimeout(flagNote.timer);
    flagNote.timer = window.setTimeout(() => {
      if (flagNote && lyricsStatus.textContent === flagNote.text) {
        lyricsStatus.textContent = flagNote.was;
      }
      flagNote = null;
    }, FLAG_NOTE_MS);
  },
});
attachTip(lyricsFlagBtn, lyricFlags.tip);

// ---- auto-synced lyrics --------------------------------------------------

/// Progress from `autosync` in Rust: timings worked out on this machine for a song LRCLIB has
/// words for but no timings. `lrc` is the whole song so far (every line has a time), or null
/// while too little has been heard to trust; `final` ends it, `ok` saying whether the result
/// passed the quality gate.
interface AutoSyncLines {
  title: string;
  artist: string;
  lrc: string | null;
  /// Set when the lines are a draft (LRCLIB has no words for the song), with each line's
  /// doubtful words.
  draft: boolean;
  doubt: number[][] | null;
  done: number;
  of: number;
  final: boolean;
  ok: boolean;
}

void listen<AutoSyncLines>("autosync://lines", (e) => {
  const p = e.payload;
  const key = `${p.title}|${p.artist}`;
  if (key !== currentKey) return;
  // Only ever in place of plain lyrics, or of nothing at all: never over real timings or the
  // listener's own edit.
  const shown = lastLyrics;
  if (!shown || shown.overridden) return;
  const replaceable = shown.plain
    ? !shown.synced || shown.auto
    : !shown.synced || (!!shown.draft && shown === autoSyncPreview);
  if (!replaceable) return;
  // The editor and the version picker own the pane while they are open.
  if (lyricEditor.isEditing() || lyricPicker.isOpen()) return;
  if (p.final) {
    // Ask again: the stored result comes back marked `auto` or as a draft, or as it was if
    // there is none.
    void loadLyricsFor(lastMeta, lastDuration, key);
    return;
  }
  if (p.lrc) {
    autoSyncPreview = p.draft
      ? {
          ...shown,
          synced: p.lrc,
          source: "draft",
          draft: { doubt: p.doubt ?? [], reviewed: (p.doubt ?? []).map(() => false) },
        }
      : { ...shown, synced: p.lrc, auto: true };
    applyLyrics(autoSyncPreview);
  }
  lyricsStatus.textContent = p.draft
    ? `${p.lrc ? "Draft lyrics" : "Lyrics"} · drafting ${p.done} of ${p.of}`
    : `${p.lrc ? "Synced lyrics" : "Lyrics"} · syncing ${p.done} of ${p.of}`;
});

// ---- publishing checked auto-synced timings ------------------------------
// Generated timings are never published on their own (plans/autosync-quick-publish.md).
// Once the listener has watched them line up, this chip sends them to LRCLIB without a trip
// through the editor — after the same confirmation, under the same record identity.

/// What the chip would publish: exactly what the pane shows, filed under the matched record.
function autoPublication() {
  const l = lastLyrics;
  if (!l?.auto || l === autoSyncPreview || !l.id) return null;
  // The pane can still hold the last track's lyrics while the new track's load.
  if (`${lastMeta.title}|${lastMeta.artist}` !== currentKey) return null;
  return publicationFor(l, lastMeta, lastDuration, { synced: l.synced, plain: l.plain });
}

function refreshPublishChip() {
  lyricsPublishBtn.hidden =
    lyricEditor.isEditing() ||
    lyricPicker.isOpen() ||
    publishedTracks.has(currentKey) ||
    !autoPublication();
  lyricsPublishBtn.disabled = publishing;
  lyricsPublishBtn.textContent = publishing ? "Publishing…" : "Looks right? Publish";
}

lyricsPublishBtn.addEventListener("click", async () => {
  const shown = lastLyrics;
  const meta = lastMeta;
  const key = currentKey;
  const p = autoPublication();
  if (!p || publishing) return;
  const go = await lyricConfirm.ask({
    title: "Publish these timings to LRCLIB?",
    message: `They were worked out on this computer, not by a person, so publish them only if you've watched them line up with the song. They become the synced lyrics every LRCLIB app gets for ${p.artistName} — ${p.trackName}; the current version is kept as an earlier revision.`,
    go: "Publish",
  });
  // What was confirmed is `p`, captured before asking, so a track change while the dialog
  // was open doesn't change what gets published or where.
  if (go === null) return;
  publishing = true;
  refreshPublishChip();
  try {
    await invoke("publish_lyrics", { publication: p });
    publishedTracks.add(key);
    showToast(`Published the timings for ${p.artistName} — ${p.trackName} to LRCLIB.`);
    // Look again, so the pane shows what LRCLIB now serves — unless something else has
    // taken the pane in the meantime.
    if (key === currentKey && lastLyrics === shown && !lyricEditor.isEditing() && !lyricPicker.isOpen()) {
      await loadLyricsFor(meta, lastDuration, key);
    }
  } catch (e) {
    showToast(`Couldn't publish to LRCLIB. ${e}`);
  } finally {
    publishing = false;
    refreshPublishChip();
  }
});

// ---- playhead ----------------------------------------------------------
// Playing/paused is derived from whether the position is actually moving —
// Pandora's DOM and <audio> elements both misreport paused state, but a
// advancing playhead cannot lie.
let lastPos = -1;
let lastMoveAt = 0;
function onPlayhead(ph: Playhead) {
  const wasPaused = lastPlayhead.paused;
  lastPlayhead = ph;
  playheadAt = performance.now();
  // A staged update says "after this song" or "while paused" depending on this, so the
  // badge has to follow it.
  if (wasPaused !== ph.paused && status.armed) renderVersion();
  const now = Date.now();
  const moved = Math.abs(ph.position - lastPos) > 0.05;
  if (moved) {
    lastPos = ph.position;
    lastMoveAt = now;
    lastLocalPlayingAt = now; // local playback active — for the source toggle and mini player
  }
  if (remoteMode) return; // remote overlay owns progress/icon/highlight

  const pct = ph.duration > 0 ? (ph.position / ph.duration) * 100 : 0;
  barEl.style.width = `${Math.min(100, pct)}%`;
  tCur.textContent = fmt(ph.position);
  tDur.textContent = fmt(ph.duration);
  if (moved) {
    if (now >= optimisticUntil) setPlayingIcon(true);
  } else if (now - lastMoveAt > 1600 && now >= optimisticUntil) {
    setPlayingIcon(false);
  }
  followLyrics(ph.position, ph.duration);
}

// ---- controls ----------------------------------------------------------
const cmd = (c: string) => invoke("player_cmd", { cmd: c }).catch(() => {});
// The icons are SVG elements: the `.hidden` PROPERTY does not exist on
// SVGElement (it's HTMLElement-only), so it must be the attribute.
let uiPlaying = false;
function setPlayingIcon(playing: boolean) {
  uiPlaying = playing;
  if (playing) {
    playIcon.setAttribute("hidden", "");
    pauseIcon.removeAttribute("hidden");
  } else {
    pauseIcon.setAttribute("hidden", "");
    playIcon.removeAttribute("hidden");
  }
}

// After a click, in-flight playhead ticks still carry pre-toggle motion; hold
// the optimistic state briefly so the icon doesn't flicker before settling.
let optimisticUntil = 0;
function togglePlayback() {
  const desired = !uiPlaying;
  setPlayingIcon(desired);
  optimisticUntil = Date.now() + 2000;
  if (remoteMode) {
    invoke("remote_cmd", { cmd: desired ? "play" : "pause" }).catch(() => {});
  } else {
    cmd("toggle");
  }
}
$("play").addEventListener("click", togglePlayback);
// Space toggles playback, unless it is typing (the station search, a lyric line being
// edited) or something already took it. A timing pass marks lines with Space and claims
// the key with preventDefault; its listener runs first because lyric-editor.ts registers
// on import. Checking the flag rather than "is a pass running" matters on the last line:
// marking it ends the pass before this listener sees the key.
window.addEventListener("keydown", (e) => {
  if (e.defaultPrevented) return;
  const el = e.target as HTMLElement;
  if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable) return;
  if (e.code === "Space") {
    e.preventDefault();
    togglePlayback();
  }
});
// Technical readout, bottom-left, opposite the version badge. Every number here was already
// being measured somewhere in the pipeline and, until now, only ever reached a bug report.
interface Technical {
  /// Measured from the audio — Pandora sends no tempo. Null until enough has been heard.
  bpm: number | null;
  bpmConfidence: number | null;
  codec: string | null;
  bitrateKbps: number | null;
  sourceRate: number | null;
  channels: number | null;
  outputRate: number | null;
  buffered: number;
  drift: number;
  starved: number;
  /// Audio Windows holds between the output callback and the speaker. 0 before anything opens.
  deviceBufferMs: number;
  /// A new size the next player gets, after a grow (or shrink) and until the rebuild that
  /// applies it. Null otherwise.
  pendingDeviceBufferMs: number | null;
  /// Times this session the device buffer ran out: the crackle that makes it grow.
  underruns: number;
}
const techEl = $("tech");

/// Confidence below which the BPM is hedged with a tilde rather than stated flat.
///
/// Calibrated against real tracks rather than guessed: a locked-in beat measures 0.51-0.79
/// (house at 120.0 and 127.6, a busy rock track at 109.2), while genuinely ambiguous material
/// sits at 0.22-0.44 (an ambient piece, a long DJ mix, and the drumless intro of a house track
/// where the only pulse is the dotted two-thirds one). 0.45 falls in the gap between them.
const BPM_HEDGE = 0.45;

const kHz = (hz: number) => `${(hz / 1000).toFixed(hz % 1000 === 0 ? 0 : 1)} kHz`;

function renderTechnical(t: Technical) {
  const parts: string[] = [];

  // Always present, even before it is known: a slot that appears later would shove the rest of
  // the line sideways a few seconds into every song.
  if (t.bpm === null) {
    parts.push("♩ —");
  } else {
    const hedge = (t.bpmConfidence ?? 0) < BPM_HEDGE ? "~" : "";
    parts.push(`♩ ${hedge}${Math.round(t.bpm)}`);
  }

  if (t.codec) {
    parts.push(t.bitrateKbps ? `${t.codec} ${t.bitrateKbps}k` : t.codec);
  }

  // Both rates when they differ, because that gap is the resampling that keeps pitch honest;
  // one when they don't, because "48 → 48 kHz" is just noise.
  if (t.sourceRate && t.outputRate && t.sourceRate !== t.outputRate) {
    parts.push(`${kHz(t.sourceRate)} → ${kHz(t.outputRate)}`);
  } else if (t.outputRate || t.sourceRate) {
    parts.push(kHz(t.outputRate || t.sourceRate!));
  }

  // Only worth a word when it isn't the stereo everyone assumes.
  if (t.channels === 1) parts.push("mono");
  else if (t.channels && t.channels > 2) parts.push(`${t.channels} ch`);

  parts.push(`buf ${t.buffered.toFixed(1)} s`);

  // The device buffer, which is the one a busy machine actually breaks. It sits at 30 ms and
  // doubles while underruns recur, so its size is itself the load signal; the arrow is a grow
  // (or a shrink) that the next player will pick up.
  if (t.deviceBufferMs > 0) {
    const now = Math.round(t.deviceBufferMs);
    const next = t.pendingDeviceBufferMs === null ? null : Math.round(t.pendingDeviceBufferMs);
    parts.push(next === null ? `out ${now} ms` : `out ${now} → ${next} ms`);
  }

  // Faults are absent rather than zero. A permanent "drift 0.00 s" trains you to stop reading
  // the line, which defeats the point of having it.
  const faults: string[] = [];
  if (t.drift >= 0.05) faults.push(`drift ${t.drift.toFixed(2)} s`);
  if (t.starved >= 0.05) faults.push(`starved ${t.starved.toFixed(2)} s`);
  if (t.underruns > 0) faults.push(`underrun${t.underruns === 1 ? "" : "s"} ${t.underruns}`);

  techEl.textContent = parts.join(" · ");
  if (faults.length) {
    // Built as a node rather than innerHTML: none of this is user data today, but a codec name
    // does come from Media Foundation and this is the sort of line that quietly grows fields.
    const marker = document.createElement("span");
    marker.className = "fault";
    marker.textContent = ` · ${faults.join(" · ")}`;
    techEl.append(marker);
  }
}

// The version badge is the entire (unobtrusive) update UI: shows the running version,
// then what is about to happen to it. Clicking walks the known -> staged -> armed -> now
// ladder one step per click; how far the app walks on its own is the Settings policy.
const versionEl = $("version");
let baseVersion = "";
let versionBusy = false;
/// A debug build, where the updater is inert and the bar is a readout rather than a button.
let devBuild = false;

/// Identity of the tree this build came from — see `build_info.rs`.
interface BuildId {
  dev: boolean;
  /// Set whenever git could be read: null in a release, or outside a repo. Present even on
  /// the tag, because the click copies `full` regardless of what the bar says.
  git: {
    branch: string | null;
    hash: string;
    full: string;
    dirty: boolean;
    tagged: boolean;
  } | null;
}
/// The full sha to put on the clipboard, once a dev build has told us one.
let devHash: string | null = null;

type Policy = "instant" | "afterSong" | "manualInstall" | "notifyOnly";
interface UpdateStatus {
  available: string | null;
  staged: boolean;
  armed: boolean;
  policy: Policy;
  /// This version was handed to its installer once already and the app came back as the old
  /// one, so it waits to be asked rather than exiting the app at every song for nothing.
  failed: boolean;
}
let status: UpdateStatus = { available: null, staged: false, armed: false, policy: "afterSong", failed: false };

/// Whether the login card is up — which is exactly "there is no engine to ask what playback
/// is doing", the frontend's version of the updater's `Playback::Unknown`.
///
/// Read off the card rather than kept in a parallel flag: `showLogin` and `onNowPlaying`
/// already move it in lockstep with whether an engine exists, and a second copy of that
/// state is a second thing that can be wrong.
function isSignedOut(): boolean {
  return !loginHint.hidden;
}

function renderVersion() {
  const v = status.available;
  versionEl.classList.toggle("update", !!v);
  if (!v) {
    versionEl.textContent = baseVersion;
    return;
  }
  if (!status.staged) {
    // Known about but not downloaded (notify-only, or a download that hasn't run yet).
    versionEl.textContent = `v${v} available`;
    return;
  }
  if (!status.armed) {
    // Downloaded and waiting to be asked — or held back because it failed to install before.
    versionEl.textContent = status.failed ? `v${v} didn't install` : `v${v} ready to install`;
    return;
  }
  // Armed: say when. The three cases match the updater's own, and the third is the one this
  // used to get wrong. `lastPlayhead` starts out `paused: true` and no playhead events arrive
  // while signed out, so a login-card session read as "paused" and the badge promised a
  // silent install that the updater had no intention of performing — the same conflation of
  // "cannot tell" with "paused" that v1.4.1 fixed on the Rust side.
  if (isSignedOut()) {
    // Nothing to wait for: no song to finish, no pause to respect.
    versionEl.textContent = `updating to v${v} shortly`;
    return;
  }
  // Paused is the moment the updater prefers — it installs within about a minute and comes
  // back paused — so "after this song" would be a lie there.
  versionEl.textContent = lastPlayhead.paused
    ? `updating to v${v} while paused`
    : `updating to v${v} after this song`;
}

attachTip(versionEl, () => {
  // Updates are compiled out of a debug build, so there is no check to promise. Say what the
  // click does instead — and that it is the *full* sha, not the short one on screen.
  if (devBuild) {
    return devHash ? "Click to copy the full commit hash" : "Dev build — updates are disabled";
  }
  const v = status.available;
  if (!v) return "Click to check for updates";
  if (!status.staged) return `Click to download v${v}`;
  if (!status.armed) {
    const when = isSignedOut() ? "Click to install now" : "Click to install after this song";
    return status.failed
      ? `The last try at installing v${v} didn't work, so it won't try again on its own. ${when}, or download the installer from GitHub.`
      : when;
  }
  return "Click to install now instead of waiting";
});

// The version number identifies a *release* exactly — CI built it from the commit a `vX.Y.Z`
// tag points at. It identifies a dev build barely at all: the same number sits on a tree that
// may be several commits ahead, on a branch, with uncommitted edits on top. So a dev build
// off its tag says which commit is actually running, and a release is untouched.
//
// Folded into `baseVersion` rather than appended afterwards, because `renderVersion` rewrites
// the bar for every update state and the first status event would otherwise erase it.
void (async () => {
  const [v, build] = await Promise.all([
    getVersion().catch(() => null),
    invoke<BuildId>("build_id").catch((): BuildId => ({ dev: false, git: null })),
  ]);
  // Joined rather than concatenated so a failed `getVersion` degrades to the commit alone
  // instead of a bar that opens with a stray separator.
  const parts: string[] = [];
  if (v) parts.push(`v${v}`);
  if (build.git) devHash = build.git.full;
  // Name the commit only when the tree is not what the tag contains. Sitting clean on the
  // tag the number is the whole truth, and the extra text would be scenery.
  if (build.git && (!build.git.tagged || build.git.dirty)) {
    // Detached HEAD has no branch to name, so the hash stands alone. Uncommitted work gets a
    // star, because a hash on its own misrepresents a tree that has been edited since.
    //
    // The branch is clipped rather than the whole string: this bar is `position: fixed`
    // opposite the technical readout, and branch names here run to the length of
    // `t3code/layout-spacing-alignment-fixes`. A CSS ellipsis would trim the *end*, which is
    // exactly the hash — the one part that cannot be guessed from context.
    const b = build.git.branch;
    const short = b && b.length > 24 ? `${b.slice(0, 23)}…` : b;
    const where = short ? `${short}@${build.git.hash}` : build.git.hash;
    parts.push(`${where}${build.git.dirty ? "*" : ""}`);
  }
  baseVersion = parts.join(" · ");
  if (build.dev) {
    devBuild = true;
    // Still a button, just a different one: with no hash to hand over there is nothing behind
    // the click, so only then does it stop presenting as one.
    versionEl.classList.toggle("readonly", !devHash);
  }
  renderVersion();
})();

invoke<UpdateStatus>("update_status")
  .then((s) => {
    status = s;
    renderVersion();
  })
  .catch(() => {});

listen<UpdateStatus>("app://update-status", (e) => {
  status = e.payload;
  renderVersion();
});

// The last thing painted before the process is replaced, so it should explain the
// silence that follows.
listen<string>("app://update-installing", (e) => {
  versionBusy = true;
  versionEl.classList.add("update");
  versionEl.textContent = `updating to v${e.payload}…`;
});
listen<string>("app://update-failed", () => {
  versionBusy = false;
  versionEl.textContent = "update failed";
  setTimeout(renderVersion, 2500);
});
// Playback started or stopped in the moment between the notice and the install, so the
// updater backed out. Nothing went wrong and it will try again — just undo the notice.
listen("app://update-stood-down", () => {
  versionBusy = false;
  renderVersion();
});

/// Put text on the clipboard, without taking a dependency for it.
///
/// `navigator.clipboard` is the right call and should work — Tauri serves the app from
/// `tauri.localhost`, which Chromium treats as a secure context, and a click is the user
/// activation it wants. The `execCommand` path is the old synchronous fallback for anywhere
/// that turns out not to be true; it is deprecated rather than gone, and this is dev-only.
async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.setAttribute("readonly", "");
      // Off-screen rather than hidden: `display:none` cannot be selected.
      ta.style.cssText = "position:fixed;top:-9999px;opacity:0";
      document.body.appendChild(ta);
      ta.select();
      const ok = document.execCommand("copy");
      ta.remove();
      return ok;
    } catch {
      return false;
    }
  }
}

versionEl.addEventListener("click", async () => {
  // A dev build never installs anything — the loop is compiled out and `update_action`
  // refuses — so the click is free to do the thing you actually want at a version bar you
  // are staring at while debugging: hand you the commit.
  if (devBuild) {
    if (versionBusy || !devHash) return;
    versionBusy = true;
    const ok = await copyText(devHash);
    versionEl.textContent = ok ? "commit copied" : "copy failed";
    setTimeout(() => {
      versionBusy = false;
      renderVersion();
    }, 1400);
    return;
  }
  if (versionBusy) return;
  versionBusy = true;
  const wasKnown = !!status.available;
  versionEl.textContent = status.staged ? "installing…" : "checking…";
  try {
    status = await invoke<UpdateStatus>("update_action");
    versionBusy = false;
    if (!status.available && !wasKnown) {
      versionEl.textContent = "up to date";
      setTimeout(renderVersion, 2200);
    } else {
      renderVersion();
    }
  } catch {
    versionBusy = false;
    versionEl.textContent = "check failed";
    setTimeout(renderVersion, 2200);
  }
});
$("skip").addEventListener("click", () =>
  remoteMode ? invoke("remote_cmd", { cmd: "skip" }).catch(() => {}) : cmd("skip")
);
$("replay").addEventListener("click", () => cmd("replay"));
thumbUpBtn.addEventListener("click", () => cmd("thumbUp"));
thumbDownBtn.addEventListener("click", () => cmd("thumbDown"));
// Sign in directly — there is no Pandora webview to log into any more. The password goes
// straight to the Rust side, which only persists it once Pandora has actually accepted it.
$("login-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const user = $("login-user") as HTMLInputElement;
  const pass = $("login-pass") as HTMLInputElement;
  const submit = $("login-submit") as HTMLButtonElement;
  const error = $("login-error");

  error.hidden = true;
  submit.disabled = true;
  submit.textContent = "Signing in…";

  try {
    await invoke("native_sign_in", { username: user.value, password: pass.value });
    // Don't leave the password sitting in the DOM once it has been handed over.
    pass.value = "";
    loginHint.hidden = true;
  } catch (err) {
    error.textContent = String(err);
    error.hidden = false;
  } finally {
    submit.disabled = false;
    submit.textContent = "Sign in";
  }
});
// ---- station switching ---------------------------------------------------
// The station name in the top-right controls *is* the picker's active row, so clicking it
// opens the Stations panel and flies the name into place rather than dropping a second,
// poorer list next to the first. A station is identified by its tuner token: the name is
// not unique.
//
// The list itself lives on the Stations page now — this module keeps only the name it paints
// into the pill.
let activeStation = "";
/// Its token, kept so the Stations page can be pointed back at it when remote mode ends.
let activeToken = "";

/// The centre of an element's *text*, not of its box.
///
/// Both ends of the flight have a box much wider than the words inside it: the station
/// button stretches across the meta block, and a row's name spans the whole text column
/// while its words sit at the left of it. Flying between box centres would start and end the
/// name where it visibly is not, so a Range measures the glyphs instead.
function textCentre(el: Element) {
  const range = document.createRange();
  range.selectNodeContents(el);
  const r = range.getBoundingClientRect();
  return r.width || r.height
    ? { x: r.left + r.width / 2, y: r.top + r.height / 2 }
    : (() => {
        const b = el.getBoundingClientRect();
        return { x: b.left + b.width / 2, y: b.top + b.height / 2 };
      })();
}

const FLIGHT_MS = 460;

/**
 * Open the Stations page, carrying the station name into its row on the way.
 *
 * The same words are on screen in both places, so the transition is that they are the same
 * words rather than a wipe laid over the change. A copy does the flying — the name on the
 * player hides, the row's own name waits invisible, and the copy cross-fades into it at the
 * end, which is also what hides the player's name being uppercase with wide tracking while
 * the row's is neither.
 */
function openStationsFromName() {
  const name = stationNameEl.textContent || "";
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!name || reduced) {
    stationsPage.open();
    return;
  }

  const from = textCentre(stationNameEl);
  const startSize = parseFloat(getComputedStyle(stationNameEl).fontSize);
  const wasVertical = getComputedStyle(stationNameEl).writingMode.startsWith("vertical");

  stationsPage.open();

  // The page is laid out now, so the destination can be measured. Its row may be anywhere in
  // a long collection; put it on screen first, without a scroll animation racing this one.
  const row = stationsPage.activeRow();
  const target = row?.querySelector<HTMLElement>(".sp-name");
  if (!row || !target) return;
  row.scrollIntoView({ block: "center", behavior: "auto" });

  const to = textCentre(target);
  const endSize = parseFloat(getComputedStyle(target).fontSize);

  const fly = document.createElement("div");
  fly.className = "station-fly";
  fly.textContent = name;
  fly.style.left = `${from.x}px`;
  fly.style.top = `${from.y}px`;
  fly.style.fontSize = `${startSize}px`;
  document.body.appendChild(fly);

  stationNameEl.style.visibility = "hidden";
  target.style.opacity = "0";

  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const scale = endSize / startSize;
  // A quarter turn only when the name on the player is actually running vertically. It did
  // for one release; rotating a horizontal name to horizontal is a no-op with a cost.
  const turn = wasVertical ? "rotate(-90deg)" : "rotate(0deg)";
  const landed = `translate(calc(-50% + ${dx}px), calc(-50% + ${dy}px)) rotate(0deg) scale(${scale})`;

  // The easing is per-keyframe rather than on the effect. An effect-level easing warps the
  // offsets too, which put the copy 90% of the way there in the first quarter of the time
  // and then started the cross-fade halfway through the flight: it read as a snap followed
  // by a wait. Keyframe easings leave the offsets meaning wall-clock, so the move takes the
  // first 80% and the cross-fade is the last 20%, which is what those numbers look like.
  const anim = fly.animate(
    [
      {
        transform: `translate(-50%, -50%) ${turn} scale(1)`,
        opacity: 1,
        easing: "cubic-bezier(0.32, 0.72, 0.28, 1)",
      },
      { transform: landed, opacity: 1, offset: 0.8, easing: "linear" },
      { transform: landed, opacity: 0 },
    ],
    // `forwards`, or the copy snaps back to an untransformed position for the frame between
    // the animation ending and `land` removing it.
    { duration: FLIGHT_MS, fill: "forwards" }
  );
  const reveal = target.animate(
    [{ opacity: 0 }, { opacity: 0, offset: 0.8 }, { opacity: 1 }],
    { duration: FLIGHT_MS, fill: "forwards" }
  );

  const land = () => {
    fly.remove();
    reveal.cancel();
    stationNameEl.style.visibility = "";
    target.style.opacity = "";
  };
  anim.addEventListener("finish", land);
  anim.addEventListener("cancel", land);
  // A `finish` event is dispatched on a frame, and a window that is not on screen barely gets
  // any: clicking the name and immediately switching away would otherwise leave it hidden
  // and missing from its row until you came back. `land` is safe to run twice.
  setTimeout(land, FLIGHT_MS + 250);
}

stationBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  openStationsFromName();
});

// ---- state events -------------------------------------------------------
// Some engine events say how things *are* — what's playing, its thumbs, the mode, the station
// and the station list — and each is sent once, when it changes. An engine that comes up fast
// sends them before this module has registered its listeners, and a Tauri emit has no replay:
// resuming after an update left the window blank until the next song. So these are listened for
// through `onState`, and once every one is registered, `catchUpOnState` (below) asks the backend
// for the latest of each and runs it through the same handler. Listening first is what makes
// the pair airtight, the same way as the sign-in card's.
const stateListeners: Promise<[string, (payload: never) => void]>[] = [];

function onState<T>(event: string, handle: (payload: T) => void) {
  const replay = handle as (payload: never) => void;
  stateListeners.push(listen<T>(event, (e) => handle(e.payload)).then(() => [event, replay]));
}

// Which station is playing, with its token. The nowplaying event carries only the name,
// and two stations can share one — see save_last_station in native.rs.
onState<{ name: string; token: string }>("engine://station-active", (p) => {
  if (!p.token) return;
  stationStats.noteActive(p.token);
  activeStation = p.name || activeStation;
  activeToken = p.token;
  stationsPage.setActiveStation(activeStation, p.token);
});

onState<{ stations: StationInfo[] }>("engine://stations", (p) => {
  const next = p.stations;
  if (!next?.length) return;
  stationStats.prune(next.map((s) => s.token));
  stationsPage.setStations(next, activeStation);
  // The station list arriving means we know what's playing, so its modes are fetchable.
  void refreshModes();
});

// ---- top-right pages -----------------------------------------------------
// Cast is its own button, hidden until a WiiM that can start a station is on the LAN. It
// opens the same Stations panel as the station name — casting lives on the rows there.
const speakersBtn = $("speakers-btn");
speakersBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  stationsPage.open();
});
$("settings-btn").addEventListener("click", (e) => {
  e.stopPropagation();
  settingsPage.open();
});
// index.html paints from a cached preference before the first frame; this is the
// authoritative answer arriving a moment later.
void settingsPage.applyStoredAppearance();
// The name gives way first when the top bar runs out of room (fitTopbar), so a cut name
// is spelled out here.
attachTip(stationBtn, () =>
  isClipped(stationNameEl)
    ? `${stationNameEl.textContent} — browse, cast, export`
    : "Stations — browse, cast, export"
);
attachTip(speakersBtn, () => "Cast a station to the speakers");
attachTip($("settings-btn"), () => "Settings");

// ---- station modes (My Station / Crowd Faves / Discovery / Deep Cuts …) ----
// Modes are set over Pandora's REST API but the tuner playlist honours them, so switching here
// really does change what plays next. It affects newly generated playlists, not the current
// track — the engine clears its queue so the change is audible within a song or two.
type Mode = {
  modeId: number;
  modeName: string;
  modeButtonText: string;
  modeDescription: string;
  isInitialMode: boolean;
};

const modeWrap = $("mode-wrap");
const modeBtn = $("mode-btn");
const modePanel = $("mode-panel");
const modeList = $("mode-list");
let modes: Mode[] = [];
let activeMode = "";

function renderModes() {
  modeList.innerHTML = "";
  modes.forEach((mode) => {
    const label = mode.modeButtonText || mode.modeName;
    const item = document.createElement("div");
    item.className = "mode-item" + (label === activeMode ? " active" : "");

    const name = document.createElement("div");
    name.className = "mode-name";
    name.textContent = label;
    item.appendChild(name);

    // Pandora's own one-liner. Shown inline rather than on hover — the names alone don't
    // explain themselves, and hover text is invisible on a touchscreen.
    if (mode.modeDescription) {
      const desc = document.createElement("div");
      desc.className = "mode-desc";
      desc.textContent = mode.modeDescription;
      item.appendChild(desc);
    }

    item.addEventListener("click", () => {
      activeMode = label;
      modeBtn.textContent = label;
      modePanel.hidden = true;
      invoke("native_set_mode", { modeId: mode.modeId }).catch(() => {
        // Roll back if Pandora rejected it, rather than showing a mode that isn't set.
        void refreshModes();
      });
      renderModes();
    });
    modeList.appendChild(item);
  });
}

async function refreshModes() {
  try {
    modes = await invoke<Mode[]>("native_modes");
  } catch {
    modes = [];
  }
  // A station with only the default mode has nothing worth choosing between.
  modeWrap.hidden = modes.length < 2;
  if (!modes.length) return;
  if (!activeMode) {
    const initial = modes.find((m) => m.isInitialMode) ?? modes[0];
    activeMode = initial.modeButtonText || initial.modeName;
  }
  modeBtn.textContent = activeMode;
  renderModes();
}

modeBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  modePanel.hidden = !modePanel.hidden;
  if (!modePanel.hidden) renderModes();
});
window.addEventListener("click", (e) => {
  if (!modePanel.hidden && !(e.target as HTMLElement).closest("#mode-wrap")) {
    modePanel.hidden = true;
  }
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") modePanel.hidden = true;
});
onState<{ mode: string }>("engine://mode", (p) => {
  if (p.mode) {
    activeMode = p.mode;
    modeBtn.textContent = activeMode;
    renderModes();
  }
});

// ---- marquee: the title, and the artist · album line ---------------------
// A line wider than the column drifts slowly to its end and back on its own, and
// hovering it hands control to the pointer: x scrubs the line under the cursor.
// Neither reaches a hard edge — both ends of the travel keep MARQUEE_SLACK of air,
// and the pointer gets a dead zone at each end of the box, because otherwise the
// last word is only readable if you can land on the box's final pixel.

const MARQUEE_SLACK = 14; // px of air kept at each end of the travel
const MARQUEE_SPEED = 36; // px/s the idle drift walks at
// The share of the drift cycle spent walking one way; the rest is the pause at
// each end. Matches the 12%→50% and 62%→100% halves of @keyframes marquee-drift.
const MARQUEE_WALK = 0.38;

/** Wire `box` (which clips) and `inner` (which moves) up as a marquee; returns its re-measure. */
function marquee(box: HTMLElement, inner: HTMLElement): () => void {
  let travel = 0; // px the line can move; 0 when it fits
  let fade = 0; // px, from --mq-fade, so each line's fade can suit its type size
  let hover = false;
  let resume: ReturnType<typeof setTimeout> | undefined;

  /** Put the line at `x` px of translation and fade whichever sides are clipped. */
  function at(x: number) {
    inner.style.transform = `translateX(${x}px)`;
    const hidL = -x;
    const hidR = x + inner.scrollWidth - box.clientWidth;
    box.style.setProperty("--fade-l", `${Math.min(fade, Math.max(0, hidL))}px`);
    box.style.setProperty("--fade-r", `${Math.min(fade, Math.max(0, hidR))}px`);
  }

  /** Hand the line back to CSS: the rest position, and the drift if it overflows. */
  function release() {
    inner.style.transform = "";
    box.style.removeProperty("--fade-l");
    box.style.removeProperty("--fade-r");
    if (travel <= 0 || hover) return;
    // Re-add on the next frame so a drift already running restarts from the top —
    // a new track should be read from its first word, not from wherever the last
    // one had drifted to.
    box.classList.remove("drift");
    requestAnimationFrame(() => {
      if (!hover && travel > 0) box.classList.add("drift");
    });
  }

  /** Re-measure after anything that can change the text or the space it has. */
  function measure() {
    clearTimeout(resume);
    fade = parseFloat(getComputedStyle(box).getPropertyValue("--mq-fade")) || 0;
    const overflow = inner.scrollWidth - box.clientWidth;
    const fits = overflow <= 1;
    box.classList.toggle("fits", fits);
    travel = fits ? 0 : overflow + 2 * MARQUEE_SLACK;
    box.style.setProperty("--mq-a", `${MARQUEE_SLACK}px`);
    box.style.setProperty("--mq-b", `${MARQUEE_SLACK - travel}px`);
    box.style.setProperty(
      "--mq-cycle",
      `${Math.max(8, travel / MARQUEE_SPEED / MARQUEE_WALK).toFixed(1)}s`
    );
    release();
  }

  box.addEventListener("pointerenter", () => {
    hover = true;
    clearTimeout(resume);
    if (travel <= 0) return;
    // Take over from the drift where it actually is, rather than letting the line
    // snap to the start before the first mousemove lands.
    const x = new DOMMatrixReadOnly(getComputedStyle(inner).transform).m41;
    box.classList.remove("drift");
    inner.style.transition = "none";
    at(x);
    void inner.offsetWidth;
    inner.style.transition = "";
  });
  box.addEventListener("mousemove", (e) => {
    if (travel <= 0) return;
    const rect = box.getBoundingClientRect();
    const grab = Math.min(60, rect.width * 0.18);
    const span = Math.max(1, rect.width - 2 * grab);
    const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left - grab) / span));
    at(MARQUEE_SLACK - ratio * travel);
  });
  box.addEventListener("pointerleave", () => {
    hover = false;
    inner.style.transform = "";
    box.style.removeProperty("--fade-l");
    box.style.removeProperty("--fade-r");
    // Let it glide home before the drift takes the wheel back, or the handover
    // reads as a jump.
    resume = setTimeout(release, 450);
  });

  return measure;
}

const measureTitle = marquee(titleEl, titleInner);
const measureByline = marquee(bylineEl, bylineInner);

// The column's width moves with the window, and with it whether each line fits.
let marqueeMeasurePending = false;
window.addEventListener("resize", () => {
  if (marqueeMeasurePending) return;
  marqueeMeasurePending = true;
  requestAnimationFrame(() => {
    marqueeMeasurePending = false;
    measureTitle();
    measureByline();
  });
});

// ---- lyric sync nudge: [ = earlier, ] = later (0.25s steps, per-track) ----
function flashStatus(text: string) {
  lyricsStatus.textContent = text;
  lyricsStatus.classList.remove("flash");
  void lyricsStatus.offsetWidth; // restart the animation
  lyricsStatus.classList.add("flash");
}
function nudgeSync(delta: number) {
  if (!syncedLines) {
    flashStatus("No synced lyrics to nudge");
    return;
  }
  syncOffset = Math.round((syncOffset + delta) * 100) / 100;
  localStorage.setItem(`syncoff:${currentKey}`, String(syncOffset));
  activeLineIdx = NO_LINE; // force re-highlight
  highlightLine(lastPlayhead.position);
  flashStatus(
    syncOffset === 0
      ? "Synced lyrics · offset cleared"
      : `Synced lyrics · offset ${syncOffset > 0 ? "+" : ""}${syncOffset.toFixed(2)}s`
  );
}
window.addEventListener("keydown", (e) => {
  const el = e.target as HTMLElement;
  if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable) return;
  // Edit mode binds Space, and a whole-file offset means nothing while individual lines
  // are being retimed — the per-line times are the thing being fixed.
  if (lyricEditor.isEditing()) return;
  if (e.key === "[" || e.code === "BracketLeft") nudgeSync(-0.25);
  else if (e.key === "]" || e.code === "BracketRight") nudgeSync(0.25);
});

// SMTC (media keys / Windows panel) pressed: reflect the state immediately
// instead of waiting for motion confirmation.
listen<{ playing: boolean }>("player://optimistic", (e) => {
  setPlayingIcon(e.payload.playing);
  optimisticUntil = Date.now() + 2000;
});

// ---- remote (network player) mode ----------------------------------------
// Which source has focus — this computer or the network renderer — is decided once, in the
// backend (focus.rs), and the screen, the play button and the media keys all follow it. While
// the renderer has focus the UI is a display for it: art, metadata, synced lyrics.
//
// Whichever source *starts* playing takes focus (the speaker only while local is not
// playing), the source toggle chooses outright, and pausing never moves it. A source that
// keeps playing off screen shows in the mini player instead of taking the screen.
const sourceToggle = $("source-toggle");
const sourceLocalBtn = $<HTMLButtonElement>("source-local");
const sourceRemoteBtn = $<HTMLButtonElement>("source-remote");
const sourceRemoteName = $("source-remote-name");
const sourceRemoteIdle = $("source-remote-idle");

let focusRemote = false;
let focusHeard = false;
listen<{ remote: boolean }>("remote://focus", (e) => {
  focusHeard = true;
  focusRemote = e.payload.remote;
  updateMode();
});
invoke<boolean>("remote_focus")
  .then((r) => {
    if (focusHeard) return; // an event already said something newer
    focusRemote = r;
    updateMode();
  })
  .catch(() => {});

const localPlaying = () => Date.now() - lastLocalPlayingAt < 1600;

const remoteKey = (r: RemoteState) => `R|${r.title}|${r.artist}|${r.album}`;

function renderRemote(r: RemoteState) {
  const key = remoteKey(r);
  if (key === currentKey) return;
  titleInner.textContent = r.title || "—";
  artistEl.textContent = r.artist || "";
  albumEl.textContent = r.album || "";
  requestAnimationFrame(measureTitle);
  requestAnimationFrame(measureByline);
  if (r.art) setArt(r.art, "");
  currentKey = key;
  syncedLines = null;
  activeLineIdx = NO_LINE;
  syncOffset = parseFloat(localStorage.getItem(`syncoff:${key}`) || "0") || 0;
  lyricFlags.trackChanged();
  loadLyricsFor({ title: r.title, artist: r.artist, album: r.album }, r.duration || null, key);
}

// Focus can name the speaker a moment before its state arrives (and the speaker can lose its
// track first); the screen switches only when there is a speaker track to show.
function updateMode() {
  const want = focusRemote && !!remote;
  if (want === remoteMode) {
    if (remoteMode && remote) renderRemote(remote); // track change within remote mode
    reflectRemoteStation();
    reflectSources();
    return;
  }
  setRemoteMode(want);
}

function setRemoteMode(want: boolean) {
  remoteMode = want;
  document.body.classList.toggle("remote", remoteMode);
  currentKey = ""; // force full re-render for the new source
  miniHeld = false; // the mini now shows the other source
  if (remoteMode && remote) {
    loginHint.hidden = true;
    showPlayer();
    renderRemote(remote);
    setPlayingIcon(remote.playing);
  } else if (lastLocalNp) {
    onNowPlaying(lastLocalNp);
    // The icon was the speaker's; show the local engine's until its playhead says otherwise.
    setPlayingIcon(localPlaying());
  }
  reflectRemoteStation();
  reflectSources();
}

// The source toggle: always on screen once a network player has been found. It moves focus
// only — neither source is started or paused by it. The dot on a segment means that source
// is audibly playing, so a speaker playing in the background is never a surprise.
function reflectSources() {
  sourceToggle.hidden = !remoteDevice;
  sourceRemoteName.textContent = remoteDevice;
  // Nothing loaded on the speaker means nothing to show; casting a station gives it a track.
  sourceRemoteIdle.hidden = !!remote;
  sourceRemoteBtn.disabled = !remote;
  setSegment(sourceLocalBtn, !remoteMode, localPlaying());
  setSegment(sourceRemoteBtn, remoteMode, !!remote?.playing);
  reflectMini();
}
function setSegment(b: HTMLButtonElement, on: boolean, playing: boolean) {
  b.classList.toggle("on", on);
  b.setAttribute("aria-checked", String(on));
  b.classList.toggle("playing", playing);
}
function chooseSource(toRemote: boolean) {
  if (toRemote === remoteMode) return;
  invoke("remote_focus_set", { remote: toRemote }).catch(() => {});
}
sourceLocalBtn.addEventListener("click", () => chooseSource(false));
sourceRemoteBtn.addEventListener("click", () => chooseSource(true));

// ---- mini player: the source that is not on screen ----------------------
// Shown while that source plays, so audio is never coming from somewhere the screen doesn't
// mention. Its play/pause drives that source directly; once paused from here it stays, so it
// can be resumed, until focus moves. Starting a source can itself move focus (see above), so
// resuming this computer from here brings it on screen.
const mini = $("mini");
const miniArt = $<HTMLImageElement>("mini-art");
const miniTitle = $("mini-title");
const miniSub = $("mini-sub");
const miniPlayIcon = $("mini-play-icon");
const miniPauseIcon = $("mini-pause-icon");
let miniHeld = false;
let miniOptimistic: { playing: boolean; until: number } | null = null;

interface MiniSource {
  title: string;
  sub: string;
  art: string;
  playing: boolean;
}
function miniSource(): MiniSource | null {
  if (remoteMode) {
    if (!lastLocalNp) return null;
    const np = lastLocalNp;
    return {
      title: np.title,
      sub: [np.artist, "this computer"].filter(Boolean).join(" · "),
      art: np.art || np.artFallback,
      playing: localPlaying(),
    };
  }
  if (!remote) return null;
  return {
    title: remote.title,
    sub: [remote.artist, `on ${remote.device}`].filter(Boolean).join(" · "),
    art: remote.art,
    playing: remote.playing,
  };
}

function reflectMini() {
  const m = miniSource();
  if (!m) miniHeld = false;
  if (!m || !(m.playing || miniHeld)) {
    mini.hidden = true;
    return;
  }
  mini.hidden = false;
  if (miniTitle.textContent !== m.title) miniTitle.textContent = m.title;
  if (miniSub.textContent !== m.sub) miniSub.textContent = m.sub;
  if (m.art && miniArt.getAttribute("src") !== m.art) miniArt.src = m.art;
  miniArt.hidden = !m.art;
  const playing =
    miniOptimistic && Date.now() < miniOptimistic.until ? miniOptimistic.playing : m.playing;
  miniPlayIcon.toggleAttribute("hidden", playing);
  miniPauseIcon.toggleAttribute("hidden", !playing);
}

$("mini-play").addEventListener("click", () => {
  const m = miniSource();
  if (!m) return;
  const desired = !m.playing;
  miniHeld = true;
  miniOptimistic = { playing: desired, until: Date.now() + 2000 };
  if (remoteMode) cmd(desired ? "play" : "pause");
  else invoke("remote_cmd", { cmd: desired ? "play" : "pause" }).catch(() => {});
  reflectMini();
});
$("mini-open").addEventListener("click", () => chooseSource(!remoteMode));

// The station button names the speaker's station while the speaker owns the screen, and
// the Stations page marks that row — the WiiM says which station its queue is, and its id is
// Jarlid's own token. Both go back to Jarlid's station when remote mode ends. The button
// hides only when the speaker is playing something that is not a Pandora station.
let shownRemoteToken = "";
function reflectRemoteStation() {
  const rs = remoteMode && remote?.station ? remote : null;
  if (rs) {
    stationBtn.hidden = false;
    stationNameEl.textContent = rs.station;
    if (rs.stationToken && rs.stationToken !== shownRemoteToken) {
      shownRemoteToken = rs.stationToken;
      stationsPage.setActiveStation(rs.station, rs.stationToken);
    }
    return;
  }
  stationBtn.hidden = remoteMode;
  if (shownRemoteToken) {
    shownRemoteToken = "";
    stationNameEl.textContent = lastLocalNp?.station || "Stations";
    if (activeToken) stationsPage.setActiveStation(activeStation, activeToken);
  }
}

// ---- top bar: fitting it beside the player ---------------------------------
// The bar holds where the music plays, the station and its mode, cast and settings — ~650px
// with a speaker on the LAN and a station with modes, more than a 1000px window has to the
// right of the album art. It is capped at the lyrics column (it must not reach over the
// art), or at the window less its insets in the stacked layout, where it has its own strip.
//
// When it doesn't fit, it gives way in a fixed order, least informative first:
//   1. the station name ellipsizes, down to STATION_NAME_MIN;
//   2. then " · idle" goes — an idle speaker's segment is already greyed out (disabled),
//      and the station name gets back whatever that frees;
//   3. then the speaker's name ellipsizes, down to SPEAKER_NAME_MIN;
//   4. then the location toggle drops its words for an icon per segment (dots kept), and
//      the station name gets back whatever that frees;
//   5. and only then does the station name go below its floor.
// Every cut name, and the dropped "idle", is in a tooltip. Step 2 is what keeps the words
// at 1000×620 with an idle "Living Room", a station with modes and the cast button: 36px
// short after step 1, and dropping " · idle" frees 43px.
//
// Measured rather than left to flex-shrink because the order is the point: flex shares a
// shortfall out in proportion, so the speaker's name would be cut while the station name
// still had room to give, and nothing in CSS can say "switch to icons once the names are as
// short as they go" — that depends on how long the names are. A container query would have
// to guess a width. Measuring reads the bar's own natural width, which nothing else depends
// on, so there is no layout feedback loop: the lyrics column it is capped by does not move
// when the bar changes.
const topbar = $("topbar");
const lyricsPane = document.querySelector<HTMLElement>(".lyrics-pane")!;
const TOPBAR_INSET = 14; // #topbar's top/right in styles.css
const STATION_NAME_MIN = 72; // "TODAY'S…" in the bar's type
const SPEAKER_NAME_MIN = 64; // "LIVING…"
const stackedLayout = matchMedia("(max-width: 820px)");

function isClipped(el: HTMLElement) {
  return el.scrollWidth > el.clientWidth + 0.5;
}

/** How wide the bar may be: up to the lyrics column, or the window in the stacked layout. */
function topbarRoom(): number {
  const lyr = lyricsPane.getBoundingClientRect();
  const left = player.hidden || stackedLayout.matches || lyr.width === 0 ? TOPBAR_INSET : lyr.left;
  return innerWidth - TOPBAR_INSET - left;
}

/** The bar's natural width: its visible controls end to end, overflow included. */
function topbarNeed(): number {
  const shown = Array.from(topbar.children).filter((el) => el.getClientRects().length > 0);
  if (!shown.length) return 0;
  return shown[shown.length - 1].getBoundingClientRect().right - shown[0].getBoundingClientRect().left;
}

function fitTopbar() {
  topbar.style.maxWidth = "";
  topbar.classList.remove("terse", "compact");
  stationNameEl.style.maxWidth = "";
  sourceRemoteName.style.maxWidth = "";
  const room = topbarRoom();
  topbar.style.maxWidth = `${Math.max(0, room)}px`;
  let over = 0;
  // After a step that frees room on its own, the names start again from full length.
  const remeasure = () => {
    stationNameEl.style.maxWidth = "";
    sourceRemoteName.style.maxWidth = "";
    over = topbarNeed() - room;
  };
  // Take up to `over` from an element's width, leaving at least `floor` of it.
  const give = (el: HTMLElement, floor: number) => {
    const w = el.getBoundingClientRect().width;
    const cut = Math.min(over, Math.max(0, w - floor));
    if (cut > 0) {
      el.style.maxWidth = `${w - cut}px`;
      over -= cut;
    }
  };

  remeasure();
  if (over <= 0) return;
  give(stationNameEl, STATION_NAME_MIN);
  if (over <= 0) return;
  if (sourceToggle.hidden) {
    give(stationNameEl, 0);
    return;
  }

  if (!sourceRemoteIdle.hidden) {
    topbar.classList.add("terse");
    remeasure();
    give(stationNameEl, STATION_NAME_MIN);
    if (over <= 0) return;
  }
  give(sourceRemoteName, SPEAKER_NAME_MIN);
  if (over <= 0) return;

  topbar.classList.add("compact");
  remeasure();
  give(stationNameEl, STATION_NAME_MIN);
  if (over > 0) give(stationNameEl, 0);
}

// Refit whenever something in the bar appears, disappears or is renamed, when remote mode
// hides the mode, and when the room changes. The observer ignores `style` and `class`,
// which are what fitTopbar itself writes.
new MutationObserver(() => fitTopbar()).observe(topbar, {
  subtree: true,
  childList: true,
  characterData: true,
  attributes: true,
  attributeFilter: ["hidden"],
});
new MutationObserver(() => fitTopbar()).observe(document.body, {
  attributes: true,
  attributeFilter: ["class"],
});
new ResizeObserver(() => fitTopbar()).observe(lyricsPane);
window.addEventListener("resize", fitTopbar);
void document.fonts.ready.then(fitTopbar);

attachTip(sourceLocalBtn, () => (topbar.classList.contains("compact") ? "This computer" : ""));
attachTip(sourceRemoteBtn, () =>
  topbar.classList.contains("compact") ||
  topbar.classList.contains("terse") ||
  isClipped(sourceRemoteName)
    ? `${remoteDevice}${remote ? "" : " · idle"}`
    : ""
);

let remoteDevice = "";
listen<RemoteState>("remote://state", (e) => {
  const st = e.payload;
  remoteDevice = st?.device || "";
  // Casting lives on the Stations page: each row can send its station to the speakers, and
  // the top-bar cast button opens that page. Both appear only when the device can cast.
  const canCast = !!st?.canCast;
  speakersBtn.hidden = !canCast;
  stationsPage.setRemoteDevice(remoteDevice, canCast);
  remote = st && st.title ? st : null;
  remoteAt = Date.now();
  updateMode();
  reflectRemoteVolume();
});

// ---- remote volume slider -------------------------------------------------
const remoteVol = $<HTMLInputElement>("remote-vol");
let volDragging = false;
let volSendTimer: number | undefined;
remoteVol.addEventListener("pointerdown", () => (volDragging = true));
remoteVol.addEventListener("pointerup", () => (volDragging = false));
remoteVol.addEventListener("input", () => {
  clearTimeout(volSendTimer);
  volSendTimer = window.setTimeout(() => {
    invoke("remote_cmd", { cmd: `vol:${remoteVol.value}` }).catch(() => {});
  }, 150);
});
function reflectRemoteVolume() {
  if (!remote || volDragging) return;
  if (remote.volume >= 0) remoteVol.value = String(Math.round(remote.volume));
  remoteVol.parentElement!.style.visibility = remote.volume >= 0 ? "visible" : "hidden";
}

// Interpolate the remote position between the 1s device polls. The source toggle's playing
// dots and the mini player follow local motion too, which has no event of its own to stop on.
setInterval(() => {
  reflectSources();
  if (!remoteMode || !remote) return;
  let pos = remote.position + (remote.playing ? (Date.now() - remoteAt) / 1000 : 0);
  if (remote.duration > 0) pos = Math.min(pos, remote.duration);
  const pct = remote.duration > 0 ? (pos / remote.duration) * 100 : 0;
  barEl.style.width = `${Math.min(100, pct)}%`;
  tCur.textContent = fmt(pos);
  tDur.textContent = fmt(remote.duration);
  if (Date.now() >= optimisticUntil) setPlayingIcon(remote.playing);
  followLyrics(pos, remote.duration);
}, 400);

// ---- events from the engine bridge ------------------------------------
onState<NowPlaying>("engine://nowplaying", (np) => void onNowPlaying(np));
listen<Playhead>("engine://playhead", (e) => onPlayhead(e.payload));
listen<Technical>("engine://technical", (e) => renderTechnical(e.payload));
onState<{ thumbUp: boolean; thumbDown: boolean }>("engine://thumbs", (p) =>
  setThumbs(p.thumbUp, p.thumbDown)
);

// Every state listener is registered above this point. Catch up on whatever was sent before
// they were, in the order they were registered: the station before the station list, which
// marks it.
void (async function catchUpOnState() {
  try {
    const handlers = await Promise.all(stateListeners);
    const latest = await invoke<Record<string, unknown>>("native_latest_state");
    for (const [event, handle] of handlers) {
      if (event in latest) handle(latest[event] as never);
    }
  } catch (e) {
    // The events still arrive from here on; at worst the window waits for the next song.
    console.warn("catching up on engine state failed", e);
  }
})();

function showLogin() {
  player.hidden = true;
  loginHint.hidden = false;
  // Signing out is the other transition the badge has to hear about: with no engine behind
  // it, an armed update installs shortly rather than waiting for a song or a pause.
  renderVersion();
}

// Ask as well as listen, and in that order.
//
// Listening alone was not enough. On a fresh install the engine gives up on the empty credential
// store in microseconds — before this module has been evaluated at all — so `engine://needs-login`
// was reliably emitted into a webview with no listener yet, and a Tauri emit has no replay. The
// event went nowhere, nothing else unhides the card, and a new machine had no way to sign in.
// Asking covers that, because the backend latches the state rather than only announcing it.
//
// The order is what covers the other direction. `listen` registers asynchronously, so querying
// first would leave a window where the latch is set after the read but before the listener
// exists — missed by both halves. Registering first means anything the query is too early to see,
// the event still catches. The two are redundant on purpose; whichever lands second is a no-op.
//
// Authoritative either way: the engine reports this only when the credential store is empty or the
// saved credentials were rejected. The old `everPlayed` guard existed because Pandora's page fired
// spurious login signals during its initial load; there is no page any more.
void (async () => {
  try {
    await listen("engine://needs-login", showLogin);
    if (await invoke<boolean>("native_needs_login")) showLogin();
  } catch {
    // Not worth a toast. If the query failed the listener is still registered, and the card is
    // reachable the moment the engine says so.
  }
})();

// ---- toast --------------------------------------------------------------
// Engine problems used to be written into #login-error, which lives inside the sign-in card and
// is hidden the moment you are signed in — so a failure during playback went nowhere at all.
const toast = $("toast");
const toastMsg = $("toast-msg");
const toastAction = $<HTMLButtonElement>("toast-action");
let toastTimer: number | undefined;

function showToast(
  message: string,
  action?: { label: string; run: () => void },
  autoHideMs = 8000
) {
  toastMsg.textContent = message;
  toastAction.hidden = !action;
  if (action) {
    toastAction.textContent = action.label;
    toastAction.onclick = () => {
      hideToast();
      action.run();
    };
  }
  toast.hidden = false;
  window.clearTimeout(toastTimer);
  // Sticky when there is something to do about it; a toast that vanishes before you can click
  // its button is worse than none.
  if (!action) toastTimer = window.setTimeout(hideToast, autoHideMs);
}

function hideToast() {
  toast.hidden = true;
  window.clearTimeout(toastTimer);
}
$("toast-close").addEventListener("click", hideToast);

// ---- problem reporting --------------------------------------------------
// Everything sent to GitHub is redacted backend-side (see diagnostics.rs) and lands in GitHub's
// editor for review — nothing is ever submitted on the user's behalf.
function reportIssue(note = "") {
  invoke("native_report_issue", {
    context: {
      userAgent: navigator.userAgent,
      station: stationNameEl.textContent ?? "",
      sourceStation: sourceEl.textContent ?? "",
      mode: modeBtn.textContent ?? "",
      remote: remoteMode,
      note,
    },
  }).catch((err) => showToast(`Could not open the issue page: ${err}`));
}

/** Record a problem for the next bug report, whether or not one is filed now. */
function recordIncident(source: string, message: string) {
  invoke("native_record_incident", { source, message }).catch(() => {});
}

const REPORT = { label: "Report issue", run: () => reportIssue() };

listen<{ message: string }>("engine://error", (e) =>
  showToast(e.payload.message, REPORT)
);

// A thrown error in the UI used to leave the interface subtly wrong with nothing said about it.
window.addEventListener("error", (e) => {
  const where = e.filename ? ` (${e.filename}:${e.lineno})` : "";
  recordIncident("ui", `${e.message}${where}`);
  showToast("Something went wrong in the interface.", REPORT);
});

// Unhandled rejections are the common shape here: every invoke() returns a promise.
window.addEventListener("unhandledrejection", (e) => {
  recordIncident("ui", `unhandled rejection: ${e.reason}`);
  showToast("Something went wrong in the interface.", REPORT);
});

// Another device holds the account's single stream. Recoverable, and the engine keeps retrying
// on its own — so say so, and offer to claim it now rather than waiting.
listen<{ message: string }>("engine://stream-taken", (e) => {
  showToast(e.payload.message, {
    label: "Play here",
    run: () => {
      invoke("native_take_over")
        .then(() => showToast("Playing here now.", undefined, 3000))
        .catch(() => {
          recordIncident("engine", "take over failed");
          showToast(
            "Could not take over — the other device still has it.",
            REPORT
          );
        });
    },
  });
});
