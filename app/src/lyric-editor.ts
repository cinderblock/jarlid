// Editing lyrics in place, on the lyrics pane itself.
//
// The first version of this was a full page you opened. It was wrong: writing a whole
// lyric set or hand-typing timestamps is rare, and fixing *one line* is what actually
// happens. So the pencil now flips the pane itself into edit mode and every line carries
// its own controls.
//
// Two jobs live here, and they are deliberately not the same mode:
//
//   Correction — the lyrics already have timings and something is wrong. Per-line: fix
//   the words, set this line's start to now, check it by jumping back and listening.
//
//   Timestamp — the lyrics have no timings at all. The song plays and you mark each line
//   as it starts, continuously. Nothing jumps back, because a backward seek re-opens the
//   stream and rebuffers; doing that between every line would make the pass impossible.
//
// While edit mode is on this module owns the pane: it renders the rows and drives its own
// cursor. `highlightLine` in main.ts stands down, because it indexes `.line` nodes
// positionally and assumes ascending timestamps, and neither holds mid-pass.

import { invoke } from "@tauri-apps/api/core";
import * as confirm from "./lyric-confirm";
import * as timeline from "./lyric-timeline";
import { publicationFor } from "./lyric-publish";
import { fillMarked } from "./draft-marks";
import {
  applyReview,
  isBlank,
  joinLine,
  reviewState,
  splitLine,
  unreviewed as unreviewedIn,
  fromTiming,
  mergeTiming,
  retext,
  toTiming,
  type Line,
  type TimedLine,
} from "./edit-lines";

export interface Lyrics {
  synced: string | null;
  plain: string | null;
  source: string;
  /// The LRCLIB record this came from. A correction is a republish keyed on these four
  /// fields, so they have to travel with the lyrics or we'd correct the wrong record.
  id: number | null;
  trackName: string | null;
  artistName: string | null;
  albumName: string | null;
  duration: number | null;
  /// True when this is a local edit rather than what LRCLIB currently serves.
  overridden: boolean;
  /// Set when this is a cached copy old enough to be worth checking again.
  stale?: boolean;
  /// Set when the listener chose this record over the automatic match.
  pinned?: boolean;
  /// Set when `synced` was worked out on this machine from LRCLIB's plain lyrics, because
  /// LRCLIB had no timings (see `autosync` in Rust). Never published unless the listener does.
  auto?: boolean;
  /// Set while these lyrics are a draft the speech engine wrote, for a song LRCLIB has nothing
  /// for, and not every line has been reviewed. Kept in a saved edit until the review is done;
  /// until then it can't be published. Indexed by line, blank lines included.
  draft?: { doubt: number[][]; reviewed: boolean[] } | null;
  /// Why there is no draft for a song LRCLIB has nothing for.
  noDraft?: "noSinging" | "unclear" | null;
  /// What `synced` can't hold: word timings and line ends, line by line (see `lyricsfile` in
  /// Rust). Present when the record or the saved edit has any.
  timing?: TimedLine[] | null;
}

export interface EditorContext {
  /// Pandora's naming for the track. Also the key local edits are filed under.
  meta: { title: string; artist: string; album: string };
  lyrics: Lyrics;
  /// The edited track's length, captured when editing began. The playhead's own duration
  /// can't stand in for it: once playback moves on it is the next song's.
  duration: number | null;
  playhead: () => { position: number; duration: number; paused: boolean };
  transport: (cmd: string) => void;
  seek: (position: number) => void;
  /// False when a network renderer owns playback: timing needs a local playhead.
  canTransport: boolean;
  /// Hand replacement lyrics back to the now-playing screen.
  onApplied: (lyrics: Lyrics) => void;
  /// Leave edit mode and repaint the pane normally.
  onExit: () => void;
}

/// Taps land late — you hear the line, then press. Shifting stamps earlier by about a
/// reaction time is what makes a hand-tapped file feel right rather than consistently
/// behind.
const TAP_LATENCY = 0.33;

/// How far before a line to drop the playhead when checking it. Long enough to hear the
/// run-in, short enough not to sit through the previous line.
const CHECK_LEAD = 2.5;

