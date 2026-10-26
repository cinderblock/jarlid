// The lyric editor's timeline: the song drawn left to right, with the lyrics laid on it.
//
// Timing by ear alone means replaying a line until the stamp feels right. The song's
// loudness shows where singing starts and where a verse drops away to the next, and the
// words the speech engine heard (when it has heard the song) show where each one fell. So
// here each lyric line is a block on that picture, and its edges are dragged into place:
// the left edge is when it starts, the right edge when it ends — dragged short of the next
// line, that end is a pause (an empty line, the way LRC clears the display). The selected
// line's words get a lane of their own, each one's start a handle too.
//
// Drawn on a canvas: hundreds of blocks and thousands of loudness bars, redrawn every frame
// while the song plays, are more than DOM rows would take gracefully. Everything it changes
// goes through the pure edits in edit-lines.ts. See plans/lyrics-timeline.md.

import { invoke } from "@tauri-apps/api/core";
import {
  endsInPause,
  isBlank,
  lineEnd,
  setEnd,
  setStart,
  setWordStart,
  wordStarts,
  wordsOf,
  type Line,
} from "./edit-lines";

export interface TimelineHost {
  /// The edited track, as Pandora names it: what the loudness and heard words are fetched for.
  meta(): { title: string; artist: string; album: string };
  lines(): Line[];
  /// The line selected in the list, or -1.
  selected(): number;
  select(i: number): void;
  /// The lines were changed here; repaint the list. `i` is the line that changed.
  changed(i: number): void;
  playhead(): { position: number; duration: number; paused: boolean };
  /// The edited track's length, when known.
  duration(): number | null;
  /// False when the playhead belongs to another song now, or a speaker plays it.
  canSeek(): boolean;
  seek(t: number): void;
}

interface Envelope {
  title: string;
  artist: string;
  rate: number;
  levels: number[];
}
interface HeardWord {
  text: string;
  start: number;
  end: number;
  p: number;
}

const panel = document.getElementById("lyric-timeline") as HTMLElement;
const canvas = document.getElementById("lyric-timeline-canvas") as HTMLCanvasElement;
const g = canvas.getContext("2d")!;

/// Lanes, top to bottom, in CSS pixels.
const LANES = {
  ruler: [0, 16],
  wave: [18, 84],
  heard: [86, 104],
  lines: [108, 136],
  words: [140, 164],
} as const;
const HEIGHT = 168;
/// How near an edge counts as grabbing it.
const GRAB = 6;
/// How near a heard word's edge a dragged edge snaps to it. Alt turns snapping off.
const SNAP = 8;
/// Where the playhead sits while following, as a share of the width.
const FOLLOW_AT = 0.25;
/// How long a scroll or zoom by hand holds the view still before it follows the song again.
const HANDS_OFF_MS = 4000;
/// Below this confidence a heard word is drawn as doubtful, as the drafter marks them.
const DOUBT = 0.8;

let host: TimelineHost | null = null;
let env: Envelope | null = null;
let heard: HeardWord[] = [];
let levelRange: [number, number] = [0, 255];
let status = "";
/// The view: the time at the left edge, and how many pixels a second takes.
let viewStart = 0;
let pps = 70;
let handsOffUntil = 0;
let frame = 0;
let width = 0;

type Drag =
  | { kind: "start"; line: number }
  | { kind: "end"; line: number }
  | { kind: "word"; line: number; word: number }
  | { kind: "pan"; fromX: number; fromStart: number; moved: boolean; t: number };
let drag: Drag | null = null;

export const isOpen = () => !panel.hidden;

export function open(h: TimelineHost) {
  host = h;
  panel.hidden = false;
  status = "Loading the song's waveform…";
  env = null;
  heard = [];
  resize();
  void load(h);
  cancelAnimationFrame(frame);
  frame = requestAnimationFrame(tick);
}

export function close() {
  panel.hidden = true;
  host = null;
  drag = null;
  cancelAnimationFrame(frame);
}

async function load(h: TimelineHost) {
  const meta = h.meta();
  try {
    const [e, words] = await Promise.all([
      invoke<Envelope>("track_envelope"),
      invoke<HeardWord[]>("heard_words", {
        artist: meta.artist,
        track: meta.title,
        album: meta.album || null,
      }),
    ]);
    if (host !== h) return;
    heard = words;
    if (e.title !== meta.title || e.artist !== meta.artist) {
      status = "The waveform needs this song playing.";
      return;
    }
    env = e;
    // Songs are mastered loud: stretch each one's own range to the lane, from its quiet
    // tenth up, or every bar is the same height.
    const sorted = [...e.levels].sort((a, b) => a - b);
    const lo = sorted[Math.floor(sorted.length * 0.1)] ?? 0;
    const hi = sorted[sorted.length - 1] ?? 255;
    levelRange = [lo, Math.max(hi, lo + 1)];
    status = "";
  } catch (err) {
    if (host === h) status = `No waveform: ${err}`;
  }
}

