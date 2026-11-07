// Find a song: search the songs saved on this computer for the station playing now, and jump
// straight to one. The station carries on with its own queue afterwards.
//
// Only the station playing now, because thumbs go to that station: a song saved on another
// one would be rated in the wrong place (the engine refuses it for the same reason). Only
// while Saved songs is on, because Settings promises that turning it off stops songs being
// played from disk. See plans/find-saved-song.md.

import { invoke } from "@tauri-apps/api/core";
import { ago, hueOf, initials } from "./stations-page";
import { clock, search as searchSongs, type SavedSong } from "./song-search";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

interface Reply {
  enabled: boolean;
  songs: SavedSong[];
}

const page = $("find-page");
const input = $<HTMLInputElement>("fs-search");
const closeBtn = $<HTMLButtonElement>("fs-close");
const subEl = $("fs-sub");
const listEl = $("fs-list");

let songs: SavedSong[] = [];
let enabled = true;
let loaded = false;
let shown: SavedSong[] = [];
/** Index into `shown` of the row Enter plays. Follows the arrow keys and the pointer. */
let cursor = 0;
let station = "";
/** The song on screen, so its row can say so. Matched by name: `nowplaying` carries no id. */
let playing = { title: "", artist: "" };
/** Opens Settings, for the "Saved songs is off" state. Set by main.ts to avoid a cycle. */
let openSettings: () => void = () => {};
let onError: (message: string) => void = () => {};

export function init(opts: { openSettings: () => void; onError: (message: string) => void }) {
  openSettings = opts.openSettings;
  onError = opts.onError;
}

export function setStation(name: string) {
  station = name;
}

export function setNowPlaying(title: string, artist: string) {
  playing = { title, artist };
  if (!page.hidden) render();
}

export function isOpen() {
  return !page.hidden;
}

/// How long `.closing` stays on before the page is hidden; matches the CSS, like the
/// Stations page's.
const CLOSE_MS = 200;
let closeTimer: ReturnType<typeof setTimeout> | undefined;

export async function open() {
  clearTimeout(closeTimer);
  page.classList.remove("closing");
  page.hidden = false;
  input.value = "";
  cursor = 0;
  loaded = false;
  render();
  page.classList.add("opening");
  page.addEventListener("animationend", () => page.classList.remove("opening"), { once: true });
  input.focus();

  try {
    const reply = await invoke<Reply>("saved_songs");
    enabled = reply.enabled;
    songs = reply.songs;
  } catch (e) {
    enabled = true;
    songs = [];
    onError(String(e));
  }
  loaded = true;
  if (!page.hidden) render();
}

export function close() {
  if (page.hidden) return;
  page.classList.remove("opening");
  page.classList.add("closing");
  clearTimeout(closeTimer);
  closeTimer = setTimeout(() => {
    page.classList.remove("closing");
    page.hidden = true;
  }, CLOSE_MS);
}

function play(song: SavedSong) {
  close();
  invoke("native_play_saved", { id: song.id }).catch((e) => onError(String(e)));
}

const isPlaying = (s: SavedSong) => s.title === playing.title && s.artist === playing.artist;

function render() {
  listEl.innerHTML = "";
  subEl.textContent = "";
  shown = [];
  input.disabled = loaded && !enabled;

  if (!loaded) return;

  if (!enabled) {
    const empty = document.createElement("div");
    empty.className = "fs-empty";
    const p = document.createElement("p");
    p.textContent =
      "Saved songs is off. Jarlid keeps songs on this computer only when it is on, so there is nothing to search.";
    const btn = document.createElement("button");
    btn.className = "btn";
    btn.textContent = "Open Settings";
    btn.addEventListener("click", () => {
      close();
      openSettings();
    });
    empty.append(p, btn);
    listEl.appendChild(empty);
    return;
  }

  const where = station || "this station";
  subEl.textContent = songs.length
    ? `${songs.length} song${songs.length === 1 ? "" : "s"} saved on ${where}`
    : "";

  if (!songs.length) {
    const empty = document.createElement("div");
    empty.className = "fs-empty";
    const p = document.createElement("p");
    p.textContent = `No songs saved on ${where} yet. Each song is saved as it comes up next, so they collect as you listen. Songs saved by an older version of Jarlid show up once they play again.`;
    empty.appendChild(p);
    listEl.appendChild(empty);
    return;
  }

  shown = searchSongs(songs, input.value);
  cursor = Math.min(cursor, Math.max(0, shown.length - 1));

  if (!shown.length) {
    const empty = document.createElement("div");
    empty.className = "fs-empty";
    empty.textContent = `Nothing saved on ${where} matches “${input.value.trim()}”.`;
    listEl.appendChild(empty);
    return;
  }

  shown.forEach((s, i) => listEl.appendChild(row(s, i)));
}

