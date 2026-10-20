// Lyrics flags: the shapes stored by `lyric_flags.rs`, and the rules for which rows a flag
// covers. Kept apart from the pane code (lyric-flags.ts) so the rules can be tested alone.

import type { Lyrics } from "./lyric-editor";

/// A stretch of the song. Rows are the pane's, inclusive; times are on the lyrics' clock.
export interface Span {
  first: number;
  last: number;
  start: number | null;
  end: number | null;
  text: string[];
}

/// A copy of the lyrics on screen when a flag was made. `autosync` is filled in by the
/// backend and is opaque here.
export interface Shown {
  source: string;
  id: number | null;
  synced: string | null;
  plain: string | null;
  auto: boolean;
  overridden: boolean;
  pinned: boolean;
  offset: number;
  autosync?: unknown;
}

export interface Flag {
  id: string;
  created: number;
  /// Null flags the whole song.
  span: Span | null;
  tags: string[];
  note: string;
  shown: Shown;
}

export interface TrackFlags {
  artist: string;
  track: string;
  album: string | null;
  duration: number | null;
  flags: Flag[];
}

/// A pane row: what the highlighter and the flags both count.
export interface Row {
  t: number | null;
  text: string;
  alt?: string;
}

/// The kinds of wrong offered so far. Stored as plain strings, so more can be added freely.
export const TAGS: { id: string; label: string; whole?: boolean }[] = [
  { id: "version", label: "Wrong version", whole: true },
  { id: "timing", label: "Timing" },
  { id: "words", label: "Words" },
];

/// Dispatched on `window` when flags are changed outside the pane (Settings removing them).
export const FLAGS_CHANGED = "lyric-flags-changed";

/// How soon after a row starts a quick flag still means the row before: you notice a line
/// was wrong just after you hear it.
export const QUICK_GRACE = 1.5;

export function shownOf(l: Lyrics, offset: number): Shown {
  return {
    source: l.source,
    id: l.id ?? null,
    synced: l.synced ?? null,
    plain: l.plain ?? null,
    auto: !!l.auto,
    overridden: !!l.overridden,
    pinned: !!l.pinned,
    offset,
  };
}

/// Whether a flag is about the lyrics `l`. Exactly the same text, except for auto-synced
/// lyrics: those keep LRCLIB's lines and only ever change their times (while a song is being
/// worked out, or when a newer aligner re-times it), so the rows stay the same rows and a flag
/// on them still points at the right place.
export function appliesTo(s: Shown, l: { synced: string | null; plain: string | null; auto?: boolean }) {
  if (s.auto && l.auto) return s.plain === (l.plain ?? null);
  return s.synced === (l.synced ?? null) && s.plain === (l.plain ?? null);
}

const hasWords = (r: Row | undefined) => !!r && r.text.trim() !== "";

/// The row playing at `p` on the lyrics' clock, by the highlighter's rule; -1 before the first.
export function rowAt(rows: Row[], p: number): number {
  let idx = -1;
  for (let i = 0; i < rows.length; i++) {
    const t = rows[i].t;
    if (t !== null && t <= p + 0.15) idx = i;
    else break;
  }
  return idx;
}

/// The rows a quick flag at `p` covers: the row playing, and the one before it too when this
/// one has only just started. A gap between verses takes the last line sung with it. Before
/// the first line, the first line. Null when nothing has words.
export function quickRows(rows: Row[], p: number): [number, number] | null {
  const firstWords = rows.findIndex(hasWords);
  if (firstWords < 0) return null;
  const idx = rowAt(rows, p);
  if (idx < firstWords) return [firstWords, firstWords];
  let first = idx;
  while (first > 0 && !hasWords(rows[first])) first--;
  if (first === idx && p - (rows[idx].t ?? p) < QUICK_GRACE) {
    let prev = idx - 1;
    while (prev >= 0 && !hasWords(rows[prev])) prev--;
    if (prev >= 0) first = prev;
  }
  return [first, idx];
}

const rowText = (r: Row) => (r.alt ? `${r.text} / ${r.alt}` : r.text);

/// A span over rows `a`..`b` (either order). Ends where the next row starts, or the song does.
export function spanOf(rows: Row[], a: number, b: number, duration: number | null): Span {
  const first = Math.max(0, Math.min(a, b));
  const last = Math.min(rows.length - 1, Math.max(a, b));
  const start = rows[first]?.t ?? null;
  const next = rows.slice(last + 1).find((r) => r.t !== null);
  const end = start === null ? null : (next?.t ?? duration ?? null);
  return { first, last, start, end, text: rows.slice(first, last + 1).map(rowText) };
}

/// Whether two stretches overlap, sit side by side, or have only blank rows (a verse break)
/// between them.
function touches(a: Span, b: Span, rows: Row[]) {
  const [lo, hi] = a.first <= b.first ? [a, b] : [b, a];
  if (hi.first <= lo.last + 1) return true;
  return rows.slice(lo.last + 1, hi.first).every((r) => !hasWords(r));
}

/// `f` added to `flags`, merged with any stretch on the same lyrics that it overlaps or
/// touches (see `touches`), so flagging line after line through a bad patch makes one stretch. The merged flag
/// keeps the oldest one's id (and with it, any evidence the backend stored). Returns the new
/// list and the flag `f` became.
export function addMerged(
  flags: Flag[],
  f: Flag,
  rows: Row[],
  duration: number | null
): { flags: Flag[]; flag: Flag } {
  const rest = flags.filter((o) => o.id !== f.id);
  const span = f.span;
  if (!span) return { flags: [...rest, f], flag: f };
  const joined = rest.filter(
    (o) => o.span && appliesTo(o.shown, f.shown) && touches(o.span, span, rows)
  );
  if (joined.length === 0) return { flags: [...rest, f], flag: f };
  const all = [...joined, f].sort((x, y) => x.created - y.created);
  const oldest = all[0];
  const first = Math.min(...all.map((o) => o.span!.first));
  const last = Math.max(...all.map((o) => o.span!.last));
  const notes = [...new Set(all.map((o) => o.note.trim()).filter(Boolean))];
  const flag: Flag = {
    ...oldest,
    span: spanOf(rows, first, last, duration),
    tags: [...new Set(all.flatMap((o) => o.tags))],
    note: notes.join("; "),
  };
  const gone = new Set(joined.map((o) => o.id));
  return { flags: [...rest.filter((o) => !gone.has(o.id)), flag], flag };
}

function clock(s: number) {
  const m = Math.floor(s / 60);
  return `${m}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
}

/// "Whole song", "Line 12 · 0:41–0:45", "Lines 12–14 · 0:41–0:52". Rows count from 1 here.
export function describe(f: Flag): string {
  const s = f.span;
  if (!s) return "Whole song";
  const lines = s.first === s.last ? `Line ${s.first + 1}` : `Lines ${s.first + 1}–${s.last + 1}`;
  if (s.start === null) return lines;
  return `${lines} · ${clock(s.start)}${s.end !== null ? `–${clock(s.end)}` : ""}`;
}
