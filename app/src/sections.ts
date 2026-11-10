// Section labels — "Chorus:", "[Verse 2]", "(Bridge)" — are structure, not lyrics.
//
// LRCLIB's plain lyrics often carry them. They were written for reading, and the Lyricsfile spec
// allows them in `plain` only: "`plain` may include section labels … not present in `lines`".
// So the pane shows a label as a heading over its section and never as a sung line. Timed
// lyrics get their headings by lining the plain text up with the timed lines, and edits
// keep them. `crates/autosync/src/text.rs` (`is_section_label`) is this recogniser's Rust twin,
// used to keep labels out of alignment. Change both together. See `plans/lyric-sections.md`.

/// Section names, as written once recognised. Keys are the name's words, lowercased, with
/// hyphens as spaces.
const NAMES: Record<string, string> = {
  intro: "Intro",
  verse: "Verse",
  "pre chorus": "Pre-Chorus",
  prechorus: "Pre-Chorus",
  chorus: "Chorus",
  "post chorus": "Post-Chorus",
  postchorus: "Post-Chorus",
  hook: "Hook",
  refrain: "Refrain",
  bridge: "Bridge",
  interlude: "Interlude",
  breakdown: "Breakdown",
  break: "Break",
  instrumental: "Instrumental",
  solo: "Solo",
  outro: "Outro",
  coda: "Coda",
  drop: "Drop",
  tag: "Tag",
};

/// Names that can also be a whole sung line ("Break", "Drop", "Solo", "Tag", "Hook") count
/// as labels only when the line says so another way: brackets, a colon or a number.
const BARE_OK = new Set([
  "Intro",
  "Verse",
  "Pre-Chorus",
  "Chorus",
  "Post-Chorus",
  "Refrain",
  "Bridge",
  "Interlude",
  "Instrumental",
  "Outro",
]);

const ROMAN = /^(i|ii|iii|iv|v|vi|vii|viii|ix|x)$/;
const ROMAN_VALUE: Record<string, number> = { i: 1, ii: 2, iii: 3, iv: 4, v: 5, vi: 6, vii: 7, viii: 8, ix: 9, x: 10 };
const CLOSE: Record<string, string> = { "[": "]", "(": ")", "{": "}" };

/// The section `line` labels, written for display ("Verse 2", "Chorus ×2", "Verse 1 · Artist"),
/// or `null` when it is a lyric.
export function sectionLabel(line: string): string | null {
  let s = line.trim();
  if (!s || s.length > 60) return null;
  let marked = false; // said to be a label by brackets or a colon, not only by its words
  const close = CLOSE[s[0]];
  if (close && s.endsWith(close)) {
    s = s.slice(1, -1).trim();
    marked = true;
  }
  // Who sings it, after a colon or dash inside the brackets: "[Verse 2: Artist]".
  let extra = "";
  const sep = marked ? s.search(/\s*(?::|\s[-–—]\s)/) : -1;
  if (sep > 0) {
    extra = s.slice(sep).replace(/^\s*(?::|[-–—])\s*/, "").trim();
    s = s.slice(0, sep);
  } else if (s.endsWith(":")) {
    s = s.slice(0, -1).trim();
    marked = true;
  }
  // How many times: "Chorus x2", "Chorus (x2)", "Chorus ×2", "2x Chorus".
  let times = 0;
  const rep = s.match(/(?:\s+|\s*\()\s*[x×]\s*(\d+)\s*\)?$/i) ?? s.match(/(?:\s+|\s*\()\s*(\d+)\s*[x×]\s*\)?$/i);
  if (rep && rep.index! > 0) {
    times = Number(rep[1]);
    s = s.slice(0, rep.index);
  } else {
    const lead = s.match(/^(\d+)\s*[x×]\s+/i);
    if (lead) {
      times = Number(lead[1]);
      s = s.slice(lead[0].length);
    }
  }
  const words = s.toLowerCase().replace(/[-–]/g, " ").split(/\s+/).filter(Boolean);
  let num = "";
  const last = words[words.length - 1];
  if (words.length > 1 && (/^\d{1,2}$/.test(last) || ROMAN.test(last))) {
    num = /^\d+$/.test(last) ? String(Number(last)) : String(ROMAN_VALUE[last]);
    words.pop();
  }
  const name = NAMES[words.join(" ")];
  if (!name) return null;
  if (!marked && !num && !BARE_OK.has(name)) return null;
  let out = num ? `${name} ${num}` : name;
  if (extra) out += ` · ${extra}`;
  if (times > 1) out += ` ×${times}`;
  return out;
}

