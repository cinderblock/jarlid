// The visualizer's own window (visualizer.html): the picture filling it, full screen on a
// double-click or F11, and optionally the lyrics over it like subtitles, a line or a word at a
// time.
//
// Rust opens and closes this window from the visualizer settings (visualizer.rs). It has no
// lyrics or playhead of its own: the main window sends both (`sendLyrics` / `sendPlayhead` in
// main.ts), and this asks for them once it is listening.

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { VisualizerView } from "./visualizer-render";
import { lineAt, lineText, wordAt, type SubLine } from "./subtitles";

type Subtitles = "off" | "lines" | "words";
/** The parts of `settings::Settings` this window reads and writes. */
interface Settings {
  visualizer: { subtitles: Subtitles };
  [rest: string]: unknown;
}

const $ = (id: string) => document.getElementById(id) as HTMLElement;
const subtitlesEl = $("subtitles");
const hintEl = $("hint");

const view = new VisualizerView($("canvas") as HTMLCanvasElement, $("stage"), $("msg"), "screen");
view.setShown(true);

// ---- full screen -------------------------------------------------------------------

const win = getCurrentWindow();

async function setFullscreen(on: boolean) {
  try {
    await win.setFullscreen(on);
  } catch (e) {
    console.error("could not change full screen", e);
    hint(`Full screen failed: ${e}`);
  }
}

document.addEventListener("dblclick", async () => setFullscreen(!(await win.isFullscreen())));
document.addEventListener("keydown", async (e) => {
  if (e.key === "F11") {
    e.preventDefault();
    await setFullscreen(!(await win.isFullscreen()));
  } else if (e.key === "Escape") {
    await setFullscreen(false);
  } else if (e.key === "l" || e.key === "L") {
    await cycleSubtitles();
  }
});

// The pointer hides once it rests, so it doesn't sit on the picture.
let stillTimer: ReturnType<typeof setTimeout> | null = null;
document.addEventListener("mousemove", () => {
  document.body.classList.remove("still");
  if (stillTimer !== null) clearTimeout(stillTimer);
  stillTimer = setTimeout(() => document.body.classList.add("still"), 2000);
});

let hintTimer: ReturnType<typeof setTimeout> | null = null;
function hint(text: string) {
  hintEl.textContent = text;
  hintEl.classList.add("show");
  if (hintTimer !== null) clearTimeout(hintTimer);
  hintTimer = setTimeout(() => hintEl.classList.remove("show"), 1800);
}

// ---- the subtitles setting ----------------------------------------------------------

let mode: Subtitles = "lines";
const MODES: Subtitles[] = ["off", "lines", "words"];
const MODE_NAMES: Record<Subtitles, string> = { off: "off", lines: "a line at a time", words: "a word at a time" };

function setMode(next: Subtitles) {
  if (next === mode) return;
  mode = next;
  shownKey = null;
  clearSubtitle();
}

/** L: the next mode, saved like any setting so the Settings page and the next launch agree. */
async function cycleSubtitles() {
  try {
    const s = await invoke<Settings>("get_settings");
    const next = MODES[(MODES.indexOf(s.visualizer.subtitles) + 1) % MODES.length];
    s.visualizer.subtitles = next;
    await invoke("set_settings", { settings: s });
    setMode(next);
    hint(`Lyrics: ${MODE_NAMES[next]}`);
  } catch (e) {
    console.error("could not change the lyrics setting", e);
    hint(`Couldn't change the lyrics setting: ${e}`);
  }
}

void listen<Settings>("settings://changed", (e) => setMode(e.payload.visualizer.subtitles));
invoke<Settings>("get_settings")
  .then((s) => setMode(s.visualizer.subtitles))
  .catch((e) => console.error("could not read the settings", e));

// ---- the lyrics ---------------------------------------------------------------------

let lines: SubLine[] | null = null;
/** The playhead as last sent, and the wall-clock time it was true at. */
let playhead = { position: 0, playing: false, at: Date.now() };

// Both arrive on change only, so once this is listening, ask for the current ones.
Promise.all([
  listen<{ lines: SubLine[] | null }>("lyrics://set", (e) => {
    lines = e.payload.lines;
    shownKey = null;
  }),
  listen<typeof playhead>("lyrics://playhead", (e) => {
    playhead = e.payload;
  }),
])
  .then(() => emit("lyrics://wanted"))
  .catch((e) => console.error("could not listen for the lyrics", e));

/** The song position now, carried forward from the last report while playing. Capped, so a
 *  main window that stops reporting doesn't run the words away. */
function position() {
  if (!playhead.playing) return playhead.position;
  return playhead.position + Math.min(1, (Date.now() - playhead.at) / 1000);
}

/** What is on screen: a key for the line or word, so a change is a new element. */
let shownKey: string | null = null;

function show(key: string, text: string, kind: "line" | "word") {
  if (key === shownKey) return;
  shownKey = key;
  clearSubtitle();
  if (!text) return;
  const el = document.createElement("div");
  el.className = kind;
  el.textContent = text;
  subtitlesEl.appendChild(el);
}

/** Fade out whatever is showing, then drop it. */
function clearSubtitle() {
  for (const el of subtitlesEl.children) {
    if (el.classList.contains("gone")) continue;
    el.classList.add("gone");
    setTimeout(() => el.remove(), 400);
  }
}

function followLyrics() {
  requestAnimationFrame(followLyrics);
  if (mode === "off" || !lines) {
    show("", "", "line");
    return;
  }
  const pos = position();
  if (mode === "lines") {
    const text = lineText(lines, pos);
    show(text ? `l:${lineAt(lines, pos)}` : "", text, "line");
  } else {
    const w = wordAt(lines, pos);
    show(w ? `w:${w.key}` : "", w?.word ?? "", "word");
  }
}
requestAnimationFrame(followLyrics);