function resize() {
  const dpr = window.devicePixelRatio || 1;
  width = panel.clientWidth;
  canvas.style.width = `${width}px`;
  canvas.style.height = `${HEIGHT}px`;
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(HEIGHT * dpr);
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
}
new ResizeObserver(() => {
  if (isOpen()) resize();
}).observe(panel);

const xOf = (t: number) => (t - viewStart) * pps;
const tOf = (x: number) => viewStart + x / pps;

function songLength() {
  return host?.duration() || (env ? env.levels.length / env.rate : 0) || host?.playhead().duration || 0;
}

// ---- drawing ---------------------------------------------------------------------------------

function tick() {
  if (!host) return;
  const ph = host.playhead();
  // Follow the song unless someone is working the view by hand.
  if (!drag && !ph.paused && host.canSeek() && performance.now() > handsOffUntil) {
    viewStart = ph.position - (width * FOLLOW_AT) / pps;
  }
  draw(ph.position);
  frame = requestAnimationFrame(tick);
}

const css = (name: string) => getComputedStyle(panel).getPropertyValue(name).trim();

function draw(position: number) {
  if (!host) return;
  const colors = {
    fg: css("--fg"),
    dim: css("--dim"),
    faint: css("--faint"),
    accent: css("--accent"),
    soft: css("--accent-soft"),
    line: css("--accent-line"),
    good: css("--good"),
  };
  g.clearRect(0, 0, width, HEIGHT);
  g.font = "11px Inter, 'Segoe UI', system-ui, sans-serif";
  g.textBaseline = "middle";

  drawRuler(colors);
  drawWave(colors);
  drawHeard(colors);
  drawLines(colors);
  drawWords(colors);

  // The playhead, through every lane.
  const x = xOf(position);
  if (host.canSeek() && x >= 0 && x <= width) {
    g.fillStyle = colors.accent;
    g.fillRect(Math.round(x) - 1, 0, 2, HEIGHT);
  }
  if (status) {
    g.fillStyle = colors.dim;
    g.textAlign = "left";
    g.fillText(status, 8, (LANES.wave[0] + LANES.wave[1]) / 2);
  }
}

function drawRuler(c: Record<string, string>) {
  const [top, bottom] = LANES.ruler;
  // A tick every second, a label every few, more sparsely when zoomed out.
  const label = pps >= 60 ? 1 : pps >= 25 ? 5 : pps >= 8 ? 10 : 30;
  const first = Math.floor(viewStart);
  g.textAlign = "left";
  for (let s = first; s <= tOf(width); s++) {
    if (s < 0) continue;
    const x = Math.round(xOf(s)) + 0.5;
    const major = s % label === 0;
    if (!major && pps < 12) continue;
    g.fillStyle = major ? c.dim : c.faint;
    g.fillRect(x, major ? top + 8 : top + 12, 1, major ? bottom - top - 8 : bottom - top - 12);
    if (major) g.fillText(clock(s), x + 3, top + 6);
  }
}

function drawWave(c: Record<string, string>) {
  if (!env) return;
  const [top, bottom] = LANES.wave;
  const mid = (top + bottom) / 2;
  const half = (bottom - top) / 2;
  const [lo, hi] = levelRange;
  g.fillStyle = c.faint;
  for (let x = 0; x < width; x++) {
    // The loudest moment under this pixel, so a short hit isn't averaged away.
    const a = Math.max(0, Math.floor(tOf(x) * env.rate));
    const b = Math.min(env.levels.length, Math.max(a + 1, Math.floor(tOf(x + 1) * env.rate)));
    if (a >= env.levels.length) break;
    let v = 0;
    for (let i = a; i < b; i++) v = Math.max(v, env.levels[i]);
    const h = Math.max(0.5, ((v - lo) / (hi - lo)) * half);
    if (v <= lo) continue;
    g.fillRect(x, mid - h, 1, h * 2);
  }
}

function drawHeard(c: Record<string, string>) {
  const [top, bottom] = LANES.heard;
  const y = (top + bottom) / 2;
  g.textAlign = "left";
  heard.forEach((w, i) => {
    const x = xOf(w.start);
    const room = (i + 1 < heard.length ? xOf(heard[i + 1].start) : xOf(w.end) + 40) - x - 3;
    if (x > width || x + room < 0 || room < 8) return;
    g.fillStyle = c.faint;
    g.fillRect(Math.round(x), top + 2, 1, bottom - top - 4);
    g.fillStyle = w.p < DOUBT ? c.faint : c.dim;
    const text = fit(w.text, room - 3);
    g.fillText(text, x + 3, y);
    if (w.p < DOUBT) dotted(x + 3, bottom - 2, g.measureText(text).width, c.line);
  });
}

