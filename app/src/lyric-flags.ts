// Flagging lyrics as wrong, in the pane, while the song plays.
//
// Two ways in. `F` flags the line playing now and nothing else happens: no layer, no dialog,
// the song carries on. The flag button opens flag mode, where the pane keeps playing and
// highlighting but its rows become clickable: click a row to flag it, shift-click to stretch
// the selected flag to there, click a flagged row to pick its flag and say what's wrong with
// it. Every change is saved as it is made.
//
// Flags are kept per track by the backend (`lyric_flags.rs`), each with a copy of the lyrics
// it was made on. A flag made on other words (the lyrics have been edited or revised since)
// isn't drawn on these rows; the bar counts it and Settings lists it.

import { invoke } from "@tauri-apps/api/core";
import type { Lyrics } from "./lyric-editor";
import {
  TAGS,
  addMerged,
  appliesTo,
  describe,
  quickRows,
  shownOf,
  spanOf,
  type Flag,
  type Row,
  type TrackFlags,
  FLAGS_CHANGED,
} from "./flags";

export interface FlagHost {
  /// `${title}|${artist}` of the track playing now, as main.ts keys it.
  currentKey: () => string;
  /// Seconds on the lyrics' clock now: the playhead plus the track's sync offset.
  now: () => number;
  /// The track's sync offset, seconds.
  offset: () => number;
  /// Whether something else owns the pane (the editor, the version picker).
  blocked: () => boolean;
  /// Whether a page is over the player, so a key press isn't meant for the lyrics.
  covered: () => boolean;
  /// Say something briefly in the pane's status line.
  flash: (text: string) => void;
}

type Meta = { title: string; artist: string; album: string };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const lyricsEl = $("lyrics");
const btn = $<HTMLButtonElement>("lyrics-flag");
const bar = $("lyric-flag-bar");
const infoEl = $("lyric-flag-info");
const tagsEl = $("lyric-flag-tags");
const noteEl = $<HTMLInputElement>("lyric-flag-note");
const removeBtn = $<HTMLButtonElement>("lyric-flag-remove");
const wholeBtn = $<HTMLButtonElement>("lyric-flag-whole");
const doneBtn = $<HTMLButtonElement>("lyric-flag-done");

let host: FlagHost | null = null;

/// The track the flags belong to, and what it shows.
let meta: Meta | null = null;
let key = "";
let duration: number | null = null;
let lyrics: Lyrics | null = null;
let rows: Row[] = [];
let flags: Flag[] = [];
/// Whether `flags` holds what's on disk yet. Nothing can be flagged until it does: every save
/// writes the whole list, so saving before the load would write over what was there.
let loaded = false;
/// Bumped whenever the track changes, so a load that answers late is dropped.
let generation = 0;
/// Saves go one after another, so a slow one can't land after a newer one.
let saving: Promise<unknown> = Promise.resolve();

let open = false;
let selected: string | null = null;
/// The row a shift-click stretches from.
let anchor = -1;
/// When the listener last scrolled or clicked the pane in flag mode.
let touchedAt = 0;
/// How long the pane holds still after that, rather than scrolling to the playing line.
const HOLD_MS = 4000;

export function init(h: FlagHost) {
  host = h;
}

export const isOpen = () => open;

/// Whether the highlighter should leave the pane's scroll alone for now.
export const holdScroll = () => open && Date.now() - touchedAt < HOLD_MS;

/// The flags on the lyrics showing, as opposed to ones made on an earlier version of them.
const current = () => (lyrics ? flags.filter((f) => appliesTo(f.shown, lyrics!)) : []);

/// What the flag button's tooltip says.
export function tip(): string {
  if (open) return "Stop flagging";
  const n = current().length;
  if (n === 0) return "Something wrong with these lyrics? Flag it (F flags the line playing now)";
  const whole = current().some((f) => !f.span);
  return `${whole ? "Flagged: the whole song" : `${n} flagged`}. Click to see or change`;
}

/// The track's lyrics were painted. Called on every repaint of the track's own lyrics,
/// including the live updates of auto-sync, so the rows are always what's on screen.
export function show(m: Meta, len: number | null, l: Lyrics, r: Row[]) {
  const k = `${m.title}|${m.artist}`;
  lyrics = l;
  rows = r;
  duration = len;
  if (k !== key) {
    close();
    meta = m;
    key = k;
    flags = [];
    loaded = false;
    selected = null;
    load();
  }
  refresh();
}