function row(s: SavedSong, i: number) {
  const el = document.createElement("button");
  el.type = "button";
  el.className = "fs-row" + (i === cursor ? " cursor" : "") + (isPlaying(s) ? " playing" : "");

  const art = document.createElement("span");
  art.className = "sp-art";
  art.style.setProperty("--tile-hue", String(hueOf(s.artist || s.title)));
  art.textContent = initials(s.artist || s.title);
  if (s.art) {
    const img = document.createElement("img");
    img.alt = "";
    img.loading = "lazy";
    img.decoding = "async";
    img.addEventListener("load", () => img.classList.add("ready"));
    img.src = s.art;
    art.appendChild(img);
  }

  const text = document.createElement("span");
  text.className = "fs-text";
  const title = document.createElement("span");
  title.className = "fs-title";
  title.textContent = s.title;
  const meta = document.createElement("span");
  meta.className = "fs-meta";
  meta.textContent = [s.artist, s.album].filter(Boolean).join(" · ");
  text.append(title, meta);

  const side = document.createElement("span");
  side.className = "fs-side";
  const len = document.createElement("span");
  len.className = "fs-len";
  len.textContent = s.length ? clock(s.length) : "";
  const when = document.createElement("span");
  when.className = "fs-when";
  when.textContent = isPlaying(s) ? "Playing now" : s.playedAt ? ago(s.playedAt) : "";
  side.append(len, when);

  el.append(art, text);
  if (s.thumbUp) {
    const up = document.createElement("span");
    up.className = "fs-up";
    up.setAttribute("aria-label", "Thumbs up");
    up.innerHTML =
      '<svg viewBox="0 0 24 24"><path d="M14 9V5a3 3 0 0 0-3-3l-4 9v11h11.3a2 2 0 0 0 2-1.7l1.4-9A2 2 0 0 0 19.7 9H14zM7 22H4.3A2.3 2.3 0 0 1 2 19.7v-6.4A2.3 2.3 0 0 1 4.3 11H7"/></svg>';
    el.appendChild(up);
  }
  el.appendChild(side);

  el.addEventListener("mousemove", () => moveCursor(i, false));
  el.addEventListener("click", () => play(s));
  return el;
}

function moveCursor(next: number, scroll: boolean) {
  if (!shown.length) return;
  next = Math.max(0, Math.min(shown.length - 1, next));
  if (next === cursor) return;
  const rows = listEl.querySelectorAll<HTMLElement>(".fs-row");
  rows[cursor]?.classList.remove("cursor");
  cursor = next;
  rows[cursor]?.classList.add("cursor");
  if (scroll) rows[cursor]?.scrollIntoView({ block: "nearest" });
}

input.addEventListener("input", () => {
  cursor = 0;
  render();
});
input.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    moveCursor(cursor + 1, true);
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    moveCursor(cursor - 1, true);
  } else if (e.key === "Enter") {
    e.preventDefault();
    const s = shown[cursor];
    if (s) play(s);
  }
});
closeBtn.addEventListener("click", close);
page.addEventListener("click", (e) => {
  if (!(e.target as HTMLElement).closest(".page-panel")) close();
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !page.hidden) close();
});
