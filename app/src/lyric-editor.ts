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
}

export interface EditorContext {
  /// Pandora's naming for the track. Also the key local edits are filed under.
  meta: { title: string; artist: string; album: string };
  lyrics: Lyrics;
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

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const lyricsEl = $("lyrics");
const bar = $("lyric-bar");
const statusEl = $("lyric-bar-status");
const timestampBtn = $<HTMLButtonElement>("lyric-timestamp");
const autoCheckWrap = $("lyric-autocheck-wrap");
const autoCheckBox = $<HTMLInputElement>("lyric-autocheck");
const saveBtn = $<HTMLButtonElement>("lyric-save");
const revertBtn = $<HTMLButtonElement>("lyric-revert");
const publishBtn = $<HTMLButtonElement>("lyric-publish");
const flagBtn = $<HTMLButtonElement>("lyric-flag");
const doneBtn = $<HTMLButtonElement>("lyric-done");
const confirmEl = $("lyric-confirm");
const confirmTitle = $("lyric-confirm-title");
const confirmMsg = $("lyric-confirm-msg");
const confirmGo = $<HTMLButtonElement>("lyric-confirm-go");
const reasonEl = $<HTMLInputElement>("lyric-reason");

interface Line {
  t: number | null;
  text: string;
}

let ctx: EditorContext | null = null;
let lines: Line[] = [];
let editing = false;
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

// ---- entering / leaving ---------------------------------------------------

export function begin(context: EditorContext) {
  ctx = context;
  editing = true;
  dirty = false;
  stale = false;
  busy = false;
  stamping = false;
  cursor = 0;

  const src = context.lyrics.synced || context.lyrics.plain || "";
  lines = src ? parseLines(src) : [{ t: null, text: "" }];

  lyricsEl.classList.add("editing");
  bar.hidden = false;
  render();
  refreshBar();
}

export function end() {
  editing = false;
  stamping = false;
  lyricsEl.classList.remove("editing", "stamping");
  bar.hidden = true;
  confirmEl.hidden = true;
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
    if (line.t !== null) row.classList.add("timed");

    const time = document.createElement("button");
    time.className = "ln-time";
    time.textContent = line.t === null ? "––:––" : stampOf(line.t);
    time.addEventListener("click", () => (stamping ? markAt(i) : setNow(i)));

    const text = document.createElement("div");
    text.className = "ln-text";
    text.contentEditable = "plaintext-only";
    text.spellcheck = false;
    text.textContent = line.text;
    text.addEventListener("input", () => {
      lines[i].text = text.textContent ?? "";
      markDirty();
    });
    text.addEventListener("paste", (e) => onPaste(e, i));
    text.addEventListener("keydown", (e) => onLineKey(e, i, text));

    const check = document.createElement("button");
    check.className = "ln-check";
    check.textContent = "▶";
    check.disabled = line.t === null || !ctx?.canTransport || stale;
    check.addEventListener("click", () => checkLine(i));

    row.append(time, text, check);
    lyricsEl.appendChild(row);
  });
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

/// Enter opens a new line below, Backspace on an empty one removes it. This is what
/// replaces the old full-page textarea: adding and removing lines has to be possible
/// without a separate editor, or "no lyrics found" is a dead end.
function onLineKey(e: KeyboardEvent, i: number, el: HTMLElement) {
  if (e.key === "Enter") {
    e.preventDefault();
    lines[i].text = el.textContent ?? "";
    lines.splice(i + 1, 0, { t: null, text: "" });
    markDirty();
    render();
    focusLine(i + 1);
  } else if (e.key === "Backspace" && !el.textContent && lines.length > 1) {
    e.preventDefault();
    lines.splice(i, 1);
    if (cursor > i) cursor--;
    markDirty();
    render();
    focusLine(Math.max(0, i - 1));
  }
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
  if (!ctx || stale) return;
  lines[i].t = Math.max(0, ctx.playhead().position - TAP_LATENCY);
  cursor = Math.min(i + 1, lines.length - 1);
  dirty = true;
  render();
  // Instant, not smooth: during a pass the cursor moves every few seconds and a smooth
  // scroll would still be gliding when the next line needs marking.
  lyricsEl
    .querySelector(`.line[data-idx="${cursor}"]`)
    ?.scrollIntoView({ block: "center", behavior: "auto" });
  if (cursor === lines.length - 1 && lines[cursor].t !== null) stopStamping();
}

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
  cursor = lines.findIndex((l) => l.t === null && l.text);
  if (cursor < 0) cursor = 0;
  lyricsEl.classList.add("stamping");
  render();
  refreshBar();
  setStatus("Space or click a line's time as each line starts. Esc stops.");
  if (ctx.playhead().paused) ctx.transport("play");
}

function stopStamping() {
  stamping = false;
  lyricsEl.classList.remove("stamping");
  render();
  refreshBar();
  setStatus(dirty ? "Timing pass done — save it." : "");
}