function load() {
  if (!meta) return;
  const gen = ++generation;
  void invoke<TrackFlags | null>("lyric_flags", {
    artist: meta.artist,
    track: meta.title,
    album: meta.album || null,
  })
    .then((t) => {
      if (gen !== generation) return;
      flags = t?.flags ?? [];
      loaded = true;
      if (selected && !flags.some((f) => f.id === selected)) selected = null;
      refresh();
    })
    .catch((e) => {
      // Leave flagging off for this track rather than risk saving over a file we can't read.
      console.warn("couldn't read lyric flags", e);
      if (gen === generation) host?.flash("Couldn't read this song's flags");
    });
}

/// Settings removed flags. Whichever track they were for, reading this one's again is cheap.
window.addEventListener(FLAGS_CHANGED, load);

/// Playback moved to another track, whose lyrics haven't arrived. Anything done now would be
/// timed against the new song and filed under the old one.
export function trackChanged() {
  close();
  generation++;
  key = "";
  meta = lyrics = null;
  rows = [];
  flags = [];
  loaded = false;
  btn.hidden = true;
}

/// The button, the row marks and (when open) the bar, from the state.
export function refresh() {
  const blocked = !!host?.blocked();
  const usable = loaded && !!lyrics && rows.some((r) => r.text.trim()) && !blocked;
  if (!usable && open) close();
  btn.hidden = !usable;
  btn.classList.toggle("on", open);
  btn.classList.toggle("flagged", !open && current().length > 0);
  // Whatever owns the pane drew its rows from other lyrics, or as an editor: not ours to mark.
  if (!blocked) decorate();
  if (open) renderBar();
}

function decorate() {
  const cur = current();
  lyricsEl.classList.toggle("flagging", open);
  lyricsEl.querySelectorAll<HTMLElement>(".line[data-idx]").forEach((div) => {
    const i = Number(div.dataset.idx);
    const covers = (f: Flag) => !!f.span && f.span.first <= i && i <= f.span.last;
    div.classList.toggle("flagged", cur.some(covers));
    div.classList.toggle("flag-sel", open && cur.some((f) => f.id === selected && covers(f)));
  });
}

// ---- making flags -------------------------------------------------------------

function newFlag(span: Flag["span"]): Flag {
  return {
    id: crypto.randomUUID(),
    created: Date.now(),
    span,
    tags: [],
    note: "",
    shown: shownOf(lyrics!, host!.offset()),
  };
}

/// Put `f` in (merging it with any stretch it touches) and save. Returns what it became.
function put(f: Flag): Flag {
  const out = addMerged(flags, f, rows, duration);
  flags = out.flags;
  persist();
  return out.flag;
}

function drop(id: string) {
  flags = flags.filter((f) => f.id !== id);
  if (selected === id) selected = null;
  persist();
}

function persist() {
  if (!meta) return;
  const m = meta;
  const k = key;
  const list = flags;
  saving = saving
    .then(() =>
      invoke<TrackFlags>("save_lyric_flags", {
        artist: m.artist,
        track: m.title,
        album: m.album || null,
        duration,
        flags: list,
      })
    )
    .then((stored) => {
      // Take the backend's copy (it adds what auto-sync heard), unless more changes have
      // been made since this save was sent.
      if (k === key && flags === list) flags = stored.flags;
    })
    .catch((e) => {
      console.warn("couldn't save lyric flags", e);
      if (k === key) host?.flash(`Couldn't save the flag: ${e}`);
    });
  refresh();
}

/// `F`: flag the line playing now.
export function quick() {
  if (!host || !lyrics || !loaded || host.blocked() || key !== host.currentKey()) return;
  if (!rows.some((r) => r.t !== null)) {
    // No timings, so no "now" to go by. Let them point at it instead.
    if (!open) toggle();
    return;
  }
  const at = quickRows(rows, host.now());
  if (!at) return;
  const f = put(newFlag(spanOf(rows, at[0], at[1], duration)));
  if (open) {
    selected = f.id;
    anchor = at[1];
    refresh();
  }
  host.flash(`Flagged · ${describe(f)}`);
}

// ---- flag mode ----------------------------------------------------------------

