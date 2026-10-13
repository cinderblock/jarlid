// Lyrics in two languages at once.
//
// LRC has one text per line and LRCLIB's Lyricsfile has nothing for a second voice or a
// translation, so uploaders pack both into the line however they like. The records for
// Madelline's "Dopamine (Split Brain Version)" alone use `FR - EN`, `EN (FR)` and
// `FR (EN)`, and translated files elsewhere put two lines on one timestamp.
//
// Getting this wrong is worse than not doing it. Parentheses usually mean backing vocals
// ("I love you (love you)", "(ooh)"), and splitting those off as a "translation" would mangle
// ordinary songs. So a file is treated as two languages only when the whole file supports it:
//
//   - most lyric lines split the same way;
//   - the halves are about the same length, as a translation is and an echo isn't;
//   - the halves share almost no words, which an echo does;
//   - neither half is made of vocables ("ooh yeah");
//   - where the language of each side can be told, they differ.
//
// Anything short of that is shown exactly as written.

export interface DualLine {
  t: number | null;
  text: string;
  /// The same line in the other language, when this line had one.
  alt?: string;
}

export interface Dual<L extends DualLine = DualLine> {
  lines: L[];
  /// Short labels for each side ("FR", "EN"); null where the language can't be told.
  labels: [string | null, string | null];
}

/// Fraction of lyric lines that must split the same way.
const MIN_SHARE = 0.6;
/// And never fewer than this many, so a short file can't qualify by accident.
const MIN_LINES = 4;

const VOCABLES = new Set(
  "ooh oh ohh oooh ah ahh aah yeah yeh yea hey ha la na da ra di doo ba dum sha mm mmm mmh hmm uh woah whoa wo ay ayy eh baby".split(
    " "
  )
);