timestampBtn.addEventListener("click", () => (stamping ? stopStamping() : startStamping()));

// Space marks during a pass. It must not fire while a line's text has focus, or typing a
// space would stamp instead of typing.
window.addEventListener("keydown", (e) => {
  if (!editing) return;
  if (e.key === "Escape") {
    if (!confirmEl.hidden) hideConfirm();
    else if (stamping) stopStamping();
    return;
  }
  const el = e.target as HTMLElement;
  if (el.isContentEditable || el.tagName === "INPUT") return;
  if (stamping && e.key === " ") {
    e.preventDefault();
    markAt(cursor);
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
  // Checking a line is only meaningful once something has a time, and auto-checking would
  // wreck a continuous pass, so it is offered only outside one.
  autoCheckWrap.hidden = stamping || !timed;
  saveBtn.disabled = busy || !dirty;
  revertBtn.hidden = !ctx?.lyrics.overridden;
  revertBtn.disabled = busy;
  flagBtn.disabled = busy || !ctx?.lyrics.id;
  publishBtn.disabled = busy || !publication();
}

function setBusy(on: boolean) {
  busy = on;
  refreshBar();
  confirmGo.disabled = on;
}

const keyArgs = () => ({
  artist: ctx!.meta.artist,
  track: ctx!.meta.title,
  album: ctx!.meta.album || null,
});

/// The edit, in the shape LRCLIB and the cache both want. Lines that never got a time
/// still belong in the plain form.
function edited() {
  const body = serialize(lines).trim();
  if (!body) return { synced: null, plain: null };
  return anyTimed()
    ? { synced: body, plain: lines.map((l) => l.text).join("\n").trim() }
    : { synced: null, plain: body };
}

async function saveOverride(): Promise<Lyrics> {
  const saved = await invoke<Lyrics>("save_lyrics_override", {
    ...keyArgs(),
    lyrics: { ...ctx!.lyrics, ...edited(), overridden: true },
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
    setStatus("Saved. These lyrics are yours until you discard them.", "ok");
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
      duration: ctx.playhead().duration || null,
    });
    ctx.lyrics = fresh;
    ctx.onApplied(fresh);
    lines = parseLines(fresh.synced || fresh.plain || "");
    if (!lines.length) lines = [{ t: null, text: "" }];
    dirty = false;
    render();
    refreshBar();
    setStatus("Your edit is gone; this is what LRCLIB serves.", "ok");
  } catch (e) {
    setStatus(String(e), "err");
  } finally {
    setBusy(false);
  }
});

// ---- publishing -----------------------------------------------------------

/// LRCLIB identifies a record by track/artist/album/duration together, so all four are
/// required and must be the matched record's own wording — publishing under Pandora's
/// spelling files a new record beside the wrong one instead of correcting it.
function publication() {
  if (!ctx) return null;
  const { lyrics, meta } = ctx;
  const duration = lyrics.duration ?? ctx.playhead().duration;
  const body = edited();
  if (!duration || (!body.synced && !body.plain)) return null;
  return {
    trackName: lyrics.trackName || meta.title,
    artistName: lyrics.artistName || meta.artist,
    albumName: lyrics.albumName || meta.album || meta.title,
    duration,
    ...body,
    ...keyArgs(),
  };
}

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
async function settle(saved: Lyrics) {
  if (!ctx) return;
  try {
    await invoke("clear_lyrics_override", keyArgs());
    const fresh = await invoke<Lyrics>("fetch_lyrics", {
      ...keyArgs(),
      duration: ctx.playhead().duration || null,
    });
    if (
      (fresh.synced ?? null) === (saved.synced ?? null) &&
      (fresh.plain ?? null) === (saved.plain ?? null)
    ) {
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
    run: async () => {
      await invoke("flag_lyrics", { trackId: id, content: reasonEl.value || null });
      setStatus("Reported to LRCLIB.", "ok");
    },
  });
});

// ---- confirmation ---------------------------------------------------------

let pending: (() => Promise<void>) | null = null;

function showConfirm(opts: {
  title: string;
  message: string;
  go: string;
  reason?: boolean;
  run: () => Promise<void>;
}) {
  confirmTitle.textContent = opts.title;
  confirmMsg.textContent = opts.message;
  confirmGo.textContent = opts.go;
  reasonEl.hidden = !opts.reason;
  reasonEl.value = "";
  pending = opts.run;
  confirmEl.hidden = false;
  (opts.reason ? reasonEl : confirmGo).focus();
}

function hideConfirm() {
  confirmEl.hidden = true;
  pending = null;
}

$("lyric-confirm-cancel").addEventListener("click", hideConfirm);

confirmGo.addEventListener("click", async () => {
  const run = pending;
  if (!run || busy) return;
  hideConfirm();
  setBusy(true);
  setStatus("Working…");
  try {
    await run();
  } catch (e) {
    setStatus(String(e), "err");
  } finally {
    setBusy(false);
  }
});