function drawLines(c: Record<string, string>) {
  const lines = host!.lines();
  const [top, bottom] = LANES.lines;
  const sel = host!.selected();
  const end = songLength();
  g.textAlign = "left";
  lines.forEach((l, i) => {
    if (l.t === null || isBlank(l)) return;
    const x0 = xOf(l.t);
    const x1 = xOf(lineEnd(lines, i, end));
    if (x1 < 0 || x0 > width) return;
    const selected = i === sel;
    g.fillStyle = c.soft;
    roundRect(x0, top, Math.max(2, x1 - x0 - 1), bottom - top, 5);
    g.fill();
    if (selected) {
      g.strokeStyle = c.accent;
      g.lineWidth = 1.5;
      g.stroke();
    }
    // The handles: the start, and the end — solid when it is a pause of its own, faint when
    // the line simply runs into the next.
    g.fillStyle = c.accent;
    g.fillRect(x0, top, 3, bottom - top);
    g.fillStyle = endsInPause(lines, i) ? c.accent : c.line;
    g.fillRect(x1 - 4, top + 6, 2, bottom - top - 12);
    g.fillStyle = selected ? c.fg : c.dim;
    g.fillText(fit(l.text, x1 - x0 - 14), x0 + 7, (top + bottom) / 2);
  });
}

function drawWords(c: Record<string, string>) {
  const lines = host!.lines();
  const i = host!.selected();
  const [top, bottom] = LANES.words;
  const line = lines[i];
  g.textAlign = "left";
  if (!line || line.t === null || isBlank(line)) {
    g.fillStyle = c.faint;
    g.fillText("Select a line to time its words", 8, (top + bottom) / 2);
    return;
  }
  const words = wordsOf(line.text);
  const starts = wordStarts(lines, i, songLength());
  const end = lineEnd(lines, i, songLength());
  starts.forEach((s, k) => {
    const x0 = xOf(s);
    const x1 = xOf(k + 1 < starts.length ? starts[k + 1] : end);
    if (x1 < 0 || x0 > width) return;
    // A word with a time of its own is solid; one placed by even shares is only outlined.
    const own = k === 0 || line.words?.[k] != null;
    g.fillStyle = own ? c.soft : "transparent";
    g.strokeStyle = c.line;
    g.lineWidth = 1;
    roundRect(x0 + 1, top, Math.max(2, x1 - x0 - 2), bottom - top, 4);
    if (own) g.fill();
    g.setLineDash(own ? [] : [3, 3]);
    g.stroke();
    g.setLineDash([]);
    g.fillStyle = c.accent;
    g.fillRect(x0, top, 2, bottom - top);
    g.fillStyle = c.fg;
    g.fillText(fit(words[k], x1 - x0 - 8), x0 + 5, (top + bottom) / 2);
  });
}

function roundRect(x: number, y: number, w: number, h: number, r: number) {
  g.beginPath();
  g.roundRect(x, y, w, h, Math.min(r, w / 2));
}

function dotted(x: number, y: number, w: number, color: string) {
  g.fillStyle = color;
  for (let d = 0; d < w; d += 3) g.fillRect(x + d, y, 1.5, 1.5);
}

/// `text` cut to fit `room` pixels, with an ellipsis; empty if not even that fits.
function fit(text: string, room: number) {
  if (room < 10) return "";
  if (g.measureText(text).width <= room) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (g.measureText(`${text.slice(0, mid)}…`).width <= room) lo = mid;
    else hi = mid - 1;
  }
  return lo ? `${text.slice(0, lo)}…` : "";
}

