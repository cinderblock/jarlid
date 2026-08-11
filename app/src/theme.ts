// Light / dark / follow-the-system.
//
// Two things have to agree: the page (a `data-theme` attribute on <html>, which
// every rule in styles.css keys off) and the window frame, which is drawn by
// Windows and only listens to `setTheme`. Doing one without the other leaves a
// dark title bar on a white app.
//
// `data-theme` is always the RESOLVED theme — "system" is answered here rather
// than in CSS, so the stylesheet never has to say anything twice.

import { getCurrentWindow } from "@tauri-apps/api/window";

export type ThemePref = "system" | "dark" | "light";

/** Mirrors the stored preference so index.html can paint before Rust answers. */
const CACHE_KEY = "jarlid.theme";

const dark = matchMedia("(prefers-color-scheme: dark)");

let pref: ThemePref = readCache();

export function themePref(): ThemePref {
  return pref;
}

/** What `pref` actually means right now. */
function resolve(): "dark" | "light" {
  if (pref !== "system") return pref;
  return dark.matches ? "dark" : "light";
}

function paint() {
  document.documentElement.dataset.theme = resolve();
}

export function setTheme(next: ThemePref) {
  pref = next;
  try {
    localStorage.setItem(CACHE_KEY, next);
  } catch {
    // Private mode, quota, a locked profile — the cache is an optimisation, and
    // settings.json is what actually remembers this.
  }
  paint();
  // The frame follows the same choice. `null` hands the decision back to Windows,
  // which is also what makes `prefers-color-scheme` inside the webview meaningful
  // again — so repaint once it has, rather than trusting the stale value.
  try {
    void getCurrentWindow()
      .setTheme(next === "system" ? null : next)
      .then(paint)
      .catch(() => {});
  } catch {
    // No Tauri window behind us — a plain browser during development. The page
    // still themes itself correctly; only the frame is missing.
  }
}

function readCache(): ThemePref {
  try {
    const v = localStorage.getItem(CACHE_KEY);
    if (v === "dark" || v === "light" || v === "system") return v;
  } catch {
    // See above — fall through to the default.
  }
  return "system";
}

// Windows can switch its app colour on a schedule; "system" has to mean it.
dark.addEventListener("change", () => {
  if (pref === "system") paint();
});

paint();