const words = (s: string) =>
  s
    .toLowerCase()
    .split(/[^\p{L}\p{N}'’-]+/u)
    .filter(Boolean);

/// A line with nothing to translate: "Ooh-ooh-ooh", "(Yeah)", an instrumental marker.
function isFiller(s: string) {
  const w = words(s).flatMap((x) => x.split("-")).filter(Boolean);
  return w.length === 0 || w.every((x) => VOCABLES.has(x) || /^(o+h*|a+h*|m+|n+a+|l+a+)$/.test(x));
}

/// How two halves of a line relate:
///
/// - `translation`: one line said twice in two languages. Evidence for a dual file.
/// - `alike`: the halves share most of their words. That is an echo ("I love you (love you)")
///   or a line that reads the same in both languages ("ma D-O-P-A-M-I-N-E" / "my
///   D-O-P-A-M-I-N-E"). It is evidence neither way: not counted for a file, nor against it.
/// - `none`: not two halves of anything; a backing vocal, a stray bracket.
type Halves = "translation" | "alike" | "none";

function relate(a: string, b: string): Halves {
  if (isFiller(a) || isFiller(b)) return "none";
  // Lengths are only comparable within a script: a line of Korean is a third the
  // characters of the same line in English. An echo is always in its line's script, so
  // two scripts already rule one out.
  const ratio = a.length / b.length;
  if (latin(a) === latin(b) && (ratio < 0.4 || ratio > 2.5)) return "none";
  return sharesWords(a, b) ? "alike" : "translation";
}

/// Whether most of a text's letters are Latin.
function latin(s: string) {
  const letters = s.match(/\p{L}/gu) ?? [];
  const lat = letters.filter((c) => /\p{Script=Latin}/u.test(c)).length;
  return lat * 2 >= letters.length;
}

function translationLike(a: string, b: string) {
  return relate(a, b) === "translation";
}

function sharesWords(a: string, b: string) {
  const wa = words(a);
  const wb = words(b);
  // Shared words: an echo repeats the line, a translation (mostly) doesn't. Short words
  // are skipped since "a", "la" and "me" are words in more than one language.
  const long = (w: string[]) => new Set(w.filter((x) => x.length >= 3));
  const sa = long(wa);
  const sb = long(wb);
  if (sa.size && sb.size) {
    let shared = 0;
    for (const x of sa) if (sb.has(x)) shared++;
    if (shared / Math.min(sa.size, sb.size) > 0.34) return true;
  }
  return false;
}

/// The ways a line can carry a second language, each returning the two halves or null.
const SPLITTERS: Record<string, (s: string) => [string, string] | null> = {
  dash: (s) => {
    const parts = s.split(" - ");
    return parts.length === 2 ? [parts[0].trim(), parts[1].trim()] : null;
  },
  slash: (s) => {
    const parts = s.split(/\s+\/\s+/);
    return parts.length === 2 ? [parts[0].trim(), parts[1].trim()] : null;
  },
  bar: (s) => {
    const parts = s.split(/\s+\|\s+/);
    return parts.length === 2 ? [parts[0].trim(), parts[1].trim()] : null;
  },
  // "Line (translation)": the bracket must close the line.
  paren: (s) => {
    const m = /^(.+?)\s*[(（]([^()（）]+)[)）]\s*$/.exec(s.trim());
    return m ? [m[1].trim(), m[2].trim()] : null;
  },
};

/// Split `lines` into two languages, or return null when it isn't confidently that.
export function detectDual<L extends DualLine>(lines: L[]): Dual<L> | null {
  return pairedByTime(lines) ?? splitInline(lines);
}

/// Two lines on the same timestamp, the second a translation of the first.
function pairedByTime<L extends DualLine>(lines: L[]): Dual<L> | null {
  const lyric = lines.filter((l) => l.text && !isFiller(l.text));
  if (lyric.length < MIN_LINES * 2) return null;
  const out: L[] = [];
  const sides: [string[], string[]] = [[], []];
  let paired = 0;
  for (let i = 0; i < lines.length; i++) {
    const a = lines[i];
    const b = lines[i + 1];
    const c = lines[i + 2];
    const sameTime = (x?: L, y?: L) => x && y && x.t != null && y.t != null && Math.abs(x.t - y.t) < 0.01;
    // Exactly two on one time: three or more is a chorus of voices, not a translation.
    if (sameTime(a, b) && !sameTime(b, c) && a.text && b!.text && translationLike(a.text, b!.text)) {
      out.push({ ...a, alt: b!.text });
      sides[0].push(a.text);
      sides[1].push(b!.text);
      paired++;
      i++;
    } else {
      out.push(a);
    }
  }
  return accept(out, paired, lyric.length / 2, sides);
}

/// Both languages in one line, joined the same way on most lines.
function splitInline<L extends DualLine>(lines: L[]): Dual<L> | null {
  const lyric = lines.filter((l) => l.text && !isFiller(l.text));
  if (lyric.length < MIN_LINES) return null;
  let best: { out: L[]; count: number; of: number; sides: [string[], string[]] } | null = null;
  for (const split of Object.values(SPLITTERS)) {
    const sides: [string[], string[]] = [[], []];
    let count = 0;
    let alike = 0;
    const out = lines.map((l) => {
      const halves = l.text ? split(l.text) : null;
      const how = halves ? relate(halves[0], halves[1]) : "none";
      if (how === "none") return l;
      if (how === "alike") alike++;
      else {
        count++;
        sides[0].push(halves![0]);
        sides[1].push(halves![1]);
      }
      return { ...l, text: halves![0], alt: halves![1] };
    });
    if (!best || count > best.count) best = { out, count, of: lyric.length - alike, sides };
  }
  return best ? accept(best.out, best.count, best.of, best.sides) : null;
}

function accept<L extends DualLine>(
  out: L[],
  count: number,
  of: number,
  sides: [string[], string[]]
): Dual<L> | null {
  if (count < MIN_LINES || count < of * MIN_SHARE) return null;
  const a = guessLanguage(sides[0].join("\n"));
  const b = guessLanguage(sides[1].join("\n"));
  // The same language on both sides is a line and its echo, however consistently written.
  if (a && a === b) return null;
  return { lines: out, labels: [a?.toUpperCase() ?? null, b?.toUpperCase() ?? null] };
}

// ---- telling the languages apart ----------------------------------------------------
//
// Only well enough to label the two sides and to reject "both sides are English". A wrong
// or missing guess costs a label, never the split itself.

const SCRIPTS: [RegExp, string][] = [
  [/[가-힯]/g, "ko"],
  [/[぀-ヿ]/g, "ja"],
  [/[一-鿿]/g, "zh"],
  [/[Ѐ-ӿ]/g, "ru"],
  [/[֐-׿]/g, "he"],
  [/[؀-ۿ]/g, "ar"],
  [/[ऀ-ॿ]/g, "hi"],
  [/[฀-๿]/g, "th"],
  [/[Ͱ-Ͽ]/g, "el"],
];

const STOPWORDS: Record<string, string> = {
  en: "the and you i to my me it's is in of your that we don't i'm with for be what when it this",
  fr: "je tu le les et des un une est que qui pas mon mes moi toi c'est j'ai dans pour avec nous vous suis",
  es: "el los las y que en una es no mi yo lo se como pero por con para porque está",
  pt: "o os e que em um uma é não eu você meu minha com pra se mas",
  de: "der die das und ich du nicht ist ein eine mit mich dich mein dein wir sie zu auf",
  it: "il e di che non un io mi ti per con sono è ma come della",
};
const STOP = Object.fromEntries(
  Object.entries(STOPWORDS).map(([lang, ws]) => [lang, new Set(ws.split(" "))])
);

export function guessLanguage(text: string): string | null {
  // A non-Latin script settles it. Japanese is told from Chinese by its kana.
  let script: string | null = null;
  let most = 0;
  for (const [re, lang] of SCRIPTS) {
    const n = text.match(re)?.length ?? 0;
    if (n > most) [script, most] = [lang, n];
  }
  if (script === "zh" && /[぀-ヿ]/.test(text)) script = "ja";
  if (script && most >= 4) return script;

  const counts: Record<string, number> = {};
  for (const w of words(text)) {
    for (const lang in STOP) if (STOP[lang].has(w)) counts[lang] = (counts[lang] ?? 0) + 1;
  }
  const ranked = Object.entries(counts).sort((x, y) => y[1] - x[1]);
  if (!ranked.length) return null;
  const [lang, n] = ranked[0];
  const runnerUp = ranked[1]?.[1] ?? 0;
  return n >= 4 && n >= runnerUp * 1.5 ? lang : null;
}