/// Text lines with the labels taken out. `heads[i]` is the label shown over `lines[i]`. A label
/// heads the first line after it that has words, and one with no line after it is dropped.
export function splitSections(text: string[]): { lines: string[]; heads: (string | undefined)[] } {
  const lines: string[] = [];
  const heads: (string | undefined)[] = [];
  let pending: string | undefined;
  for (const t of text) {
    const label = sectionLabel(t);
    if (label) {
      pending = label;
      continue;
    }
    lines.push(t);
    if (pending && t.trim()) {
      heads[lines.length - 1] = pending;
      pending = undefined;
    } else heads[lines.length - 1] = undefined;
  }
  return { lines, heads };
}

const norm = (s: string) => s.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "");

/// Headings for timed lines (`sung`, in order) from the labels in `plain`: each plain line
/// with a label is lined up with its timed line by their words, in order. Lines that differ
/// a little (an edit, a fixed typo) match too, as long as the lines around them still do.
export function sectionsFor(plain: string | null | undefined, sung: string[]): (string | undefined)[] {
  const out: (string | undefined)[] = new Array(sung.length).fill(undefined);
  if (!plain) return out;
  const split = splitSections(plain.split(/\r?\n/));
  if (!split.heads.some(Boolean)) return out;
  // The plain side: its lines with words, each with the label pending over it.
  const P: { key: string; head?: string }[] = [];
  let carry: string | undefined;
  split.lines.forEach((t, i) => {
    carry = split.heads[i] ?? carry;
    if (!t.trim()) return;
    P.push({ key: norm(t), head: carry });
    carry = undefined;
  });
  const T = sung.map((t, i) => ({ key: norm(t), i })).filter((x) => x.key);
  // Longest common subsequence over whole lines, then fill the gaps between matches in order.
  const n = P.length;
  const m = T.length;
  const L: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let a = n - 1; a >= 0; a--)
    for (let b = m - 1; b >= 0; b--)
      L[a][b] = P[a].key === T[b].key ? L[a + 1][b + 1] + 1 : Math.max(L[a + 1][b], L[a][b + 1]);
  const pairs: [number, number][] = [];
  for (let a = 0, b = 0; a < n && b < m; ) {
    if (P[a].key === T[b].key) pairs.push([a++, b++]);
    else if (L[a + 1][b] >= L[a][b + 1]) a++;
    else b++;
  }
  pairs.push([n, m]);
  let pa = 0;
  let pb = 0;
  for (const [a, b] of pairs) {
    // Between two matches: as many unmatched lines on each side are paired up, in order.
    for (let k = 0; k < Math.min(a - pa, b - pb); k++) {
      const h = P[pa + k].head;
      if (h) out[T[pb + k].i] = h;
    }
    // A label over plain lines that have no timed line goes to the next timed line.
    for (let k = Math.min(a - pa, b - pb); k < a - pa; k++) {
      const h = P[pa + k].head;
      if (h && a < n && !P[a].head) P[a] = { ...P[a], head: h };
    }
    if (a < n && b < m && P[a].head) out[T[b].i] = P[a].head;
    pa = a + 1;
    pb = b + 1;
  }
  return out;
}

/// Timed lines with any labels timed as lines of their own taken out (some LRC files do that),
/// and where those labels were.
export function dropLabels<T extends { t: number; text: string }>(
  parsed: T[]
): { lines: T[]; marks: { t: number; label: string }[] } {
  const lines: T[] = [];
  const marks: { t: number; label: string }[] = [];
  for (const l of parsed) {
    const label = sectionLabel(l.text);
    if (label) marks.push({ t: l.t, label });
    else lines.push(l);
  }
  return { lines, marks };
}

/// Each timed line's heading: from the labels the LRC itself timed (`marks`), each over the
/// first line with words at or after it, or else from the plain text's labels.
export function timedHeads(
  lines: { t: number; text: string }[],
  marks: { t: number; label: string }[],
  plain: string | null | undefined
): (string | undefined)[] {
  if (!marks.length) return sectionsFor(plain, lines.map((l) => l.text));
  const out: (string | undefined)[] = new Array(lines.length).fill(undefined);
  for (const m of marks) {
    const i = lines.findIndex((l) => l.t >= m.t && l.text.trim());
    if (i >= 0) out[i] = m.label;
  }
  return out;
}

/// The plain form of edited lines, with the labels `oldPlain` had put back over the lines
/// they headed. When the edited lines carry labels of their own, those win.
export function withSections(oldPlain: string | null | undefined, text: string[]): string {
  if (text.some((t) => sectionLabel(t))) return text.join("\n").trim();
  const heads = sectionsFor(oldPlain, text);
  const out: string[] = [];
  text.forEach((t, i) => {
    const h = heads[i];
    if (h) {
      if (out.length && out[out.length - 1].trim()) out.push("");
      out.push(`[${h.replace(" · ", ": ").replace(/ ×(\d+)$/, " x$1")}]`);
    }
    out.push(t);
  });
  return out.join("\n").trim();
}
