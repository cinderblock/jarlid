// What Jarlid knows about your listening that Pandora will not tell it.
//
// Neither `user.getStationList` nor `station.getStation` carries a play count or a
// last-played time — checked, and written down in `plans/station-galaxy.md`. So two of the
// Stations page's four sort orders have to be counted here, against the only listening this
// client can honestly account for: listening done through Jarlid. A fresh install sorts flat
// and fills in as you play things, which is the truthful behaviour rather than a guess.
//
// Keyed by station **token**, never by name: two stations can share a name, and merging their
// tallies would quietly make both wrong.

const KEY = "station-stats";

export interface StationStat {
  /// Tracks started while this station was the one playing.
  plays: number;
  /// When it was last switched to, epoch ms.
  last: number;
  /// Art of the last track played from it — the fallback when Pandora has no station cover.
  art?: string;
}

type Stats = Record<string, StationStat>;

function read(): Stats {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) || "{}");
    return raw && typeof raw === "object" ? (raw as Stats) : {};
  } catch {
    return {};
  }
}

let stats: Stats = read();

function write() {
  try {
    localStorage.setItem(KEY, JSON.stringify(stats));
  } catch {
    // A full quota is not worth interrupting playback over; the tallies are a sort key.
  }
}

export function statFor(token: string): StationStat | undefined {
  return stats[token];
}

/** The station we are on, as `engine://station-active` reports it. */
let activeToken = "";

export function noteActive(token: string) {
  if (!token || token === activeToken) return;
  activeToken = token;
  const s = (stats[token] ||= { plays: 0, last: 0 });
  s.last = Date.now();
  write();
}

export function active() {
  return activeToken;
}

/**
 * A track started. Counts one play against whatever station is on, and remembers its art.
 *
 * Counting track starts rather than station switches is the point: leaving one station on all
 * afternoon should outrank flicking through six, and switching back and forth must not inflate
 * anything. On a blend the art is still the blend's — you tuned to the blend.
 */
export function noteTrack(art: string) {
  if (!activeToken) return;
  const s = (stats[activeToken] ||= { plays: 0, last: 0 });
  s.plays += 1;
  s.last = Date.now();
  if (art) s.art = art;
  write();
}

/** Drop tallies for stations that no longer exist, so a deleted station stops taking room. */
export function prune(liveTokens: Iterable<string>) {
  const live = new Set(liveTokens);
  if (!live.size) return; // an empty list means "not loaded yet", not "you have none"
  let changed = false;
  for (const token of Object.keys(stats)) {
    if (!live.has(token)) {
      delete stats[token];
      changed = true;
    }
  }
  if (changed) write();
}
