// LRC text to timed lines. Shared by the pane and the version picker.

import { detectDual, type Dual } from "./dual-lyrics";

export interface LyricLine {
  t: number;
  text: string;
  /// The same line in a second language, when the file carries two (see dual-lyrics.ts).
  alt?: string;
}

export function parseLrc(lrc: string): LyricLine[] {
  const out: LyricLine[] = [];
  const re = /\[(\d+):(\d+)(?:[.:](\d+))?\]/g;
  for (const raw of lrc.split(/\r?\n/)) {
    const stamps: number[] = [];
    let m: RegExpExecArray | null;
    re.lastIndex = 0;
    while ((m = re.exec(raw)) !== null) {
      const min = parseInt(m[1], 10);
      const sec = parseInt(m[2], 10);
      const frac = m[3] ? parseInt(m[3].padEnd(3, "0").slice(0, 3), 10) / 1000 : 0;
      stamps.push(min * 60 + sec + frac);
    }
    const text = raw.replace(re, "").trim();
    for (const t of stamps) out.push({ t, text });
  }
  // Stable, so two lines sharing a timestamp keep their order — which is how one kind of
  // two-language file pairs a line with its translation.
  out.sort((a, b) => a.t - b.t);
  return out;
}

/// Whether a set of lyrics carries two languages, judged on the form that will be shown.
export function dualOf(l: { synced: string | null; plain: string | null }): Dual | null {
  if (l.synced) return detectDual(parseLrc(l.synced));
  if (l.plain) return detectDual(l.plain.split(/\r?\n/).map((text) => ({ t: null, text })));
  return null;
}
