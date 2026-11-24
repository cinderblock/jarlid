// The visualizer in the main window: in the lyrics pane for a song with no lyrics, or behind the
// whole window. Drawn by visualizer-render.ts; main.ts decides when each shows. The visualizer's
// own window is visualizer-window.ts.

import { VisualizerView } from "./visualizer-render";

// ---- the preferences ---------------------------------------------------------------
// Shaped like recents.ts: settings.json remembers them, and a `localStorage` copy lets the first
// song after launch get them right before Rust has answered.

export interface MainPrefs {
  /** In the lyrics pane, for a song with no lyrics. */
  noLyrics: boolean;
  /** Behind the whole window. */
  background: boolean;
}

const CACHE_KEY = "jarlid.visualizer";
/** Fired when the preferences change, so main.ts can show or drop the views. */
export const VISUALIZER_PREF_CHANGED = "jarlid:visualizer-pref";
/** What a fresh settings.json says. */
const DEFAULTS: MainPrefs = { noLyrics: true, background: false };

let prefs = readCache();

export const visualizerPrefs = (): MainPrefs => prefs;

export function setVisualizerPrefs(next: MainPrefs) {
  if (next.noLyrics === prefs.noLyrics && next.background === prefs.background) return;
  prefs = { noLyrics: next.noLyrics, background: next.background };
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(prefs));
  } catch {
    // Private mode, quota, a locked profile — settings.json is what actually remembers this.
  }
  window.dispatchEvent(new CustomEvent(VISUALIZER_PREF_CHANGED));
}

function readCache(): MainPrefs {
  try {
    const v = JSON.parse(localStorage.getItem(CACHE_KEY) ?? "null");
    if (typeof v?.noLyrics === "boolean" && typeof v?.background === "boolean") return v;
  } catch {
    // Unreadable, or an older build's plain "on"/"off": the defaults stand until Rust answers.
  }
  return { ...DEFAULTS };
}

// ---- the views -----------------------------------------------------------------------

const $ = (id: string) => document.getElementById(id) as HTMLElement;

/** Over the lyrics pane. */
export const pane = new VisualizerView(
  $("visualizer-canvas") as HTMLCanvasElement,
  $("visualizer"),
  $("visualizer-msg"),
  "pane"
);

/** Behind the whole window, under the UI. */
export const behind = new VisualizerView(
  $("visualizer-bg-canvas") as HTMLCanvasElement,
  $("visualizer-bg"),
  $("visualizer-bg-msg"),
  "behind"
);
