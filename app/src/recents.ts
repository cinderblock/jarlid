// Which end of the recently-played strip the newest song sits at.
//
// Shaped like theme.ts, and for the same reason: settings.json is what actually
// remembers this, but the strip is drawn on the first frame — before Rust has
// answered — so a copy lives in `localStorage` purely to get that frame the right
// way round. Without it the newest track would visibly jump from one end to the
// other a moment after the window opens.
//
// Only the *rendering* is reversed. The stored history array stays newest-first,
// because a setting that rewrote it would be destructive: applied twice, or
// applied to a list some older build had already turned round, it would leave the
// history genuinely out of order with no way to tell.

export type RecentsOrder = "newestRight" | "newestLeft";

/** Mirrors the stored preference so the first paint is not the wrong way round. */
const CACHE_KEY = "jarlid.recentsOrder";

/** Fired when the direction changes, so whoever draws the strip can redraw it. */
export const RECENTS_ORDER_CHANGED = "jarlid:recents-order";

let pref: RecentsOrder = readCache();

export function recentsOrder(): RecentsOrder {
  return pref;
}

export function setRecentsOrder(next: RecentsOrder) {
  if (next === pref) return;
  pref = next;
  try {
    localStorage.setItem(CACHE_KEY, next);
  } catch {
    // Private mode, quota, a locked profile — the cache is an optimisation, and
    // settings.json is what actually remembers this.
  }
  window.dispatchEvent(new CustomEvent(RECENTS_ORDER_CHANGED));
}

function readCache(): RecentsOrder {
  try {
    const v = localStorage.getItem(CACHE_KEY);
    if (v === "newestRight" || v === "newestLeft") return v;
  } catch {
    // See above — fall through to the default.
  }
  // The Pandora app's direction, and what a fresh settings.json says.
  return "newestRight";
}
