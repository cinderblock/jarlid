// The lyric editor: fix wrong words, tap timings onto lyrics that only exist as plain
// text, keep the result locally, and optionally send the correction back to LRCLIB.
//
// Why this lives in the app rather than being a link to a web form: LRCLIB's own site
// has no editor, and the third-party publish forms can only be prefilled with metadata,
// never with the lyrics body — so "fix one word" would mean retyping the song. More to
// the point, adding timings needs the playhead, which only the player has.

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
  /// False when a network renderer owns playback: `transport` drives the local engine,
  /// so its buttons would restart a track nobody is listening to here.
  canTransport: boolean;
  /// Hand replacement lyrics back to the now-playing screen.
  onApplied: (lyrics: Lyrics) => void;
}

/// Taps land late — you hear the line, then press. Shifting stamps earlier by about a
/// reaction time is what makes a hand-tapped file feel right rather than consistently
/// behind.
const TAP_LATENCY = 0.33;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const page = $("lyric-page");
const trackEl = $("le-track");
const textEl = $<HTMLTextAreaElement>("le-text");
const wordsNote = $("le-words-note");
const wordsPane = $("le-words");
const timingPane = $("le-timing");
const linesEl = $("le-lines");
const clockEl = $("le-clock");
const statusEl = $("le-status");
const saveBtn = $<HTMLButtonElement>("le-save");
const publishBtn = $<HTMLButtonElement>("le-publish");
const flagBtn = $<HTMLButtonElement>("le-flag");
const revertBtn = $<HTMLButtonElement>("le-revert");
const stampBtn = $<HTMLButtonElement>("le-stamp");
const backBtn = $<HTMLButtonElement>("le-back");
const playPauseBtn = $<HTMLButtonElement>("le-playpause");
const restartBtn = $<HTMLButtonElement>("le-restart");
const confirmEl = $("le-confirm");
const confirmTitle = $("le-confirm-title");
const confirmMsg = $("le-confirm-msg");
const confirmGo = $<HTMLButtonElement>("le-confirm-go");
const reasonEl = $<HTMLInputElement>("le-reason");

const timingTab = document.querySelector<HTMLInputElement>(
  'input[name="le-mode"][value="timing"]'
)!;

let ctx: EditorContext | null = null;
let cursor = 0;
let clockTimer: number | undefined;
let busy = false;
/// Set when playback has moved off the track being edited — see notePlaybackMoved.
let stale = false;

/** True while the editor owns the screen — the player's own key bindings stand down. */
export function isOpen() {
  return !page.hidden;
}

// ---- LRC text <-> lines ---------------------------------------------------

interface Line {
  t: number | null;
  text: string;
}

const STAMP = /^\s*\[(\d+):(\d+(?:[.:]\d+)?)\]\s?/;

function parseLines(text: string): Line[] {
  return text.split(/\r?\n/).map((raw) => {
    const m = STAMP.exec(raw);
    if (!m) return { t: null, text: raw.trim() };
    return {
      t: parseInt(m[1], 10) * 60 + parseFloat(m[2].replace(":", ".")),
      text: raw.slice(m[0].length),
    };
  });
}

function stamp(t: number) {
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  return `[${String(m).padStart(2, "0")}:${s.toFixed(2).padStart(5, "0")}]`;
}

function serialize(lines: Line[]) {
  return lines.map((l) => (l.t === null ? l.text : `${stamp(l.t)} ${l.text}`)).join("\n");
}

/** The plain form of an LRC body — LRCLIB stores both, and they should agree. */
function stripStamps(text: string) {
  return text
    .split(/\r?\n/)
    .map((l) => {
      // A line can carry several stamps when the same words recur; strip the lot.
      let out = l;
      while (STAMP.test(out)) out = out.replace(STAMP, "");
      return out;
    })
    .join("\n");
}

const hasStamps = (text: string) => parseLines(text).some((l) => l.t !== null);

// ---- opening / closing ----------------------------------------------------

export function open(context: EditorContext) {
  ctx = context;
  busy = false;
  const { meta, lyrics } = context;

  textEl.value = lyrics.synced || lyrics.plain || "";
  cursor = 0;

  trackEl.textContent = lyrics.id
    ? `LRCLIB #${lyrics.id} · ${lyrics.artistName} — ${lyrics.trackName}`
    : `${meta.artist} — ${meta.title} · not in LRCLIB yet`;

  wordsNote.textContent = lyrics.synced
    ? "Each line keeps its own timestamp, so fixing a word cannot knock the timing out of step."
    : lyrics.plain
      ? "These lyrics have no timings. Add them on the Timing tab and the result can be published back as a synced version."
      : "Nothing was found for this track. Paste the words in, and add timings if you want to.";

  revertBtn.hidden = !lyrics.overridden;
  stale = false;
  refreshTiming();
  setMode("words");
  setBusy(false);
  setStatus(lyrics.overridden ? "Showing your local edit." : "");

  page.hidden = false;
  hideConfirm();
  clockTimer = window.setInterval(tickClock, 200);
  textEl.focus();
}