/// How far "Back" goes during a timing pass: enough to hear the run-in to a line that was
/// marked wrong, without replaying the whole verse.
const REWIND = 5;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const lyricsEl = $("lyrics");
const bar = $("lyric-bar");
const statusEl = $("lyric-bar-status");
const timestampBtn = $<HTMLButtonElement>("lyric-timestamp");
const rewindBtn = $<HTMLButtonElement>("lyric-rewind");
const autoCheckWrap = $("lyric-autocheck-wrap");
const autoCheckBox = $<HTMLInputElement>("lyric-autocheck");
const saveBtn = $<HTMLButtonElement>("lyric-save");
const revertBtn = $<HTMLButtonElement>("lyric-revert");
const publishBtn = $<HTMLButtonElement>("lyric-publish");
const flagBtn = $<HTMLButtonElement>("lyric-flag");
const doneBtn = $<HTMLButtonElement>("lyric-done");
const timelineBtn = $<HTMLButtonElement>("lyric-timeline-btn");

let ctx: EditorContext | null = null;
let lines: Line[] = [];
let editing = false;
/// The lyrics are a draft (see `Lyrics.draft`): every line carries a ✓ to check it off, and
/// Publish waits until all of them are.
let reviewing = false;
/// The line selected in the list and on the timeline, or -1.
let selected = -1;
/// The line the next mark lands on, during a timestamp pass.
let cursor = 0;
let stamping = false;
let dirty = false;
let busy = false;
let stale = false;

export const isEditing = () => editing;

// ---- LRC text <-> lines ---------------------------------------------------

const STAMP = /^\s*\[(\d+):(\d+(?:[.:]\d+)?)\]\s?/;

function parseLines(text: string): Line[] {
  return text.split(/\r?\n/).map((raw) => {
    const m = STAMP.exec(raw);
    if (!m) return { t: null, text: raw.trim() };
    return {
      t: parseInt(m[1], 10) * 60 + parseFloat(m[2].replace(":", ".")),
      text: raw.slice(m[0].length).trim(),
    };
  });
}

function stampOf(t: number) {
  const m = Math.floor(t / 60);
  return `${String(m).padStart(2, "0")}:${(t - m * 60).toFixed(2).padStart(5, "0")}`;
}

const serialize = (ls: Line[]) =>
  ls.map((l) => (l.t === null ? l.text : `[${stampOf(l.t)}] ${l.text}`)).join("\n");

const anyTimed = () => lines.some((l) => l.t !== null);

/// The first line at or after `from` that a timing pass should land on, or -1.
const nextLyric = (from: number) => {
  for (let i = Math.max(0, from); i < lines.length; i++) if (!isBlank(lines[i])) return i;
  return -1;
};

// ---- entering / leaving ---------------------------------------------------

export function begin(context: EditorContext) {
  ctx = context;
  editing = true;
  dirty = false;
  stale = false;
  busy = false;
  stamping = false;
  cursor = 0;

  load(context.lyrics);

  lyricsEl.classList.add("editing");
  bar.hidden = false;
  render();
  refreshBar();
  if (reviewing) {
    setStatus(
      "A draft the speech engine wrote. Listen along, fix what's wrong, and ✓ each line once it's right. Underlined words are the ones it wasn't sure of."
    );
  }
  followTimer = window.setInterval(followPlayhead, 250);
  selected = -1;
  if (localStorage.getItem(TIMELINE_KEY) === "open") openTimeline();
}

/// Take `l` as the lines to edit, with a draft's review state when it is one.
function load(l: Lyrics) {
  const src = l.synced || l.plain || "";
  // The LRC says which lines there are; a Lyricsfile's timing adds their words' times and
  // line ends. With no LRC at all, the timing is all there is.
  if (src) {
    lines = l.timing?.length ? mergeTiming(parseLines(src), l.timing) : parseLines(src);
  } else {
    lines = l.timing?.length ? fromTiming(l.timing) : [{ t: null, text: "" }];
  }
  const draft = l.draft;
  reviewing = !!draft;
  if (draft) applyReview(lines, draft);
}