function clock(s: number) {
  return `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
}

// ---- interaction -----------------------------------------------------------------------------

/// What is under the pointer: a handle to drag, a block to select, or open time.
function hit(x: number, y: number): Drag | { kind: "select"; line: number } | { kind: "word-click"; t: number } | null {
  if (!host) return null;
  const lines = host.lines();
  const end = songLength();
  if (y >= LANES.lines[0] && y <= LANES.lines[1]) {
    // Where one line runs straight into the next, its end and the next one's start are the
    // same moment. The pointer's side of it decides: just left grabs the end (to open a pause),
    // just right the start — the sides their handles are drawn on.
    for (let i = 0; i < lines.length; i++) {
      const l = lines[i];
      if (l.t === null || isBlank(l)) continue;
      const x0 = xOf(l.t);
      const x1 = xOf(lineEnd(lines, i, end));
      if (x >= x0 - 2 && x <= x0 + GRAB) return { kind: "start", line: i };
      if (x >= x1 - GRAB - 2 && x < x1) return { kind: "end", line: i };
      if (x > x0 && x < x1) return { kind: "select", line: i };
    }
    return null;
  }
  if (y >= LANES.words[0] && y <= LANES.words[1]) {
    const i = host.selected();
    const line = lines[i];
    if (!line || line.t === null || isBlank(line)) return null;
    const starts = wordStarts(lines, i, end);
    for (let k = 0; k < starts.length; k++) {
      if (Math.abs(x - xOf(starts[k])) <= GRAB) {
        return k === 0 ? { kind: "start", line: i } : { kind: "word", line: i, word: k };
      }
    }
    return { kind: "word-click", t: tOf(x) };
  }
  return null;
}

/// `t`, pulled onto the nearest heard word's start (or end, for an end) within reach.
function snap(t: number, toEnd: boolean, free: boolean) {
  if (free || !heard.length) return t;
  let best = t;
  let reach = SNAP / pps;
  for (const w of heard) {
    const at = toEnd ? w.end : w.start;
    if (Math.abs(at - t) < reach) {
      reach = Math.abs(at - t);
      best = at;
    }
  }
  return best;
}

canvas.addEventListener("pointerdown", (e) => {
  if (!host) return;
  const { x, y } = local(e);
  const h = hit(x, y);
  // So a drag that leaves the canvas still ends here. It throws for a pointer that is already
  // gone; the press still counts.
  try {
    canvas.setPointerCapture(e.pointerId);
  } catch {
    // nothing to capture
  }
  if (h?.kind === "start" || h?.kind === "end" || h?.kind === "word") {
    drag = h;
    host.select(h.line);
    return;
  }
  if (h?.kind === "select") {
    // A second click on the selected block plays from just before it, to hear the line.
    const t = host.lines()[h.line].t;
    if (h.line === host.selected() && t !== null && host.canSeek()) host.seek(Math.max(0, t - 1));
    host.select(h.line);
    return;
  }
  if (h?.kind === "word-click") {
    if (host.canSeek()) host.seek(Math.max(0, h.t - 0.3));
    return;
  }
  // Open time: a click seeks there; a drag pans.
  drag = { kind: "pan", fromX: x, fromStart: viewStart, moved: false, t: tOf(x) };
});

canvas.addEventListener("pointermove", (e) => {
  if (!host) return;
  const { x, y } = local(e);
  if (!drag) {
    const h = hit(x, y);
    canvas.style.cursor =
      h?.kind === "start" || h?.kind === "end" || h?.kind === "word"
        ? "ew-resize"
        : h
          ? "pointer"
          : "grab";
    return;
  }
  const lines = host.lines();
  const t = tOf(x);
  const free = e.altKey;
  switch (drag.kind) {
    case "pan":
      if (Math.abs(x - drag.fromX) > 3) drag.moved = true;
      if (drag.moved) {
        viewStart = drag.fromStart - (x - drag.fromX) / pps;
        handsOffUntil = performance.now() + HANDS_OFF_MS;
        canvas.style.cursor = "grabbing";
      }
      return;
    case "start":
      setStart(lines, drag.line, snap(t, false, free));
      break;
    case "end":
      setEnd(lines, drag.line, snap(t, true, free));
      break;
    case "word":
      setWordStart(lines, drag.line, drag.word, snap(t, false, free), songLength());
      break;
  }
  handsOffUntil = performance.now() + HANDS_OFF_MS;
  host.changed(drag.line);
});

canvas.addEventListener("pointerup", () => {
  if (drag?.kind === "pan" && !drag.moved && host?.canSeek()) host.seek(Math.max(0, drag.t));
  drag = null;
});

// The wheel scrolls through the song; with Ctrl (or a pinch) it zooms about the pointer.
canvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const { x } = local(e);
    if (e.ctrlKey) {
      const at = tOf(x);
      pps = Math.min(600, Math.max(4, pps * Math.exp(-e.deltaY / 300)));
      viewStart = at - x / pps;
    } else {
      viewStart += (e.deltaX || e.deltaY) / pps;
    }
    handsOffUntil = performance.now() + HANDS_OFF_MS;
  },
  { passive: false }
);

function local(e: MouseEvent) {
  const r = canvas.getBoundingClientRect();
  return { x: e.clientX - r.left, y: e.clientY - r.top };
}

/// Bring line `i` into view, unless it already is.
export function reveal(i: number) {
  const lines = host?.lines();
  const t = lines?.[i]?.t;
  if (t == null) return;
  const x = xOf(t);
  if (x < 20 || x > width - 60) {
    viewStart = t - (width * FOLLOW_AT) / pps;
    handsOffUntil = performance.now() + HANDS_OFF_MS;
  }
}
