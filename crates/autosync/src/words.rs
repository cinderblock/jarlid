//! Word starts for lines whose starts are already known.
//!
//! Most synced lyrics time lines, not words: LRCLIB's LRC, or a listener tapping each line in the
//! editor. Those line starts are trusted as they are. What is missing is where each word inside
//! a line falls, and that can be read off a stream of timed words: what the speech model heard,
//! or another transcription's word times (an LRCLIB Lyricsfile whose lines don't match its LRC).
//!
//! The lyric words are matched to the timed ones in order (Needleman–Wunsch, gaps free), a word
//! only matching something heard inside its own line's span. A matched word starts where it
//! was heard; the rest are spread between their matched neighbours by syllable count, at the
//! song's own pace past the last one. A line's first word is its start, always.

use crate::text;
use crate::Heard;

/// A line whose start (and end: the next line's start, or where it stops being shown) is known.
#[derive(Debug, Clone)]
pub struct Line<'a> {
    pub text: &'a str,
    pub start: f64,
    pub end: f64,
}

/// The tunables. [`Params::default`] is what ships.
#[derive(Debug, Clone)]
pub struct Params {
    /// Below this, two words are different words, not one misheard.
    pub match_sim: f64,
    /// Seconds outside a line's span that a heard word may still match one of its words: a
    /// tapped line start lands a little late or early.
    pub slack: f64,
    /// Seconds subtracted from every heard start. Unlike the line aligner's
    /// ([`crate::align::Params::lead`], 0.15 s), zero: the engine's word starts land on sung word
    /// starts (median error 0.09 s at 0, 0.16 s at 0.15, on the ground truth in
    /// `plans/word-timings-for-line-timed-lyrics.md`).
    pub lead: f64,
    /// Seconds per syllable when nothing in the song measures it.
    pub syllable_seconds: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            match_sim: 0.6,
            slack: 0.6,
            lead: 0.0,
            syllable_seconds: 0.22,
        }
    }
}

/// One line's word starts, by position in its whitespace-separated words.
#[derive(Debug, Clone, PartialEq)]
pub struct LineWords {
    pub starts: Vec<f64>,
    /// How many of its words (the first one aside, which is the line's start) were matched to a
    /// timed word, rather than spread between matched ones.
    pub matched: usize,
    /// Which words start where they were heard. The first word is the line's start, not heard,
    /// so always `false`.
    pub heard: Vec<bool>,
}

/// The smallest gap kept between consecutive word starts, seconds.
const MIN_GAP: f64 = 0.03;

/// One normalised lyric word piece: `oh-oh-oh` is one shown word but three pieces.
struct Piece {
    line: usize,
    word: usize,
    first: bool,
    text: String,
}

pub fn time_words(lines: &[Line], heard: &[Heard], p: &Params) -> Vec<LineWords> {
    let shown: Vec<Vec<&str>> = lines
        .iter()
        .map(|l| l.text.split_whitespace().collect())
        .collect();
    let mut pieces = Vec::new();
    for (i, words) in shown.iter().enumerate() {
        for (k, w) in words.iter().enumerate() {
            for (n, t) in text::words(w).into_iter().enumerate() {
                pieces.push(Piece {
                    line: i,
                    word: k,
                    first: n == 0,
                    text: t,
                });
            }
        }
    }
    let heard_text: Vec<Vec<String>> = heard.iter().map(|h| text::words(&h.text)).collect();
    let pairs = match_pieces(&pieces, lines, heard, &heard_text, p);

    // Each shown word's heard start, when its first piece matched.
    let mut known: Vec<Vec<Option<f64>>> = shown.iter().map(|w| vec![None; w.len()]).collect();
    for (pi, hi) in pairs {
        let piece = &pieces[pi];
        let h = &heard[hi];
        if piece.first {
            known[piece.line][piece.word] = Some(h.start - p.lead);
        }
    }
    let pace = pace(&shown, &known, p.syllable_seconds);
    let fill = fill(lines, &known);

    lines
        .iter()
        .enumerate()
        .map(|(i, line)| fill_line(line, &shown[i], &known[i], pace, fill))
        .collect()
}