export function end() {
  editing = false;
  stamping = false;
  window.clearInterval(followTimer);
  timeline.close();
  nowRow = -1;
  lyricsEl.classList.remove("editing", "stamping");
  bar.hidden = true;
  confirm.cancel();
  ctx?.onExit();
  ctx = null;
}

doneBtn.addEventListener("click", () => {
  // Leaving with unsaved work should be possible, but not by accident — a timing pass is
  // several minutes of tapping and a stray click on Done should not bin it silently.
  if (dirty) {
    showConfirm({
      title: "Leave without saving?",
      message: "The changes on screen have not been saved, and closing the editor drops them.",
      go: "Discard",
      run: async () => {
        dirty = false;
        end();
      },
    });
    return;
  }
  end();
});

/// Playback moved to another track. The words still belong to the track being edited and
/// still save to it, but the playhead does not, so marking would write another song's
/// times into this one.
export function notePlaybackMoved() {
  if (!editing || stale) return;
  stale = true;
  stamping = false;
  lyricsEl.classList.remove("stamping");
  refreshBar();
  render();
  setStatus(
    `Playback moved on. Words still save to ${ctx?.meta.artist} — ${ctx?.meta.title}, but timing needs this track playing.`,
    "err"
  );
}

// ---- rendering ------------------------------------------------------------

/// Rows keep `.line` and `data-idx` so the pane's own styling still applies; the extra
/// children are what edit mode adds.
function render() {
  lyricsEl.innerHTML = "";
  lines.forEach((line, i) => {
    const row = document.createElement("div");
    row.className = "line editable";
    row.dataset.idx = String(i);
    if (stamping && i === cursor) row.classList.add("cursor");
    if (isBlank(line)) row.classList.add("blank");
    if (line.t !== null) row.classList.add("timed");
    if (i === nowRow) row.classList.add("now");
    if (i === selected) row.classList.add("selected");
    if (reviewing) row.classList.add("draft");

    const time = document.createElement("button");
    time.className = "ln-time";
    time.textContent = line.t === null ? "––:––" : stampOf(line.t);
    time.addEventListener("click", () => (stamping ? markAt(i) : setNow(i)));

    const text = document.createElement("div");
    text.className = "ln-text";
    text.contentEditable = "plaintext-only";
    text.spellcheck = false;
    fillMarked(text, line.text, line.doubt ?? []);
    text.addEventListener("input", () => {
      retext(lines[i], text.textContent ?? "");
      // The reviewer has been at this line: the engine's doubts about it are answered.
      if (lines[i].doubt?.length) {
        lines[i].doubt = [];
        row.classList.add("touched");
      }
      markDirty();
    });
    text.addEventListener("paste", (e) => onPaste(e, i));
    text.addEventListener("keydown", (e) => onLineKey(e, i, text));
    text.addEventListener("focus", () => selectLine(i, false));

    const check = document.createElement("button");
    check.className = "ln-check";
    check.textContent = "▶";
    check.disabled = line.t === null || !ctx?.canTransport || stale;
    check.addEventListener("click", () => checkLine(i));

    row.append(time, text, check);
    if (reviewing) row.append(reviewButton(line, i, row));
    lyricsEl.appendChild(row);
  });
}

/// A draft line's ✓: pressed once the line's words and start are right. A line in timed
/// lyrics needs a start before it can be checked, or it would vanish from the synced copy.
function reviewButton(line: Line, i: number, row: HTMLElement) {
  const ok = document.createElement("button");
  ok.className = "ln-ok";
  ok.textContent = "✓";
  if (isBlank(line)) {
    ok.disabled = true;
    ok.style.visibility = "hidden";
    return ok;
  }
  const paint = () => {
    row.classList.toggle("reviewed", !!lines[i].ok);
    ok.setAttribute("aria-pressed", String(!!lines[i].ok));
    ok.setAttribute("aria-label", lines[i].ok ? "Reviewed — undo" : "Mark this line reviewed");
  };
  ok.disabled = line.t === null && anyTimed();
  ok.addEventListener("click", () => {
    lines[i].ok = !lines[i].ok;
    paint();
    markDirty();
  });
  paint();
  return ok;
}