export function toggle() {
  if (open) close();
  else if (loaded && lyrics && !host?.blocked()) {
    open = true;
    selected = null;
    anchor = -1;
    bar.hidden = false;
    refresh();
  }
}

export function close() {
  if (!open) return;
  open = false;
  selected = null;
  bar.hidden = true;
  refresh();
}

const selectedFlag = () => flags.find((f) => f.id === selected) ?? null;

function renderBar() {
  const f = selectedFlag();
  const cur = current();
  const older = flags.length - cur.length;
  const olderNote = older
    ? ` ${older === 1 ? "One more flag is" : `${older} more are`} on an earlier version of these lyrics (see Settings).`
    : "";
  if (f) {
    infoEl.textContent = `${describe(f)}. What's wrong with it?`;
  } else if (cur.length === 0) {
    infoEl.textContent =
      "Click a line to flag it, then shift-click another to flag everything between. " +
      `F flags the line playing now.${olderNote}`;
  } else {
    infoEl.textContent =
      `${cur.length} flagged. Click a flagged line to change it, or another line to flag it.` +
      olderNote;
  }

  tagsEl.hidden = noteEl.hidden = removeBtn.hidden = !f;
  if (f) {
    tagsEl.replaceChildren(
      ...TAGS.filter((t) => !t.whole || !f.span).map((t) => {
        const b = document.createElement("button");
        b.className = "flag-tag";
        b.textContent = t.label;
        const on = f.tags.includes(t.id);
        b.classList.toggle("on", on);
        b.setAttribute("aria-pressed", String(on));
        b.addEventListener("click", () => {
          const tags = on ? f.tags.filter((x) => x !== t.id) : [...f.tags, t.id];
          put({ ...f, tags });
        });
        return b;
      })
    );
    // Don't overwrite what's being typed when something else repaints the bar.
    if (document.activeElement !== noteEl) noteEl.value = f.note;
  }
  const whole = cur.find((x) => !x.span);
  wholeBtn.classList.toggle("on", !!whole && whole.id === selected);
  wholeBtn.textContent = whole ? "Whole song (flagged)" : "Flag whole song";
}

function rowOf(e: Event): number {
  const div = (e.target as HTMLElement).closest<HTMLElement>(".line[data-idx]");
  return div && lyricsEl.contains(div) ? Number(div.dataset.idx) : -1;
}

lyricsEl.addEventListener("click", (e) => {
  if (!open || !lyrics) return;
  const i = rowOf(e);
  if (i < 0) return;
  touchedAt = Date.now();
  const sel = selectedFlag();
  if (e.shiftKey && sel?.span && anchor >= 0) {
    selected = put({ ...sel, span: spanOf(rows, anchor, i, duration) }).id;
  } else {
    const hit = current().find((f) => f.span && f.span.first <= i && i <= f.span.last);
    if (hit) selected = hit.id === selected ? null : hit.id;
    else selected = put(newFlag(spanOf(rows, i, i, duration))).id;
    anchor = i;
  }
  refresh();
});
// Shift-click would otherwise select text across the rows it stretches over.
lyricsEl.addEventListener("mousedown", (e) => {
  if (open && e.shiftKey) e.preventDefault();
});
lyricsEl.addEventListener("wheel", () => {
  if (open) touchedAt = Date.now();
});
lyricsEl.addEventListener("pointerdown", () => {
  if (open) touchedAt = Date.now();
});

wholeBtn.addEventListener("click", () => {
  const whole = current().find((f) => !f.span);
  if (whole) selected = whole.id === selected ? null : whole.id;
  else selected = put(newFlag(null)).id;
  anchor = -1;
  refresh();
});

removeBtn.addEventListener("click", () => {
  if (selected) drop(selected);
});

noteEl.addEventListener("change", () => {
  const f = selectedFlag();
  if (f && f.note !== noteEl.value.trim()) put({ ...f, note: noteEl.value.trim() });
});
noteEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter") noteEl.blur();
});

doneBtn.addEventListener("click", close);
btn.addEventListener("click", toggle);

window.addEventListener("keydown", (e) => {
  if (e.ctrlKey || e.altKey || e.metaKey) return;
  const el = e.target as HTMLElement;
  if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable) {
    if (open && e.key === "Escape" && el === noteEl) noteEl.blur();
    return;
  }
  if (host?.covered()) return;
  if (e.key === "f" || e.key === "F") quick();
  else if (open && e.key === "Escape") close();
});
