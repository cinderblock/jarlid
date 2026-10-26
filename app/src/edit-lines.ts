// The lyric editor's lines, and the edits on them that don't need the page: splitting and
// joining lines, and keeping a draft's review state straight through both. Kept apart from
// lyric-editor.ts, which owns the DOM, so it can be tested.

export interface Line {
  t: number | null;
  text: string;
  /// Reviewing a draft: the words the engine was unsure of, until the line is edited, and
  /// whether a person has checked the line.
  doubt?: number[];
  ok?: boolean;
}

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
  line.text = head;
  const fresh: Line = { t, text: tail };
  if (reviewing) {
    fresh.ok = false;
    // A real split makes both halves new lines to the reviewer, and the engine's doubts
    // can't be divided by position. Opening an empty line below leaves this one as it was.
    if (head && tail) {
      line.doubt = fresh.doubt = [];
      line.ok = false;
    }
  }
  lines.splice(i + 1, 0, fresh);
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
  above.text = `${above.text.trimEnd()} ${text.trimStart()}`;
  if (reviewing) {
    above.doubt = [];
    above.ok = false;
  }
  lines.splice(i, 1);
  return at;
}
