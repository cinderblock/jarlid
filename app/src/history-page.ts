// History: every song played, newest first, a day at a time, and any of them played again
// whose audio is saved on this computer. The strip under the art shows only the most recent;
// this is how to go further back. See plans/play-from-history.md.
//
// The same scrim and sheet as Find a song, and the same rows.

import { invoke } from "@tauri-apps/api/core";
import { hueOf, initials } from "./stations-page";
import {
  byDay,
  matchesQuery,
  playability,
  type HistItem,
  type IndexedSong,
  type Playability,
  type Station,
} from "./play-history";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const page = $("history-page");
const input = $<HTMLInputElement>("hp-search");
const closeBtn = $<HTMLButtonElement>("hp-close");
const subEl = $("hp-sub");
const listEl = $("hp-list");

export interface Index {
  enabled: boolean;
  songs: IndexedSong[];
}

interface Context {
  history: () => HistItem[];
  /** Tuner token of the station playing now. */
  active: () => string;
  stations: () => Station[];
  /** Whether the screen shows the network speakers, where saved songs do not play. */
  remote: () => boolean;
  /** The entry playing now, if it is the newest. */
  playing: () => HistItem | undefined;
  openSettings: () => void;
  onError: (message: string) => void;
}

let ctx: Context | undefined;
let index: Index = { enabled: true, songs: [] };
let loaded = false;
/** Rows on screen, in order, with what clicking each would do. */
let shown: { p: Playability; el: HTMLElement; canPlay: boolean }[] = [];
let cursor = 0;

export function init(context: Context) {
  ctx = context;
}

/** What is saved on disk, from Rust. Shared with the strip's card so both say the same. */
export async function fetchIndex(): Promise<Index> {
  return invoke<Index>("saved_index");
}

/** Play `h` as `p` says. Resolves once the engine has taken it. */
export async function playEntry(p: Playability) {
  if (p.kind !== "play") return;
  const args: Record<string, string> = { id: p.id };
  if (p.switches) {
    args.stationName = p.station.name;
    args.stationToken = p.station.token;
  }
  await invoke("native_play_saved", args);
}

/** One line on what playing `h` again would involve, or why it can't. Empty when nothing to say. */
export function describe(p: Playability): string {
  switch (p.kind) {
    case "play":
      return p.switches ? `Plays on ${p.station.name || "its station"}` : "";
    case "unsaved":
      return "Not saved on this computer";
    case "gone":
      return "Its station was deleted";
    case "off":
      return "";
  }
}

export function isOpen() {
  return !page.hidden;
}

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
    index = await fetchIndex();
  } catch (e) {
    // Not signed in, or the engine is down: the history is still worth reading.
    index = { enabled: true, songs: [] };
    ctx?.onError(String(e));
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

/** Redraw if open: a new song arrived, or the station changed. */
export function refresh() {
  if (!page.hidden) render();
}

function play(p: Playability) {
  if (p.kind !== "play" || ctx?.remote()) return;
  close();
  playEntry(p).catch((e) => ctx?.onError(String(e)));
}

const time = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });

function render() {
  if (!ctx) return;
  listEl.innerHTML = "";
  shown = [];
  const history = ctx.history();
  const remote = ctx.remote();
  const playing = ctx.playing();

  subEl.innerHTML = "";
  const note = (text: string, action?: { label: string; run: () => void }) => {
    const p = document.createElement("span");
    p.textContent = text;
    subEl.appendChild(p);
    if (action) {
      const b = document.createElement("button");
      b.className = "hp-link";
      b.type = "button";
      b.textContent = action.label;
      b.addEventListener("click", action.run);
      subEl.appendChild(b);
    }
  };

  if (!history.length) {
    const empty = document.createElement("div");
    empty.className = "fs-empty";
    empty.textContent = "Nothing played yet. Songs collect here as you listen.";
    listEl.appendChild(empty);
    return;
  }

  const matching = history.filter((h) => matchesQuery(h, input.value));
  const verdicts = new Map<HistItem, Playability>();
  const verdict = (h: HistItem) => {
    let v = verdicts.get(h);
    if (!v) {
      v = loaded ? playability(h, index, ctx!.active(), ctx!.stations()) : { kind: "unsaved" };
      verdicts.set(h, v);
    }
    return v;
  };

  const total = `${history.length} song${history.length === 1 ? "" : "s"}`;
  if (remote) {
    note(`${total}. Playing on the speakers: songs play again only on this computer.`);
  } else if (loaded && !index.enabled) {
    note(`${total}. Saved songs is off, so none can be played again.`, {
      label: "Open Settings",
      run: () => {
        close();
        ctx?.openSettings();
      },
    });
  } else if (loaded) {
    const can = history.filter((h) => verdict(h).kind === "play").length;
    note(`${total} · ${can} saved on this computer, which can be played again`);
  } else {
    note(total);
  }

  if (!matching.length) {
    const empty = document.createElement("div");
    empty.className = "fs-empty";
    empty.textContent = `Nothing in the history matches “${input.value.trim()}”.`;
    listEl.appendChild(empty);
    return;
  }

  for (const day of byDay(matching)) {
    const group = document.createElement("section");
    group.className = "hp-day";
    if (day.label) {
      const head = document.createElement("h3");
      head.textContent = day.label;
      group.appendChild(head);
    }
    for (const h of day.items) {
      const p = verdict(h);
      const canPlay = loaded && p.kind === "play" && !remote && h !== playing;
      const el = row(h, p, h === playing, canPlay, shown.length);
      shown.push({ p, el, canPlay });
      group.appendChild(el);
    }
    listEl.appendChild(group);
  }
  cursor = Math.min(cursor, shown.length - 1);
  shown[cursor]?.el.classList.add("cursor");
}