/// Lyric pieces ↔ heard words, monotone, each pair inside the piece's line span. Returns
/// `(piece, heard)` index pairs. Gaps cost nothing: an unheard word or a stray heard one is
/// expected, and only matches are evidence.
fn match_pieces(
    pieces: &[Piece],
    lines: &[Line],
    heard: &[Heard],
    heard_text: &[Vec<String>],
    p: &Params,
) -> Vec<(usize, usize)> {
    let (n, m) = (pieces.len(), heard.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let score = |i: usize, j: usize| -> Option<f64> {
        let piece = &pieces[i];
        let line = &lines[piece.line];
        let t = heard[j].start - p.lead;
        if t < line.start - p.slack || t > line.end + p.slack {
            return None;
        }
        // A heard word can carry two lyric pieces ("oh-oh" heard as "ohoh"); compare against
        // its first normalised piece, and the whole thing joined.
        let h = &heard_text[j];
        let joined = h.concat();
        let sim = h
            .first()
            .map(|f| text::similarity(&piece.text, f))
            .unwrap_or(0.0)
            .max(text::similarity(&piece.text, &joined));
        if sim < p.match_sim {
            return None;
        }
        // Closer to the line's span beats further outside it.
        let outside = (line.start - t).max(t - line.end).max(0.0);
        Some(sim - 0.2 * outside / p.slack.max(1e-6))
    };
    // dp[i][j]: best score for pieces[..i] against heard[..j].
    let w = m + 1;
    let mut dp = vec![0.0f64; (n + 1) * w];
    let mut from = vec![0u8; (n + 1) * w]; // 0 = skip piece, 1 = skip heard, 2 = match
    for i in 1..=n {
        for j in 1..=m {
            let mut best = dp[(i - 1) * w + j];
            let mut how = 0u8;
            if dp[i * w + j - 1] > best {
                best = dp[i * w + j - 1];
                how = 1;
            }
            if let Some(s) = score(i - 1, j - 1) {
                let v = dp[(i - 1) * w + j - 1] + s;
                if v > best {
                    best = v;
                    how = 2;
                }
            }
            dp[i * w + j] = best;
            from[i * w + j] = how;
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        match from[i * w + j] {
            2 => {
                out.push((i - 1, j - 1));
                i -= 1;
                j -= 1;
            }
            1 => j -= 1,
            _ => i -= 1,
        }
    }
    out.reverse();
    out
}

/// Syllables in a word, roughly: groups of vowels, at least one.
pub fn syllables(word: &str) -> usize {
    let mut n = 0;
    let mut in_vowel = false;
    for c in word.chars().flat_map(char::to_lowercase) {
        let v =
            matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y') || (c.is_alphabetic() && !c.is_ascii());
        if v && !in_vowel {
            n += 1;
        }
        in_vowel = v;
    }
    // A silent final e ("love", "time"), unless it's the only vowel group.
    let lower = word.to_lowercase();
    let letters: String = lower.chars().filter(|c| c.is_alphabetic()).collect();
    if n > 1 && letters.ends_with('e') && !letters.ends_with("le") && !letters.ends_with("ee") {
        n -= 1;
    }
    n.max(1)
}

/// How far into its span this song's lines put their last word: the median share, over lines of
/// three or more words whose last word was heard.
fn fill(lines: &[Line], known: &[Vec<Option<f64>>]) -> f64 {
    let mut shares: Vec<f64> = lines
        .iter()
        .zip(known)
        .filter(|(l, k)| k.len() >= 3 && l.end > l.start)
        .filter_map(|(l, k)| {
            k.last()
                .copied()
                .flatten()
                .map(|t| (t - l.start) / (l.end - l.start))
        })
        .filter(|s| (0.0..1.0).contains(s))
        .collect();
    if shares.len() < 5 {
        return DEFAULT_FILL;
    }
    shares.sort_by(f64::total_cmp);
    shares[shares.len() / 2].clamp(0.4, 0.95)
}

/// How far into its span a line's last word starts, when the song doesn't say.
const DEFAULT_FILL: f64 = 0.75;

/// Seconds per syllable in this song: the median over consecutive matched words of a line.
fn pace(shown: &[Vec<&str>], known: &[Vec<Option<f64>>], fallback: f64) -> f64 {
    let mut rates = Vec::new();
    for (words, times) in shown.iter().zip(known) {
        for k in 1..words.len() {
            if let (Some(a), Some(b)) = (times[k - 1], times[k]) {
                let dt = b - a;
                if dt > 0.0 && dt < 2.0 {
                    rates.push(dt / syllables(words[k - 1]) as f64);
                }
            }
        }
    }
    if rates.len() < 8 {
        return fallback;
    }
    rates.sort_by(f64::total_cmp);
    rates[rates.len() / 2].clamp(0.1, 0.6)
}

fn fill_line(
    line: &Line,
    words: &[&str],
    known: &[Option<f64>],
    pace: f64,
    fill: f64,
) -> LineWords {
    let n = words.len();
    if n == 0 {
        return LineWords {
            starts: Vec::new(),
            matched: 0,
            heard: Vec::new(),
        };
    }
    let limit = line.end.max(line.start + MIN_GAP * n as f64);
    // Anchors: the line's start, then every matched word that keeps the order and leaves room
    // for the words before it.
    let mut anchors: Vec<(usize, f64)> = vec![(0, line.start)];
    for k in 1..n {
        let Some(t) = known[k] else { continue };
        let (pk, pt) = *anchors.last().unwrap();
        let room = MIN_GAP * (k - pk) as f64;
        if t >= pt + room && t <= limit - MIN_GAP * (n - k) as f64 {
            anchors.push((k, t));
        }
    }
    let matched = anchors.len() - 1;
    let syl: Vec<f64> = words.iter().map(|w| syllables(w) as f64).collect();
    let mut starts = vec![0.0; n];
    for pair in anchors.windows(2) {
        let ((a, ta), (b, tb)) = (pair[0], pair[1]);
        spread(&mut starts, &syl, a, ta, b, tb);
    }
    // After the last anchor. In a line nothing was heard of, the last word starts as far into
    // the line as this song's lines usually put it: singers stretch a line to its bar, and a
    // fixed pace ends unheard lines early (Wings: p90 0.44 → 0.16 s). After a heard word, the
    // words that follow keep the song's pace.
    let (a, ta) = *anchors.last().unwrap();
    starts[a] = ta;
    if a + 1 < n {
        let last = if matched == 0 {
            line.start + fill * (limit - line.start)
        } else {
            ta + pace * syl[a..n - 1].iter().sum::<f64>()
        };
        let last = last
            .min(limit - MIN_GAP)
            .max(ta + MIN_GAP * (n - 1 - a) as f64);
        spread(&mut starts, &syl, a, ta, n - 1, last);
    }
    for k in 1..n {
        if starts[k] < starts[k - 1] + MIN_GAP {
            starts[k] = starts[k - 1] + MIN_GAP;
        }
    }
    let mut heard = vec![false; n];
    for &(k, _) in &anchors[1..] {
        heard[k] = true;
    }
    LineWords {
        starts,
        matched,
        heard,
    }
}

/// Words `a..b` between anchors at `ta` and `tb`, each taking time by its syllables.
fn spread(starts: &mut [f64], syl: &[f64], a: usize, ta: f64, b: usize, tb: f64) {
    let total: f64 = syl[a..b].iter().sum();
    let mut t = ta;
    starts[a] = ta;
    for k in a + 1..b {
        t += (tb - ta) * syl[k - 1] / total;
        starts[k] = t;
    }
    starts[b] = tb;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(words: &[(&str, f64)]) -> Vec<Heard> {
        words
            .iter()
            .map(|&(text, start)| Heard {
                text: text.into(),
                start,
                end: start + 0.2,
                p: 1.0,
            })
            .collect()
    }

    fn params() -> Params {
        Params::default()
    }

    #[test]
    fn matched_words_start_where_heard_and_the_first_is_the_line() {
        let lines = [Line {
            text: "I took a walk",
            start: 10.0,
            end: 14.0,
        }];
        let h = heard(&[("i", 10.2), ("took", 10.5), ("a", 11.0), ("walk", 11.4)]);
        let out = time_words(&lines, &h, &params());
        assert_eq!(out[0].starts, vec![10.0, 10.5, 11.0, 11.4]);
        assert_eq!(out[0].matched, 3);
    }

    #[test]
    fn unheard_words_are_spread_by_syllables() {
        let lines = [Line {
            text: "one beautiful day",
            start: 0.0,
            end: 10.0,
        }];
        // "day" heard at 2.0; "beautiful" not heard. one (1 syl) then beautiful (3).
        let h = heard(&[("day", 2.0)]);
        let out = time_words(&lines, &h, &params());
        assert!((out[0].starts[1] - 0.5).abs() < 1e-9, "{:?}", out[0].starts);
        assert_eq!(out[0].starts[2], 2.0);
    }

    #[test]
    fn a_word_heard_in_another_line_does_not_count() {
        let lines = [
            Line {
                text: "leader leader",
                start: 0.0,
                end: 2.0,
            },
            Line {
                text: "no leader",
                start: 2.0,
                end: 4.0,
            },
        ];
        let h = heard(&[
            ("leader", 0.1),
            ("leader", 1.0),
            ("no", 2.1),
            ("leader", 3.0),
        ]);
        let out = time_words(&lines, &h, &params());
        assert_eq!(out[0].starts, vec![0.0, 1.0]);
        assert_eq!(out[1].starts, vec![2.0, 3.0]);
    }

    #[test]
    fn hyphenated_words_take_their_first_piece() {
        let lines = [Line {
            text: "say oh-oh-oh now",
            start: 0.0,
            end: 5.0,
        }];
        let h = heard(&[
            ("say", 0.0),
            ("oh", 1.0),
            ("oh", 1.3),
            ("oh", 1.6),
            ("now", 2.5),
        ]);
        let out = time_words(&lines, &h, &params());
        assert_eq!(out[0].starts, vec![0.0, 1.0, 2.5]);
    }

    #[test]
    fn nothing_heard_still_gives_ordered_starts_inside_the_line() {
        let lines = [Line {
            text: "a b c d",
            start: 5.0,
            end: 6.0,
        }];
        let out = time_words(&lines, &[], &params());
        let s = &out[0].starts;
        assert_eq!(s[0], 5.0);
        assert!(s.windows(2).all(|w| w[1] > w[0]) && s[3] < 6.0, "{s:?}");
        assert_eq!(out[0].matched, 0);
    }

    #[test]
    fn out_of_order_matches_are_dropped() {
        let lines = [Line {
            text: "x y z",
            start: 0.0,
            end: 3.0,
        }];
        // A heard "y" before the line's own start can't move word 1 before word 0.
        let h = heard(&[("y", -0.3), ("z", 2.0)]);
        let out = time_words(&lines, &h, &params());
        assert!(
            out[0].starts[1] > 0.0 && out[0].starts[2] == 2.0,
            "{:?}",
            out[0].starts
        );
    }

    #[test]
    fn syllable_counts() {
        assert_eq!(syllables("love"), 1);
        assert_eq!(syllables("beautiful"), 3);
        assert_eq!(syllables("reaper"), 2);
        assert_eq!(syllables("&"), 1);
    }
}
