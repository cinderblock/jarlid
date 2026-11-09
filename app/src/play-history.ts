// The play history: what it remembers, and which of its songs can be played again.
//
// Pure, so it can be tested without a DOM. The strip under the art and the History page both
// draw from this; see plans/play-from-history.md.
//
// Pandora plays nothing on demand, so a song can only be played again when its audio is saved
// on this computer (the opt-in "Saved songs" cache). A saved song also knows which stations it
// was saved on, and it is played on one of those, because thumbs go to the station playing.

import { fold } from "./song-search";

export interface HistItem {
  art: string;
  title: string;
  artist: string;
  album?: string;
  /** Epoch milliseconds: when it started playing. */
  at?: number;
  /** Pandora's id for the track. Absent on entries recorded before v1.19. */
  id?: string;
  /** The station it played on: its tuner token, and its name at the time. */
  station?: string;
  stationName?: string;
}

/** As the `saved_index` command describes a saved song. */
export interface IndexedSong {
  id: string;
  title: string;
  artist: string;
  /** Tuner tokens of the stations it was saved on. */
  stations: string[];
}

export interface Station {
  name: string;
  token: string;
}

/**
 * How far back the history goes. The strip shows the newest {@link STRIP_LENGTH}; the History
 * page shows all of it. At a few hundred bytes an entry this is well inside `localStorage`.
 */
export const HISTORY_LENGTH = 1000;
export const STRIP_LENGTH = 40;

/** The same song as the newest entry, so a resumed track is not recorded twice. */
export function isRepeat(history: HistItem[], next: HistItem): boolean {
  const top = history[0];
  if (!top) return false;
  if (top.id && next.id) return top.id === next.id;
  return top.title === next.title && top.artist === next.artist;
}

/** `next` added at the front, trimmed to `limit`. Newest-first, always. */
export function pushed(history: HistItem[], next: HistItem, limit = HISTORY_LENGTH): HistItem[] {
  if (isRepeat(history, next)) return history;
  return [next, ...history].slice(0, limit);
}

/** Read the stored list, tolerating anything a past build or a hand edit left there. */
export function parse(text: string | null): HistItem[] {
  try {
    const raw: unknown = JSON.parse(text || "[]");
    if (!Array.isArray(raw)) return [];
    return raw.filter(
      (h): h is HistItem =>
        !!h && typeof h === "object" && typeof h.title === "string" && typeof h.artist === "string"
    );
  } catch {
    return [];
  }
}

export type Playability =
  | { kind: "play"; id: string; station: Station; switches: boolean }
  | { kind: "off" }
  | { kind: "unsaved" }
  | { kind: "gone" };

/**
 * Whether `h` can be played again, and on which station.
 *
 * - `play`: its audio is saved. `switches` says the station changes to play it.
 * - `off`: Saved songs is off, so nothing plays from disk.
 * - `unsaved`: its audio is not on this computer (never saved, or evicted since).
 * - `gone`: saved, but only on stations no longer in the list.
 *
 * Matching is by id when the entry has one. Entries from before ids were recorded match on
 * title and artist, folded, which can only be wrong between two recordings with identical
 * names; either is the same song to a listener.
 *
 * The station is, in order: the one playing now (no switch, and the queue is kept), the one
 * the entry was played on, then any other it was saved on. `stations` empty means the list
 * has not arrived yet, so any station is assumed to still exist.
 */
export function playability(
  h: HistItem,
  index: { enabled: boolean; songs: IndexedSong[] },
  active: string,
  stations: Station[]
): Playability {
  if (!index.enabled) return { kind: "off" };
  const title = fold(h.title);
  const artist = fold(h.artist);
  const song = h.id
    ? index.songs.find((s) => s.id === h.id)
    : index.songs.find((s) => fold(s.title) === title && fold(s.artist) === artist);
  if (!song || !song.stations.length) return { kind: "unsaved" };

  const known = (token: string) => !stations.length || stations.some((s) => s.token === token);
  const nameOf = (token: string) =>
    stations.find((s) => s.token === token)?.name ??
    (token === h.station ? h.stationName : undefined) ??
    "";

  const order = [active, h.station, ...song.stations].filter(
    (t): t is string => !!t && song.stations.includes(t) && known(t)
  );
  const token = order[0];
  if (!token) return { kind: "gone" };
  return {
    kind: "play",
    id: song.id,
    station: { token, name: nameOf(token) },
    switches: token !== active,
  };
}

/** Matches every word of `query` against the title, artist, album and station. */
export function matchesQuery(h: HistItem, query: string): boolean {
  const words = fold(query).split(" ").filter(Boolean);
  if (!words.length) return true;
  const hay = fold(`${h.title} ${h.artist} ${h.album ?? ""} ${h.stationName ?? ""}`);
  const squashed = hay.replace(/ /g, "");
  return words.every((w) => hay.includes(w) || squashed.includes(w));
}

/** Local midnight at the start of the day `ms` falls in. */
function dayStart(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/**
 * The heading for the day `at` falls in, relative to `now`: "Today", "Yesterday", the weekday
 * within the last week, then the date (with the year once it is not this year's).
 */
export function dayLabel(at: number, now = Date.now()): string {
  const days = Math.round((dayStart(now) - dayStart(at)) / 86_400_000);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  const d = new Date(at);
  if (days < 7) return d.toLocaleDateString(undefined, { weekday: "long" });
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  return d.toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
}

/**
 * The history split into days, newest first, each day's songs newest first. Entries without a
 * time (very old builds did not record one) go in a last group with an empty label.
 */
export function byDay(
  history: HistItem[],
  now = Date.now()
): { label: string; items: HistItem[] }[] {
  const groups: { label: string; key: number; items: HistItem[] }[] = [];
  for (const h of history) {
    const key = h.at ? dayStart(h.at) : -1;
    let group = groups.find((g) => g.key === key);
    if (!group) {
      group = { label: h.at ? dayLabel(h.at, now) : "", key, items: [] };
      groups.push(group);
    }
    group.items.push(h);
  }
  groups.sort((a, b) => b.key - a.key);
  return groups.map(({ label, items }) => ({ label, items }));
}