// ---- the timeline -----------------------------------------------------------
// The song's waveform with the lines laid on it (lyric-timeline.ts). Its edits go straight
// into `lines`; the list is repainted to match.

const TIMELINE_KEY = "lyrics.timeline";

function openTimeline() {
  timelineBtn.classList.add("on");
  timelineBtn.setAttribute("aria-pressed", "true");
  timeline.open({
    meta: () => ctx!.meta,
    lines: () => lines,
    selected: () => selected,
    select: (i) => selectLine(i, true),
    changed: () => {
      markDirty();
      render();
    },
    playhead: () => ctx!.playhead(),
    duration: () => trackDuration(),
    canSeek: () => !!ctx?.canTransport && !stale,
    seek: (t) => ctx?.seek(t),
  });
}

timelineBtn.addEventListener("click", () => {
  if (timeline.isOpen()) {
    timeline.close();
    timelineBtn.classList.remove("on");
    timelineBtn.setAttribute("aria-pressed", "false");
    localStorage.setItem(TIMELINE_KEY, "closed");
  } else {
    openTimeline();
    localStorage.setItem(TIMELINE_KEY, "open");
  }
});

/// Select line `i` in the list and on the timeline. From the timeline, the list scrolls to the
/// row; from the list, the timeline scrolls to the block.
function selectLine(i: number, fromTimeline: boolean) {
  if (i === selected) return;
  lyricsEl.querySelector(`.line[data-idx="${selected}"]`)?.classList.remove("selected");
  selected = i;
  const row = lyricsEl.querySelector<HTMLElement>(`.line[data-idx="${i}"]`);
  row?.classList.add("selected");
  if (fromTimeline) row?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  else timeline.reveal(i);
}

// ---- following the song ----------------------------------------------------
// Reviewing is reading along: the row now playing is marked, and kept in view unless the
// reviewer is busy in a line. A timing pass has its own cursor and needs none of this.

let followTimer = 0;
/// The row marked as playing, or -1.
let nowRow = -1;

function followPlayhead() {
  if (!editing || !ctx) return;
  let at = -1;
  // Once playback has moved to another song, its playhead says nothing about these lines.
  if (!stamping && !stale) {
    const position = ctx.playhead().position;
    // Mid-edit, times needn't be in order: the latest start already passed wins.
    let best = -Infinity;
    lines.forEach((l, i) => {
      if (l.t !== null && !isBlank(l) && l.t <= position && l.t >= best) {
        best = l.t;
        at = i;
      }
    });
  }
  if (at === nowRow) return;
  lyricsEl.querySelector(`.line[data-idx="${nowRow}"]`)?.classList.remove("now");
  nowRow = at;
  const row = lyricsEl.querySelector<HTMLElement>(`.line[data-idx="${at}"]`);
  row?.classList.add("now");
  const typing = (document.activeElement as HTMLElement | null)?.isContentEditable;
  if (row && reviewing && !typing) row.scrollIntoView({ block: "nearest", behavior: "smooth" });
}

function focusLine(i: number, toEnd = true) {
  const el = lyricsEl.querySelector<HTMLElement>(`.line[data-idx="${i}"] .ln-text`);
  if (!el) return;
  el.focus();
  const range = document.createRange();
  range.selectNodeContents(el);
  range.collapse(!toEnd);
  const sel = getSelection();
  sel?.removeAllRanges();
  sel?.addRange(range);
}

// ---- line editing ---------------------------------------------------------

