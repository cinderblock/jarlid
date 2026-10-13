// Choosing among the lyrics LRCLIB has for a track.
//
// A search finds several records for most tracks: other versions, other languages,
// duplicate uploads. The backend ranks them and shows the best, and sometimes the best is
// wrong. This lists every record found and lets the listener try them.
//
// Trying one means *previewing it in the pane*, live against the song, not reading it in a
// list. Watching the lines light up (or fail to) is how you tell the radio edit from the
// album cut and the English version from the French one. Each preview is temporary. Only
// "Use this" pins a record, and the backend then serves that record for the track from
// then on.
//
// Two states, both in the pane: the list (which hides the lyrics) and a preview (which
// shows them, with a bar to step between versions or go back to the list).

import { invoke } from "@tauri-apps/api/core";
import type { Lyrics } from "./lyric-editor";

export interface PickerContext {
  /// Pandora's naming for the track. Also the key a choice is filed under.
  meta: { title: string; artist: string; album: string };
  /// The playing track's length, to compare each version against.
  duration: number | null;
  /// What the track shows now.
  current: Lyrics;
  /// Paint a candidate into the pane without adopting it.
  preview: (lyrics: Lyrics) => void;
  /// A candidate was chosen; it is now the track's lyrics.
  onChosen: (lyrics: Lyrics) => void;
  /// The choice was forgotten; look the track up again.
  onReset: () => void;
  /// Closing after a preview: repaint the pane with the track's own lyrics.
  onExit: () => void;
  /// The picker closed, however it closed.
  onClosed: () => void;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const lyricsEl = $("lyrics");
const panel = $("lyric-picker");
const statusEl = $("lyric-picker-status");
const listEl = $("lyric-picker-list");
const autoBtn = $<HTMLButtonElement>("lyric-picker-auto");
const closeBtn = $<HTMLButtonElement>("lyric-picker-close");
const bar = $("lyric-pick-bar");
const infoEl = $("lyric-pick-info");
const prevBtn = $<HTMLButtonElement>("lyric-pick-prev");
const nextBtn = $<HTMLButtonElement>("lyric-pick-next");
const listBtn = $<HTMLButtonElement>("lyric-pick-list");
const useBtn = $<HTMLButtonElement>("lyric-pick-use");

let ctx: PickerContext | null = null;
let candidates: Lyrics[] = [];
/// Index of the candidate in the pane, or -1 while the list is showing.
let previewing = -1;
/// Whether the pane has been painted with something other than the track's own lyrics.
let painted = false;
let busy = false;
/// Bumped on every open, so a search that answers after the picker was closed (or
/// reopened for another track) is ignored.
let generation = 0;

export const isOpen = () => ctx !== null;

export async function open(c: PickerContext) {
  ctx = c;
  candidates = [];
  previewing = -1;
  painted = false;
  busy = false;
  const gen = ++generation;

  showList();
  listEl.replaceChildren();
  autoBtn.hidden = !c.current.pinned;
  setStatus("Searching LRCLIB…");

  let found: Lyrics[];
  try {
    found = await invoke<Lyrics[]>("lyrics_candidates", {
      artist: c.meta.artist,
      track: c.meta.title,
      album: c.meta.album || null,
      duration: c.duration,
    });
  } catch (e) {
    if (gen !== generation) return;
    setStatus(`Couldn't search LRCLIB: ${e}`, true);
    return;
  }
  if (gen !== generation) return;

  // What's showing came from an exact lookup that search may not reproduce. It belongs
  // in the list regardless; there's no rank for it, so it goes first.
  const cur = c.current;
  if (cur.id != null && !cur.overridden && !found.some((f) => f.id === cur.id)) {
    found.unshift(cur);
  }
  candidates = found;
  renderList();
}

/// Leave the picker, putting the track's own lyrics back if a preview replaced them.
export function close() {
  if (!ctx) return;
  const c = ctx;
  ctx = null;
  generation++;
  panel.hidden = true;
  bar.hidden = true;
  lyricsEl.hidden = false;
  if (painted) c.onExit();
  c.onClosed();
}

// ---- the list -----------------------------------------------------------------

function showList() {
  previewing = -1;
  bar.hidden = true;
  lyricsEl.hidden = true;
  panel.hidden = false;
}

function setStatus(text: string, err = false) {
  statusEl.textContent = text;
  statusEl.classList.toggle("err", err);
}

function renderList() {
  if (!ctx) return;
  const cur = ctx.current;
  if (candidates.length === 0) {
    setStatus("LRCLIB has nothing else for this track.");
  } else if (cur.overridden) {
    // A local edit wins over any record, so choosing one would change nothing visible.
    setStatus(
      `${countLabel()} You're showing your own edit, which wins over all of them. ` +
        "Discard it in edit mode to use one."
    );
  } else {
    setStatus(`${countLabel()} Pick one to try it against the song.`);
  }

  listEl.replaceChildren(
    ...candidates.map((l, i) => {
      const row = document.createElement("button");
      row.className = "pick-row";
      const showing = !cur.overridden && l.id === cur.id;
      row.classList.toggle("current", showing);

      const title = document.createElement("div");
      title.className = "pick-title";
      title.textContent = l.trackName || "Untitled";
      if (l.albumName) {
        const album = document.createElement("span");
        album.className = "pick-album";
        album.textContent = ` · ${l.albumName}`;
        title.append(album);
      }

      const meta = document.createElement("div");
      meta.className = "pick-meta";
      const bits = [lengthLabel(l), l.synced ? "Synced" : "Words only"];
      if (showing) bits.push(cur.pinned ? "Your choice" : "Showing now");
      else if (i === 0 && !cur.pinned) bits.push("Best match");
      meta.textContent = bits.join(" · ");

      const snippet = document.createElement("div");
      snippet.className = "pick-snippet";
      snippet.textContent = opening(l);

      row.append(title, meta, snippet);
      row.addEventListener("click", () => previewAt(i));
      return row;
    })
  );
}

const countLabel = () =>
  candidates.length === 1 ? "1 version found." : `${candidates.length} versions found.`;

function fmt(s: number) {
  const m = Math.floor(s / 60);
  return `${m}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
}

/// The record's length, and how far it is from what's playing. Further than a couple of
/// seconds is a different cut, whose timings will drift.
function lengthLabel(l: Lyrics) {
  if (l.duration == null) return "Length unknown";
  const len = fmt(l.duration);
  const want = ctx?.duration;
  if (!want) return len;
  const gap = Math.round(l.duration - want);
  if (Math.abs(gap) <= 2) return `${len}, same length`;
  return `${len}, ${Math.abs(gap)}s ${gap > 0 ? "longer" : "shorter"}`;
}

/// The first couple of distinct lines. Usually enough to tell the language, or a version
/// that doubles every line.
function opening(l: Lyrics) {
  const text = l.synced ?? l.plain ?? "";
  const lines: string[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const t = raw.replace(/^(\s*\[[^\]]*\])+/, "").trim();
    if (t && !lines.includes(t)) lines.push(t);
    if (lines.length === 2) break;
  }
  return lines.join(" / ");
}

// ---- previewing ---------------------------------------------------------------

function previewAt(i: number) {
  if (!ctx || i < 0 || i >= candidates.length) return;
  previewing = i;
  painted = true;
  panel.hidden = true;
  lyricsEl.hidden = false;
  bar.hidden = false;
  ctx.preview(candidates[i]);
  refreshBar();
}

function refreshBar() {
  if (!ctx || previewing < 0) return;
  const l = candidates[previewing];
  const showing = !ctx.current.overridden && l.id === ctx.current.id;
  infoEl.textContent =
    `${previewing + 1} of ${candidates.length} · ${l.trackName || "Untitled"}` +
    `${l.albumName ? ` · ${l.albumName}` : ""} · ${lengthLabel(l)}`;
  infoEl.classList.remove("err");
  prevBtn.disabled = busy || previewing === 0;
  nextBtn.disabled = busy || previewing === candidates.length - 1;
  listBtn.disabled = busy;
  // Choosing what's already chosen would only renew the pin; nothing to do.
  useBtn.disabled = busy || ctx.current.overridden || (showing && !!ctx.current.pinned);
  useBtn.textContent = showing && !ctx.current.pinned ? "Keep this" : "Use this";
}

const keyArgs = () => ({
  artist: ctx!.meta.artist,
  track: ctx!.meta.title,
  album: ctx!.meta.album || null,
});

async function use() {
  if (!ctx || previewing < 0 || busy) return;
  busy = true;
  refreshBar();
  try {
    const chosen = await invoke<Lyrics>("choose_lyrics", {
      ...keyArgs(),
      lyrics: candidates[previewing],
    });
    if (!ctx) return;
    const c = ctx;
    painted = false; // what's painted is now the track's own lyrics
    close();
    c.onChosen(chosen);
  } catch (e) {
    busy = false;
    refreshBar();
    infoEl.textContent = `Couldn't save that choice: ${e}`;
    infoEl.classList.add("err");
  }
}

