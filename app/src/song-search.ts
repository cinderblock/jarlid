// Matching for the Find a song panel. Pure, so it can be tested without a DOM.

export interface SavedSong {
  id: string;
  title: string;
  artist: string;
  album: string;
  /** Seconds. */
  length: number;
  art: string;
  thumbUp: boolean;
  /** Epoch milliseconds: when it last played, from the file's modified time. */
  playedAt: number;
}

/**
 * Lower-case, accents off, punctuation to spaces. "Beyoncé" is found by typing "beyonce", and
 * "AC/DC" by "ac dc" or "acdc" (see `matches`).
 */
export function fold(text: string): string {
  return text
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim();
}

/**
 * Whether `song` matches every word of `query`, each anywhere in the title, artist or album.
 * Each word is also tried against the text with its spaces removed, so "acdc" finds "AC/DC".
 * An empty query matches everything.
 */
export function matches(song: SavedSong, query: string): boolean {
  const words = fold(query).split(" ").filter(Boolean);
  if (!words.length) return true;
  const hay = fold(`${song.title} ${song.artist} ${song.album}`);
  const squashed = hay.replace(/ /g, "");
  return words.every((w) => hay.includes(w) || squashed.includes(w));
}

/**
 * The songs matching `query`, best first: a title that starts with the query, then any title
 * match, then everything else. Within each, the order given (most recently played first).
 */
export function search(songs: SavedSong[], query: string): SavedSong[] {
  const q = fold(query);
  if (!q) return songs.slice();
  const rank = (s: SavedSong) => {
    const title = fold(s.title);
    if (title.startsWith(q)) return 0;
    if (title.includes(q)) return 1;
    return 2;
  };
  return songs
    .filter((s) => matches(s, query))
    .map((s, i) => ({ s, i, r: rank(s) }))
    .sort((a, b) => a.r - b.r || a.i - b.i)
    .map((x) => x.s);
}

/** 3:07, or 1:02:03 for something over an hour. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}