/// Enter splits the line at the caret (at the end, that opens a new line below), Backspace
/// on an empty one removes it, and Backspace at the start of one joins it to the line above.
/// This is what replaces the old full-page textarea: adding and removing lines has to be
/// possible without a separate editor, or "no lyrics found" is a dead end — and a draft's
/// lines are often broken in the wrong place.
function onLineKey(e: KeyboardEvent, i: number, el: HTMLElement) {
  const text = el.textContent ?? "";
  if (e.key === "Enter") {
    e.preventDefault();
    const at = caretOffset(el) ?? text.length;
    splitLine(lines, i, text, at, reviewing);
    if (cursor > i) cursor++;
    markDirty();
    render();
    focusLine(i + 1, false);
  } else if (e.key === "Backspace" && text && i > 0 && caretOffset(el) === 0) {
    e.preventDefault();
    const at = joinLine(lines, i, text, reviewing);
    if (cursor >= i) cursor--;
    markDirty();
    render();
    focusLineAt(i - 1, at);
  } else if (e.key === "Backspace" && !text && lines.length > 1) {
    e.preventDefault();
    lines.splice(i, 1);
    if (cursor > i) cursor--;
    markDirty();
    render();
    focusLine(Math.max(0, i - 1));
  }
}

/// Characters before the caret in `el`, or null without a caret in it.
function caretOffset(el: HTMLElement): number | null {
  const sel = getSelection();
  if (!sel?.rangeCount || !sel.isCollapsed) return null;
  const range = sel.getRangeAt(0);
  if (!el.contains(range.startContainer)) return null;
  const before = document.createRange();
  before.selectNodeContents(el);
  before.setEnd(range.startContainer, range.startOffset);
  return before.toString().length;
}

/// Focus line `i` with the caret `at` characters in.
function focusLineAt(i: number, at: number) {
  const el = lyricsEl.querySelector<HTMLElement>(`.line[data-idx="${i}"] .ln-text`);
  if (!el) return;
  el.focus();
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  let left = at;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const len = node.textContent?.length ?? 0;
    if (left <= len) {
      getSelection()?.collapse(node, left);
      return;
    }
    left -= len;
  }
  focusLine(i);
}

/// Pasting a whole song into one line splits it across lines rather than jamming it into
/// one. Without this, a track LRCLIB has nothing for would have no usable path.
function onPaste(e: ClipboardEvent, i: number) {
  const text = e.clipboardData?.getData("text/plain") ?? "";
  if (!text.includes("\n")) return; // ordinary paste, let the browser do it
  e.preventDefault();
  const pasted = parseLines(text);
  lines.splice(i, 1, ...pasted);
  markDirty();
  render();
  focusLine(i + pasted.length - 1);
}

function markDirty() {
  dirty = true;
  refreshBar();
}

// ---- timing ---------------------------------------------------------------

/// Set one line's start to the current playhead. The correction case: you noticed a
/// single line is out, so you fix that line.
function setNow(i: number) {
  if (!ctx || stale || !ctx.canTransport) return;
  lines[i].t = Math.max(0, ctx.playhead().position - TAP_LATENCY);
  markDirty();
  render();
  if (autoCheckBox.checked) checkLine(i);
}

/// Mark during a continuous pass: stamp the cursor line and move on. Deliberately does
/// not seek, verify, or re-render anything but the rows — the song is still playing and
/// the next line is seconds away.
function markAt(i: number) {
  if (!ctx || stale || isBlank(lines[i])) return;
  lines[i].t = Math.max(0, ctx.playhead().position - TAP_LATENCY);
  dirty = true;
  dropMisplacedGaps(i);
  const next = nextLyric(i + 1);
  if (next < 0) {
    // Last line with words: a gap after it (the outro) has to start after it, too.
    for (let j = i + 1; j < lines.length; j++) {
      const t = lines[j].t;
      if (t !== null && t < lines[i].t!) lines[j].t = null;
    }
    stopStamping();
    return;
  }
  cursor = next;
  render();
  showCursor();
}

/// A timed blank line clears the display through an instrumental gap, which is worth
/// keeping, but only while it still falls between the lines around it. Retiming those
/// lines moves the gap's real position; a stamp left behind would sort into the middle of
/// a verse and blank it. So once line `i` is marked, a blank line between it and the
/// lyric before it keeps its time only if it still sits between theirs.
function dropMisplacedGaps(i: number) {
  let prev = i - 1;
  while (prev >= 0 && isBlank(lines[prev])) prev--;
  const lo = prev >= 0 ? (lines[prev].t ?? -Infinity) : -Infinity;
  const hi = lines[i].t!;
  for (let j = prev + 1; j < i; j++) {
    const t = lines[j].t;
    if (t !== null && (t < lo || t > hi)) lines[j].t = null;
  }
}