function row(h: HistItem, p: Playability, isPlaying: boolean, canPlay: boolean, i: number) {
  const remote = !!ctx?.remote();
  const el = document.createElement("button");
  el.type = "button";
  // Dimmed only once the answer is in, so the list does not flash grey while it loads.
  el.className =
    "fs-row hp-row" + (isPlaying ? " playing" : "") + (canPlay || !loaded || isPlaying ? "" : " unplayable");
  el.setAttribute("aria-disabled", String(!canPlay));

  const art = document.createElement("span");
  art.className = "sp-art";
  art.style.setProperty("--tile-hue", String(hueOf(h.artist || h.title)));
  art.textContent = initials(h.artist || h.title);
  if (h.art) {
    const img = document.createElement("img");
    img.alt = "";
    img.loading = "lazy";
    img.decoding = "async";
    img.addEventListener("load", () => img.classList.add("ready"));
    img.src = h.art;
    art.appendChild(img);
  }
  if (canPlay) {
    const glyph = document.createElement("span");
    glyph.className = "hp-play";
    glyph.innerHTML = '<svg viewBox="0 0 24 24"><path d="M8 5l11 7-11 7z"/></svg>';
    art.appendChild(glyph);
  }

  const text = document.createElement("span");
  text.className = "fs-text";
  const title = document.createElement("span");
  title.className = "fs-title";
  title.textContent = h.title;
  const meta = document.createElement("span");
  meta.className = "fs-meta";
  meta.textContent = [h.artist, h.album].filter(Boolean).join(" · ");
  text.append(title, meta);

  const side = document.createElement("span");
  side.className = "fs-side";
  const when = document.createElement("span");
  when.className = "fs-len";
  when.textContent = isPlaying ? "Playing now" : h.at ? time(h.at) : "";
  const where = document.createElement("span");
  where.className = "fs-when";
  // What a click would do, when it is more than "play this here"; otherwise the station it
  // played on, which is what a listener scanning back through a day wants to know.
  where.textContent = (!remote && !isPlaying && loaded && index.enabled && describe(p)) || h.stationName || "";
  side.append(when, where);

  el.append(art, text, side);
  el.addEventListener("mousemove", () => moveCursor(i, false));
  el.addEventListener("click", () => {
    if (canPlay) play(p);
  });
  return el;
}

function moveCursor(next: number, scroll: boolean) {
  if (!shown.length) return;
  next = Math.max(0, Math.min(shown.length - 1, next));
  if (next === cursor) return;
  shown[cursor]?.el.classList.remove("cursor");
  cursor = next;
  shown[cursor].el.classList.add("cursor");
  if (scroll) shown[cursor].el.scrollIntoView({ block: "nearest" });
}

input.addEventListener("input", () => {
  cursor = 0;
  render();
  listEl.scrollTop = 0;
});
input.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    moveCursor(cursor + 1, true);
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    moveCursor(cursor - 1, true);
  } else if (e.key === "PageDown" || e.key === "PageUp") {
    e.preventDefault();
    const rowH = shown[0]?.el.offsetHeight || 50;
    const step = Math.max(1, Math.floor(listEl.clientHeight / rowH) - 1);
    moveCursor(cursor + (e.key === "PageDown" ? step : -step), true);
  } else if (e.key === "Enter") {
    e.preventDefault();
    const r = shown[cursor];
    if (r?.canPlay) play(r.p);
  }
});
closeBtn.addEventListener("click", close);
page.addEventListener("click", (e) => {
  if (!(e.target as HTMLElement).closest(".page-panel")) close();
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !page.hidden) close();
});