export function close() {
  page.hidden = true;
  window.clearInterval(clockTimer);
  ctx = null;
}

/// Playback has moved to another track while the editor is open. The words on screen
/// still belong to the track that was opened, and still save to it — but the playhead
/// does not any more, so stamping against it would write times from a different song,
/// and "Restart track" would restart one nobody opened.
export function notePlaybackMoved() {
  if (!isOpen() || stale || !ctx) return;
  stale = true;
  setMode("words");
  refreshTiming();
  setStatus(
    `Playback moved on. These words still save to ${ctx.meta.artist} — ${ctx.meta.title}, but timing needs the track playing.`,
    "err"
  );
}

/// Timing needs a playhead that belongs to these lyrics, and a local engine to drive.
function refreshTiming() {
  const usable = !stale && !!ctx?.canTransport;
  for (const b of [playPauseBtn, restartBtn]) b.disabled = !usable;
  for (const b of [stampBtn, backBtn]) b.disabled = stale;
  timingTab.disabled = stale;
}

$("le-close").addEventListener("click", close);

// ---- mode tabs ------------------------------------------------------------

function setMode(mode: "words" | "timing") {
  const timing = mode === "timing";
  wordsPane.hidden = timing;
  timingPane.hidden = !timing;
  const radio = document.querySelector<HTMLInputElement>(
    `input[name="le-mode"][value="${mode}"]`
  );
  if (radio) radio.checked = true;
  if (timing) {
    // Start where there is work to do, so opening the tab on an already-timed file
    // doesn't park the cursor on line 1 waiting to overwrite a good stamp.
    const lines = parseLines(textEl.value);
    const next = lines.findIndex((l) => l.t === null && l.text !== "");
    cursor = next === -1 ? 0 : next;
    renderLines();
  }
}

for (const radio of document.querySelectorAll<HTMLInputElement>('input[name="le-mode"]')) {
  radio.addEventListener("change", () => {
    if (radio.checked) setMode(radio.value as "words" | "timing");
  });
}

// ---- timing ---------------------------------------------------------------

/// Lines are read back out of the textarea every time this pane opens, so a word fixed
/// on the Words tab is never overwritten by a stale copy held here.
function renderLines() {
  const lines = parseLines(textEl.value);
  if (cursor > lines.length) cursor = lines.length;
  linesEl.innerHTML = "";
  lines.forEach((line, i) => {
    const row = document.createElement("div");
    row.className = "le-line";
    if (i === cursor) row.classList.add("at");
    if (line.t !== null) row.classList.add("stamped");

    const time = document.createElement("button");
    time.className = "le-time";
    time.textContent = line.t === null ? "––:––" : stamp(line.t).slice(1, -1);
    // Clicking a row's time is how you resume a pass in the middle after a mistake.
    time.addEventListener("click", () => {
      cursor = i;
      renderLines();
    });

    const words = document.createElement("span");
    words.className = "le-words-cell";
    words.textContent = line.text || " ";

    row.append(time, words);
    linesEl.appendChild(row);
  });
  linesEl.querySelector(".le-line.at")?.scrollIntoView({ block: "center", behavior: "smooth" });
}

function stampCurrent() {
  if (!ctx || stale) return;
  const lines = parseLines(textEl.value);
  if (cursor >= lines.length) return;
  lines[cursor].t = Math.max(0, ctx.playhead().position - TAP_LATENCY);
  textEl.value = serialize(lines);
  cursor++;
  renderLines();
  updatePublishability();
}

function stepBack() {
  const lines = parseLines(textEl.value);
  if (cursor <= 0) return;
  cursor--;
  lines[cursor].t = null;
  textEl.value = serialize(lines);
  renderLines();
}

stampBtn.addEventListener("click", stampCurrent);
backBtn.addEventListener("click", stepBack);
restartBtn.addEventListener("click", () => {
  ctx?.transport("replay");
  cursor = 0;
  renderLines();
});
playPauseBtn.addEventListener("click", () => ctx?.transport("toggle"));

function tickClock() {
  if (!ctx) return;
  const { position, paused } = ctx.playhead();
  const m = Math.floor(position / 60);
  const s = Math.floor(position % 60);
  clockEl.textContent = `${m}:${String(s).padStart(2, "0")}`;
  playPauseBtn.textContent = paused ? "Play" : "Pause";
}