/// Centre the cursor row, so the next click lands where the last one did. Instant, not
/// smooth: during a pass the cursor moves every few seconds and a smooth scroll would
/// still be gliding when the next line needs marking.
function showCursor() {
  lyricsEl
    .querySelector(`.line[data-idx="${cursor}"]`)
    ?.scrollIntoView({ block: "center", behavior: "auto" });
}

/// A mark that landed wrong is fixed by going back a few seconds and marking again, not
/// by sitting through the whole song. The cursor steps back with the playhead to the
/// first line marked after the new position, so the next press redoes it.
function rewind() {
  if (!ctx || stale || !ctx.canTransport) return;
  const to = Math.max(0, ctx.playhead().position - REWIND);
  ctx.seek(to);
  const redo = lines.findIndex((l, i) => i < cursor && !isBlank(l) && l.t !== null && l.t > to);
  if (redo >= 0) cursor = redo;
  render();
  showCursor();
  setStatus(
    redo >= 0
      ? `Back to ${stampOf(to)}. Mark line ${cursor + 1} again when it starts.`
      : `Back to ${stampOf(to)}.`
  );
}

rewindBtn.addEventListener("click", rewind);

function checkLine(i: number) {
  const t = lines[i].t;
  if (t === null || !ctx || !ctx.canTransport || stale) return;
  ctx.seek(Math.max(0, t - CHECK_LEAD));
  setStatus(`Playing from ${stampOf(Math.max(0, t - CHECK_LEAD))} to check line ${i + 1}.`);
}

function startStamping() {
  if (!ctx || stale || !ctx.canTransport) return;
  stamping = true;
  // Resume where there is work rather than restamping good lines from the top.
  cursor = lines.findIndex((l) => l.t === null && !isBlank(l));
  if (cursor < 0) cursor = nextLyric(0);
  if (cursor < 0) {
    stamping = false;
    setStatus("There are no lines to time yet.", "err");
    return;
  }
  lyricsEl.style.setProperty("--stamp-pad", `${Math.round(lyricsEl.clientHeight / 2)}px`);
  lyricsEl.classList.add("stamping");
  render();
  refreshBar();
  showCursor();
  setStatus(`Space or click a line's time as each line starts. ← goes back ${REWIND}s. Esc stops.`);
  if (ctx.playhead().paused) ctx.transport("play");
}

function stopStamping() {
  stamping = false;
  lyricsEl.classList.remove("stamping");
  render();
  refreshBar();
  // The pass's extra padding just went, so the list moved under the reader. Bring back
  // the line the pass ended on.
  lyricsEl
    .querySelector(`.line[data-idx="${cursor}"]`)
    ?.scrollIntoView({ block: "nearest", behavior: "auto" });
  setStatus(dirty ? "Timing pass done — save it." : "");
}

timestampBtn.addEventListener("click", () => (stamping ? stopStamping() : startStamping()));

// Space marks during a pass. It must not fire while a line's text has focus, or typing a
// space would stamp instead of typing.
window.addEventListener("keydown", (e) => {
  if (!editing) return;
  // An open confirmation takes Escape for itself (see lyric-confirm.ts).
  if (e.key === "Escape") {
    if (stamping) stopStamping();
    return;
  }
  const el = e.target as HTMLElement;
  if (el.isContentEditable || el.tagName === "INPUT") return;
  if (stamping && e.key === " ") {
    e.preventDefault();
    markAt(cursor);
  } else if (stamping && e.key === "ArrowLeft") {
    e.preventDefault();
    rewind();
  }
});

// ---- the action bar -------------------------------------------------------