async function backToAutomatic() {
  if (!ctx || busy) return;
  busy = true;
  autoBtn.disabled = true;
  try {
    await invoke("clear_lyrics_choice", keyArgs());
    const c = ctx;
    painted = false; // a fresh lookup is about to repaint anyway
    close();
    c.onReset();
  } catch (e) {
    setStatus(`Couldn't forget the choice: ${e}`, true);
  } finally {
    busy = false;
    autoBtn.disabled = false;
  }
}

prevBtn.addEventListener("click", () => previewAt(previewing - 1));
nextBtn.addEventListener("click", () => previewAt(previewing + 1));
listBtn.addEventListener("click", () => {
  showList();
  renderList();
});
useBtn.addEventListener("click", () => void use());
autoBtn.addEventListener("click", () => void backToAutomatic());
closeBtn.addEventListener("click", close);

// Escape steps back one level: preview to list, list to closed. Arrows step through
// versions while previewing.
window.addEventListener("keydown", (e) => {
  if (!ctx) return;
  const el = e.target as HTMLElement;
  if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable) return;
  if (e.key === "Escape") {
    if (previewing >= 0 && !busy) {
      showList();
      renderList();
    } else close();
  } else if (previewing >= 0 && !busy && e.key === "ArrowLeft") previewAt(previewing - 1);
  else if (previewing >= 0 && !busy && e.key === "ArrowRight") previewAt(previewing + 1);
});
