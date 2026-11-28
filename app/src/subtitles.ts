// What the visualizer window's subtitles show at a moment of the song: the line being sung, or
// the word. Pure, so it can be tested without a window.

/** A timed line as the main window sends it: when it starts, its words, and when each word
 *  starts if the lyrics are timed that finely. */
export interface SubLine {
  t: number;
  text: string;
  words: number[] | null;
}

/** A line's words, split the way the pane splits them (so they line up with `words`). */
export const wordsOf = (text: string) => text.trim().split(/\s+/).filter(Boolean);

/** Seconds a word is given, at most, when the line has no word times: a line that runs on into
 *  a long instrumental would otherwise spread its last words across the whole gap. */
const MAX_WORD_S = 0.6;
/** How long the last word of a line stays up once it has been sung, at most. */
const WORD_HOLD_S = 1.5;
/** How early a line, and a word, lights: the same leads as the lyrics pane (`highlightLine` and
 *  `followWords` in main.ts), so the window and the pane change together. */
const LINE_LEAD_S = 0.15;
const WORD_LEAD_S = 0.05;

/** The line being sung at `pos`, by index, or -1 before the first one. */
export function lineAt(lines: readonly SubLine[], pos: number): number {
  let idx = -1;
  for (let i = 0; i < lines.length && lines[i].t <= pos; i++) idx = i;
  return idx;
}

/** The line to show in line mode at `pos`, by index, or -1 for none. */
export const currentLine = (lines: readonly SubLine[], pos: number) => lineAt(lines, pos + LINE_LEAD_S);

/** The text to show in line mode at `pos`, or "" for nothing (before the first line, or a
 *  pause, which LRC writes as an empty line). */
export function lineText(lines: readonly SubLine[], pos: number): string {
  const i = currentLine(lines, pos);
  return i < 0 ? "" : lines[i].text.trim();
}

/** When each word of line `i` starts: its own word times, or spread evenly from the line's
 *  start towards the next line's. */
export function wordTimes(lines: readonly SubLine[], i: number): number[] {
  const line = lines[i];
  const words = wordsOf(line.text);
  if (line.words && line.words.length === words.length) return line.words;
  const next = lines[i + 1]?.t ?? Infinity;
  const end = Math.min(next, line.t + words.length * MAX_WORD_S);
  const step = words.length ? (end - line.t) / words.length : 0;
  return words.map((_, k) => line.t + k * step);
}

/** The word being sung at `pos`, with a key that changes whenever the word does (the same word
 *  twice in a row is two words), or null for nothing. */
export function wordAt(lines: readonly SubLine[], at: number): { key: string; word: string } | null {
  const pos = at + WORD_LEAD_S;
  const i = lineAt(lines, pos);
  if (i < 0) return null;
  const words = wordsOf(lines[i].text);
  if (!words.length) return null;
  const times = wordTimes(lines, i);
  let k = -1;
  for (let j = 0; j < times.length && times[j] <= pos; j++) k = j;
  if (k < 0) return null;
  const last = times[times.length - 1];
  if (k === words.length - 1 && pos > last + WORD_HOLD_S) return null;
  return { key: `${i}:${k}`, word: words[k] };
}