function setStatus(text: string, kind?: "ok" | "err") {
  statusEl.textContent = text;
  statusEl.classList.toggle("ok", kind === "ok");
  statusEl.classList.toggle("err", kind === "err");
}

function refreshBar() {
  const timed = anyTimed();
  timestampBtn.textContent = stamping ? "Stop timing" : timed ? "Retime all…" : "Add timings…";
  timestampBtn.disabled = busy || stale || !ctx?.canTransport;
  rewindBtn.hidden = !stamping;
  // Checking a line is only meaningful once something has a time, and auto-checking would
  // wreck a continuous pass, so it is offered only outside one.
  autoCheckWrap.hidden = stamping || !timed;
  saveBtn.disabled = busy || !dirty;
  revertBtn.hidden = !ctx?.lyrics.overridden;
  revertBtn.disabled = busy;
  flagBtn.disabled = busy || !ctx?.lyrics.id;
  // A draft goes out only once a person has checked every line of it.
  const left = unreviewed();
  publishBtn.disabled = busy || !publication() || left > 0;
  publishBtn.textContent = left
    ? `${left} ${left === 1 ? "line" : "lines"} left to review`
    : PUBLISH_LABEL;
}

const PUBLISH_LABEL = publishBtn.textContent;

/// Lines of a draft with words that nobody has checked yet.
const unreviewed = () => (reviewing ? unreviewedIn(lines) : 0);

/// What a saved edit records of the review (see `reviewState`).
const draftState = (): Lyrics["draft"] => (reviewing ? reviewState(lines) : null);

function setBusy(on: boolean) {
  busy = on;
  refreshBar();
}

/// The edited track's length. The live playhead only fills in while this track is still
/// the one playing — after that it describes another song, and a publish or lookup keyed
/// on it would land on that song's identity.
const trackDuration = () =>
  ctx?.duration || (ctx && !stale ? ctx.playhead().duration : 0) || null;

const keyArgs = () => ({
  artist: ctx!.meta.artist,
  track: ctx!.meta.title,
  album: ctx!.meta.album || null,
});

/// The edit, in the shape LRCLIB and the cache both want. Lines that never got a time
/// still belong in the plain form.
function edited() {
  const body = serialize(lines).trim();
  if (!body) return { synced: null, plain: null, timing: null };
  return anyTimed()
    ? {
        synced: body,
        plain: lines.map((l) => l.text).join("\n").trim(),
        timing: toTiming(lines),
      }
    : { synced: null, plain: body, timing: null };
}

async function saveOverride(): Promise<Lyrics> {
  const saved = await invoke<Lyrics>("save_lyrics_override", {
    ...keyArgs(),
    lyrics: { ...ctx!.lyrics, ...edited(), draft: draftState(), overridden: true },
  });
  ctx!.lyrics = saved;
  ctx!.onApplied(saved);
  dirty = false;
  refreshBar();
  return saved;
}

saveBtn.addEventListener("click", async () => {
  if (busy || !ctx) return;
  const body = edited();
  if (!body.synced && !body.plain) {
    setStatus("There is nothing to save.", "err");
    return;
  }
  setBusy(true);
  try {
    await saveOverride();
    const left = unreviewed();
    setStatus(
      left
        ? `Saved, with ${left} ${left === 1 ? "line" : "lines"} still to review. The review carries on from here next time.`
        : "Saved. These lyrics are yours until you discard them.",
      "ok"
    );
  } catch (e) {
    setStatus(String(e), "err");
  } finally {
    setBusy(false);
  }
});

revertBtn.addEventListener("click", async () => {
  if (busy || !ctx) return;
  setBusy(true);
  try {
    await invoke("clear_lyrics_override", keyArgs());
    const fresh = await invoke<Lyrics>("fetch_lyrics", {
      ...keyArgs(),
      duration: trackDuration(),
    });
    ctx.lyrics = fresh;
    ctx.onApplied(fresh);
    load(fresh);
    dirty = false;
    render();
    refreshBar();
    setStatus(
      fresh.draft
        ? "Your edit is gone; this is the speech engine's draft again."
        : "Your edit is gone; this is what LRCLIB serves.",
      "ok"
    );
  } catch (e) {
    setStatus(String(e), "err");
  } finally {
    setBusy(false);
  }
});