// Keys only mean anything on the timing tab, and never while a text field has focus.
window.addEventListener("keydown", (e) => {
  if (!isOpen()) return;
  if (e.key === "Escape") {
    if (!confirmEl.hidden) hideConfirm();
    else close();
    return;
  }
  const tag = (e.target as HTMLElement).tagName;
  if (tag === "TEXTAREA" || tag === "INPUT") return;
  if (timingPane.hidden) return;
  if (e.key === " ") {
    e.preventDefault();
    stampCurrent();
  } else if (e.key === "Backspace") {
    e.preventDefault();
    stepBack();
  }
});

// ---- saving ---------------------------------------------------------------

function setStatus(text: string, kind?: "ok" | "err") {
  statusEl.textContent = text;
  statusEl.classList.toggle("ok", kind === "ok");
  statusEl.classList.toggle("err", kind === "err");
}

function setBusy(on: boolean) {
  busy = on;
  for (const b of [saveBtn, publishBtn, flagBtn, revertBtn, confirmGo]) b.disabled = on;
  if (!on) {
    flagBtn.disabled = !ctx?.lyrics.id;
    updatePublishability();
  }
}

/** The lyrics as currently edited, in the shape LRCLIB and the cache both want. */
function edited(): { synced: string | null; plain: string | null } {
  const text = textEl.value.trim();
  if (!text) return { synced: null, plain: null };
  return hasStamps(text)
    ? { synced: text, plain: stripStamps(text).trim() }
    : { synced: null, plain: text };
}

function keyArgs() {
  const meta = ctx!.meta;
  return { artist: meta.artist, track: meta.title, album: meta.album || null };
}

async function saveOverride(): Promise<Lyrics> {
  const body = edited();
  const base = ctx!.lyrics;
  const saved = await invoke<Lyrics>("save_lyrics_override", {
    ...keyArgs(),
    lyrics: { ...base, ...body, overridden: true },
  });
  ctx!.lyrics = saved;
  ctx!.onApplied(saved);
  revertBtn.hidden = false;
  return saved;
}

saveBtn.addEventListener("click", async () => {
  if (busy || !ctx) return;
  if (!edited().synced && !edited().plain) {
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
    textEl.value = fresh.synced || fresh.plain || "";
    revertBtn.hidden = true;
    setStatus("Your edit is gone; this is what LRCLIB serves.", "ok");
  } catch (e) {
    setStatus(String(e), "err");
  } finally {
    setBusy(false);
  }
});

// ---- publishing -----------------------------------------------------------

/// LRCLIB identifies a record by track/artist/album/duration together, so all four are
/// required. Prefer the matched record's own wording: publishing under Pandora's
/// spelling would file a *new* record beside the wrong one instead of correcting it.
function publication() {
  if (!ctx) return null;
  const { lyrics, meta } = ctx;
  const duration = lyrics.duration ?? ctx.playhead().duration;
  if (!duration) return null;
  return {
    trackName: lyrics.trackName || meta.title,
    artistName: lyrics.artistName || meta.artist,
    albumName: lyrics.albumName || meta.album || meta.title,
    duration,
    ...edited(),
    ...keyArgs(),
  };
}

function updatePublishability() {
  const p = publication();
  publishBtn.disabled = busy || !p || (!p.synced && !p.plain);
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
      ? `This becomes the version everyone gets for ${p.artistName} — ${p.trackName}. ` +
        `The current one is kept as an earlier revision, so nothing is destroyed.`
      : `This adds ${p.artistName} — ${p.trackName} to the public LRCLIB database, ` +
        `where anyone can fetch it.`,
    go: "Publish",
    run: async () => {
      // Save first: the fix should survive even if the network or the challenge fails.
      const saved = await saveOverride();
      setStatus("Solving LRCLIB's proof-of-work…");
      await invoke("publish_lyrics", { publication: p });
      setStatus("Published to LRCLIB.", "ok");
      await settle(saved);
    },
  });
});

/// Once LRCLIB serves the correction itself, the local copy is redundant — drop it so
/// the pane stops calling itself an edit. If LRCLIB hasn't caught up, keep ours.
async function settle(saved: Lyrics) {
  if (!ctx) return;
  try {
    await invoke("clear_lyrics_override", keyArgs());
    const fresh = await invoke<Lyrics>("fetch_lyrics", {
      ...keyArgs(),
      duration: ctx.playhead().duration || null,
    });
    if ((fresh.synced ?? null) === (saved.synced ?? null) && (fresh.plain ?? null) === (saved.plain ?? null)) {
      ctx.lyrics = fresh;
      ctx.onApplied(fresh);
      revertBtn.hidden = true;
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
      "Tells LRCLIB the published lyrics for this track are wrong — the right move when " +
      "they belong to a different song entirely and there is nothing to correct by hand.",
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

$("le-confirm-cancel").addEventListener("click", hideConfirm);

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
