// The lyric editor's lines, and the edits on them that don't need the page: splitting and
// joining lines, keeping a draft's review state and each line's word timings straight through
// both, and converting to and from the `timing` a Lyricsfile carries. Kept apart from
// lyric-editor.ts, which owns the DOM, so it can be tested.

export interface Line {
  t: number | null;
  text: string;
  /// Reviewing a draft: the words the engine was unsure of, until the line is edited, and
  /// whether a person has checked the line.
  doubt?: number[];
  ok?: boolean;
  /// When each word starts, by position in `words(text)`; null for a word not yet timed.
  /// Absent when the line has no word timing at all.
  words?: (number | null)[];
}

/// One line of `Lyrics.timing` (`lyricsfile::TimedLine` in Rust). An empty `text` is a pause.
export interface TimedLine {
  text: string;
  start: number;
  end?: number | null;
  words?: { text: string; start: number; end?: number | null }[];
}

/// A line's words, as word timings index them.
export const wordsOf = (text: string) => text.trim().split(/\s+/).filter(Boolean);

/// A draft's review state, as `Lyrics.draft` carries it: by line, blank lines included.
export interface DraftReview {
  doubt: number[][];
  reviewed: boolean[];
}

/// Blank lines are the gaps between verses. They have no start of their own to hear, so
/// a timing pass steps over them, and there is nothing in one to review.
export const isBlank = (l: Line) => !l.text.trim();

/// Give `lines` a draft's review state. Blank lines count as reviewed.
export function applyReview(lines: Line[], draft: DraftReview) {
  lines.forEach((line, i) => {
    line.doubt = draft.doubt[i] ?? [];
    line.ok = !!draft.reviewed[i] || isBlank(line);
  });
}

/// Lines of a draft with words that nobody has checked yet.
export const unreviewed = (lines: Line[]) => lines.filter((l) => !isBlank(l) && !l.ok).length;

/// What a saved edit records of a review: nothing once it is done, which is what lets it be
/// published (`publish_lyrics` refuses a saved edit that is still a draft).
export function reviewState(lines: Line[]): DraftReview | null {
  if (unreviewed(lines) === 0) return null;
  return {
    doubt: lines.map((l) => l.doubt ?? []),
    reviewed: lines.map((l) => !!l.ok || isBlank(l)),
  };
}

/// Split line `i`, whose text is now `text`, at character `at`; at the end, that opens an
/// empty line below. A timed line's second half starts where its share of the characters puts
/// it between this line and the next timed one: a guess, but a closer one than no time at all,
/// and in a draft it gets checked.
export function splitLine(lines: Line[], i: number, text: string, at: number, reviewing: boolean) {
  const line = lines[i];
  const head = text.slice(0, at).trimEnd();
  const tail = text.slice(at).trimStart();
  let t: number | null = null;
  if (line.t !== null && head && tail) {
    const next = lines.slice(i + 1).find((l) => l.t !== null && !isBlank(l))?.t ?? null;
    const end = next !== null && next > line.t ? next : line.t + 4;
    t = line.t + ((end - line.t) * head.length) / (head.length + tail.length);
  }
  if (line.words && head && tail) {
    // The words before the caret keep their times, and the second half starts at its first
    // word's, when that is known: a better guess than its share of the characters.
    const n = wordsOf(head).length;
    const rest = line.words.slice(n);
    line.words = line.words.slice(0, n);
    if (rest[0] != null) t = rest[0];
    line.text = head;
    const fresh: Line = { t, text: tail, words: rest };
    afterSplit(line, fresh, reviewing, true);
    lines.splice(i + 1, 0, fresh);
    return;
  }
  line.text = head;
  const fresh: Line = { t, text: tail };
  afterSplit(line, fresh, reviewing, !!(head && tail));
  lines.splice(i + 1, 0, fresh);
}

function afterSplit(line: Line, fresh: Line, reviewing: boolean, real: boolean) {
  if (!reviewing) return;
  fresh.ok = false;
  // A real split makes both halves new lines to the reviewer, and the engine's doubts
  // can't be divided by position. Opening an empty line below leaves this one as it was.
  if (real) {
    line.doubt = fresh.doubt = [];
    line.ok = false;
  }
}

/// Join line `i`, whose text is now `text`, onto the end of the line above. A blank line
/// above is simply dropped: the line keeps its own place and start. Returns where the join
/// is, in the line above's text.
export function joinLine(lines: Line[], i: number, text: string, reviewing: boolean): number {
  const above = lines[i - 1];
  if (isBlank(above)) {
    lines.splice(i - 1, 1);
    return 0;
  }
  const at = above.text.trimEnd().length + 1;
  const below = lines[i];
  // Word times survive a join when both halves had them, or the joined line has a mix.
  if (above.words || below.words) {
    above.words = [
      ...(above.words ?? wordsOf(above.text).map(() => null)),
      ...(below.words ?? wordsOf(text).map((_, k) => (k === 0 ? below.t : null))),
    ];
  }
  above.text = `${above.text.trimEnd()} ${text.trimStart()}`;
  if (reviewing) {
    above.doubt = [];
    above.ok = false;
  }
  lines.splice(i, 1);
  return at;
}

/// Line `i`'s text changed to `text`. Its word times stay if it still has as many words —
/// a spelling fixed — and go if not, since which time belongs to which word is then unknown.
export function retext(line: Line, text: string) {
  if (line.words && wordsOf(text).length !== line.words.length) delete line.words;
  line.text = text;
}

/// The editor's lines from a Lyricsfile's timing. A line's end becomes the empty line that
/// clears the display there, which is how the editor (and LRC) hold a pause.
export function fromTiming(timing: TimedLine[]): Line[] {
  const out: Line[] = [];
  timing.forEach((tl, i) => {
    const line: Line = { t: tl.start, text: tl.text };
    const n = wordsOf(tl.text).length;
    if (tl.words?.length && tl.words.length === n) line.words = tl.words.map((w) => w.start);
    out.push(line);
    const next = timing[i + 1];
    if (tl.end != null && tl.text.trim() && (!next || tl.end < next.start)) {
      const cleared = next && !next.text.trim() && Math.abs(next.start - tl.end) < 0.005;
      if (!cleared) out.push({ t: tl.end, text: "" });
    }
  });
  return out;
}

/// What a saved edit or a publication carries beyond its LRC: the timed lines with their words,
/// and each line's end where an empty line follows it. Null when no line has word times — then
/// the LRC says everything, pauses included.
export function toTiming(lines: Line[]): TimedLine[] | null {
  const timed = lines.filter((l) => l.t !== null);
  const anyWords = timed.some((l) => l.words?.some((w) => w !== null));
  if (!anyWords) return null;
  return timed.map((l, i) => {
    const out: TimedLine = { text: l.text.trim(), start: l.t! };
    const next = timed[i + 1];
    if (out.text && next && !next.text.trim()) out.end = next.t;
    const words = wordsOf(l.text);
    // Only lines whose every word has a time: the format has no untimed word.
    if (l.words && l.words.length === words.length && l.words.every((w) => w !== null)) {
      out.words = words.map((text, k) => ({ text, start: l.words![k]! }));
    }
    return out;
  });
}