// ---- publishing -----------------------------------------------------------

/// The edit as LRCLIB would file it, under the record it came from (see `publicationFor`).
const publication = () =>
  ctx ? publicationFor(ctx.lyrics, ctx.meta, trackDuration(), edited()) : null;

publishBtn.addEventListener("click", () => {
  const p = publication();
  if (!p) {
    setStatus("The track's length isn't known yet, and LRCLIB needs it to file lyrics.", "err");
    return;
  }
  showConfirm({
    title: ctx!.lyrics.id ? "Publish this correction?" : "Add these lyrics to LRCLIB?",
    message: ctx!.lyrics.id
      ? `This becomes the version everyone gets for ${p.artistName} — ${p.trackName}. The current one is kept as an earlier revision, so nothing is destroyed.`
      : `This adds ${p.artistName} — ${p.trackName} to the public LRCLIB database, where anyone can fetch it.`,
    go: "Publish",
    run: async () => {
      const saved = await saveOverride(); // the fix should survive a failed publish
      setStatus("Solving LRCLIB's proof-of-work…");
      await invoke("publish_lyrics", { publication: p });
      setStatus("Published to LRCLIB.", "ok");
      await settle(saved);
    },
  });
});

/// Once LRCLIB serves the correction itself the local copy is redundant — drop it, so the
/// pane stops calling itself an edit. If LRCLIB hasn't caught up, keep ours.
/// Whether two LRC texts time the same lines alike. Not a string comparison: LRCLIB rewrites
/// the LRC it derives from a published Lyricsfile in its own spacing (`[00:12.00]Line`, no
/// space), so the text it serves back differs from what was sent while saying the same thing.
function sameLrc(a: string | null, b: string | null) {
  const norm = (lrc: string | null) =>
    parseLines(lrc ?? "")
      .filter((l) => l.t !== null)
      .map((l) => `${Math.round(l.t! * 100)}|${l.text.trim()}`)
      .join("\n");
  return norm(a) === norm(b);
}

async function settle(saved: Lyrics) {
  if (!ctx) return;
  try {
    await invoke("clear_lyrics_override", keyArgs());
    const fresh = await invoke<Lyrics>("fetch_lyrics", {
      ...keyArgs(),
      duration: trackDuration(),
    });
    if (sameLrc(fresh.synced, saved.synced) && (fresh.plain ?? null) === (saved.plain ?? null)) {
      ctx.lyrics = fresh;
      ctx.onApplied(fresh);
      refreshBar();
      setStatus("Published to LRCLIB — it now serves your version.", "ok");
      return;
    }
  } catch {
    // fall through and put the local copy back
  }
  await saveOverride().catch(() => {});
}

flagBtn.addEventListener("click", () => {
  const id = ctx?.lyrics.id;
  if (!id) return;
  showConfirm({
    title: "Report these lyrics?",
    message:
      "Tells LRCLIB the published lyrics for this track are wrong — the right move when they belong to a different song entirely and there is nothing to correct by hand.",
    go: "Report",
    reason: true,
    run: async (reason) => {
      await invoke("flag_lyrics", {
        trackId: id,
        content: reason || null,
        ...keyArgs(),
      });
      setStatus("Reported to LRCLIB.", "ok");
    },
  });
});

// ---- confirmation ---------------------------------------------------------

/// Ask first (see lyric-confirm.ts), then run the action with the bar busy and its outcome
/// in the status line. `run` gets the reason typed, when one was asked for.
async function showConfirm(opts: {
  title: string;
  message: string;
  go: string;
  reason?: boolean;
  run: (reason: string) => Promise<void>;
}) {
  const reason = await confirm.ask(opts);
  if (reason === null || busy || !editing) return;
  setBusy(true);
  setStatus("Working…");
  try {
    await opts.run(reason);
  } catch (e) {
    setStatus(String(e), "err");
  } finally {
    setBusy(false);
  }
}
