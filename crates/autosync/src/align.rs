//! Lining known lyric words up with heard words, and reading line start times off the result.
//!
//! The word alignment is a global sequence alignment (Needleman–Wunsch) with free leading and
//! trailing *heard* words: a model on a song hears plenty that is not in the lyrics — intros,
//! ad-libs, hallucinated "Thank you for watching" over an outro — and none of that should cost
//! anything at the ends. Lyric words the model missed are a normal gap, not a free one.
//!
//! Order is what makes this work on songs at all. A chorus repeats the same words four times, so
//! a word on its own says nothing about *which* chorus it is; aligning the whole sequence at once
//! does.

use crate::text::{similarity, words};
use crate::Heard;

/// Fallback seconds-per-word, when too few words matched to measure it.
const DEFAULT_WORD_SECONDS: f64 = 0.35;

/// The aligner's tunables. [`Params::default`] is what ships; the fields are public so the bench
/// can sweep them against the corpus.
#[derive(Debug, Clone)]
pub struct Params {
    /// Below this, two words are not the same word misheard but two different words.
    pub match_sim: f64,
    /// Cost of a lyric word the model did not hear, or a heard word not in the lyrics.
    pub gap: f64,
    /// Cost of pairing two unrelated words. Worse than two gaps, so the alignment prefers to
    /// leave both unmatched rather than pretend they correspond.
    pub mismatch: f64,
    /// Matched words a line needs before its time is read off them, unless every word in the
    /// line matched. One stray match — an "it's" borrowed from the line before — is not
    /// evidence, and a line anchored on it drags every interpolated line around it.
    pub min_anchor_words: usize,
    /// Seconds subtracted from every line. Lyrics are stamped where the line *begins to be
    /// heard*, and a highlight that lands a beat early reads better than one a beat late.
    /// Measured: Parakeet's windowed word starts run 0.15 s behind LRCLIB's hand-synced lines on
    /// the 56-track corpus, and taking 0.15 s off moved lines within 0.3 s from 52% to 59%. Other
    /// engines will need their own figure.
    pub lead: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            match_sim: 0.6,
            gap: -0.4,
            mismatch: -1.0,
            min_anchor_words: 2,
            lead: 0.15,
        }
    }
}

/// When one line starts.
#[derive(Debug, Clone, PartialEq)]
pub struct LineTime {
    /// Seconds from the start of the track. `None` for a blank line, or when nothing anchors it.
    pub t: Option<f64>,
    /// Words in the line.
    pub words: usize,
    /// How many of them were matched to something heard.
    pub matched: usize,
    /// True when `t` was inferred from neighbouring lines rather than read off a matched word in
    /// this one.
    pub interpolated: bool,
}

/// The result for a whole song.
#[derive(Debug, Clone)]
pub struct Alignment {
    /// One entry per input line, blank lines included.
    pub lines: Vec<LineTime>,
    pub lyric_words: usize,
    pub matched_words: usize,
}

impl Alignment {
    /// Share of lyric words found in what was heard.
    pub fn coverage(&self) -> f64 {
        self.matched_words as f64 / self.lyric_words.max(1) as f64
    }
}

/// Align `lines` (the lyric text, one entry per line) against `heard`.
pub fn align(lines: &[&str], heard: &[Heard]) -> Alignment {
    align_with(lines, heard, &Params::default())
}

/// [`align`] with explicit tunables.
pub fn align_with(lines: &[&str], heard: &[Heard], params: &Params) -> Alignment {
    // Every lyric word, tagged with its line.
    let mut lyric: Vec<(usize, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        lyric.extend(words(line).into_iter().map(|w| (i, w)));
    }
    let heard_words: Vec<String> = heard.iter().map(|h| words(&h.text).concat()).collect();

    let pairs = match_words(&lyric, &heard_words, params);
    let matched_words = pairs.iter().filter(|p| p.is_some()).count();

    // Seconds per word, measured from consecutive matched words, so lines can be extrapolated
    // back to a first word that was not itself matched.
    let mut gaps: Vec<f64> = Vec::new();
    let mut last: Option<(usize, f64)> = None;
    for (k, p) in pairs.iter().enumerate() {
        if let Some(j) = *p {
            let t = heard[j].start;
            if let Some((lk, lt)) = last {
                if k == lk + 1 && t > lt {
                    gaps.push(t - lt);
                }
            }
            last = Some((k, t));
        }
    }
    let word_seconds = median(&mut gaps)
        .unwrap_or(DEFAULT_WORD_SECONDS)
        .clamp(0.15, 0.8);

    // Each line from its own first matched word, stepped back over any unmatched words before it.
    let mut out: Vec<LineTime> = lines
        .iter()
        .map(|_| LineTime {
            t: None,
            words: 0,
            matched: 0,
            interpolated: false,
        })
        .collect();
    let mut first_word_of_line: Vec<Option<usize>> = vec![None; lines.len()];
    for (k, (line, _)) in lyric.iter().enumerate() {
        let entry = &mut out[*line];
        entry.words += 1;
        first_word_of_line[*line].get_or_insert(k);
        if let Some(j) = pairs[k] {
            entry.matched += 1;
            if entry.t.is_none() {
                let before = k - first_word_of_line[*line].unwrap_or(k);
                entry.t = Some(heard[j].start - before as f64 * word_seconds - params.lead);
            }
        }
    }
    for line in out.iter_mut() {
        if line.matched < params.min_anchor_words.min(line.words) {
            line.t = None;
        }
    }

    enforce_order(&mut out);
    interpolate(&mut out);

    Alignment {
        lines: out,
        lyric_words: lyric.len(),
        matched_words,
    }
}

/// For each lyric word, the index of the heard word it aligned to, if any.
fn match_words(lyric: &[(usize, String)], heard: &[String], params: &Params) -> Vec<Option<usize>> {
    let (n, m) = (lyric.len(), heard.len());
    let width = m + 1;
    let mut score = vec![0f64; (n + 1) * width];
    // 0 = diagonal, 1 = lyric gap (up), 2 = heard gap (left).
    let mut from = vec![0u8; (n + 1) * width];

    for i in 1..=n {
        score[i * width] = i as f64 * params.gap;
        from[i * width] = 1;
    }
    // Row 0 stays zero: heard words before the first lyric word are free.
    for cell in &mut from[1..=m] {
        *cell = 2;
    }

    let pair = |i: usize, j: usize| {
        let s = similarity(&lyric[i].1, &heard[j]);
        if s >= params.match_sim {
            2.0 * s - 0.5
        } else {
            params.mismatch
        }
    };

    for i in 1..=n {
        for j in 1..=m {
            let diagonal = score[(i - 1) * width + j - 1] + pair(i - 1, j - 1);
            let up = score[(i - 1) * width + j] + params.gap;
            let left = score[i * width + j - 1] + params.gap;
            let (best, dir) = if diagonal >= up && diagonal >= left {
                (diagonal, 0)
            } else if up >= left {
                (up, 1)
            } else {
                (left, 2)
            };
            score[i * width + j] = best;
            from[i * width + j] = dir;
        }
    }

    // Heard words after the last lyric word are free too: end on the best cell of the last row.
    let mut j = (0..=m)
        .max_by(|&a, &b| score[n * width + a].total_cmp(&score[n * width + b]))
        .unwrap_or(m);
    let mut i = n;
    let mut out = vec![None; n];
    while i > 0 {
        match from[i * width + j] {
            0 if j > 0 => {
                if similarity(&lyric[i - 1].1, &heard[j - 1]) >= params.match_sim {
                    out[i - 1] = Some(j - 1);
                }
                i -= 1;
                j -= 1;
            }
            2 if j > 0 => j -= 1,
            _ => i -= 1,
        }
    }
    out
}

/// Drop any line start that runs backwards relative to the lines around it.
///
/// Alignment keeps matched words in order, but stepping back over unmatched first words can
/// overshoot the previous line. The later evidence wins; the overshooting line is re-derived by
/// interpolation.
fn enforce_order(lines: &mut [LineTime]) {
    let mut latest = f64::NEG_INFINITY;
    for line in lines.iter_mut() {
        if let Some(t) = line.t {
            if t < latest {
                line.t = None;
            } else {
                latest = t;
            }
        }
    }
}

/// Give unanchored, non-blank lines a time between their anchored neighbours, spaced by word
/// count.
fn interpolate(lines: &mut [LineTime]) {
    let anchors: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].t.is_some()).collect();
    for pair in anchors.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (ta, tb) = (
            lines[a].t.unwrap_or_default(),
            lines[b].t.unwrap_or_default(),
        );
        let span: usize = (a..b).map(|i| lines[i].words).sum();
        if span == 0 {
            continue;
        }
        let mut done = lines[a].words;
        for line in lines.iter_mut().take(b).skip(a + 1) {
            if line.words > 0 {
                line.t = Some(ta + (tb - ta) * done as f64 / span as f64);
                line.interpolated = true;
            }
            done += line.words;
        }
    }
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alignment with no lead, so tests can assert exact times.
    fn exact(lines: &[&str], heard: &[Heard]) -> Alignment {
        let params = Params {
            lead: 0.0,
            ..Params::default()
        };
        align_with(lines, heard, &params)
    }

    fn heard(spec: &[(&str, f64)]) -> Vec<Heard> {
        spec.iter()
            .map(|&(text, start)| Heard {
                text: text.into(),
                start,
                end: start + 0.3,
                p: 1.0,
            })
            .collect()
    }

    #[test]
    fn lines_take_their_first_matched_word() {
        let lines = [
            "hello darkness my old friend",
            "",
            "I've come to talk with you again",
        ];
        let h = heard(&[
            ("Hello", 10.0),
            ("darkness", 10.5),
            ("my", 11.0),
            ("old", 11.3),
            ("friend", 11.7),
            ("I've", 15.0),
            ("come", 15.4),
            ("to", 15.7),
            ("talk", 16.0),
            ("with", 16.4),
            ("you", 16.7),
            ("again", 17.0),
        ]);
        let a = exact(&lines, &h);
        assert_eq!(a.lines[0].t, Some(10.0));
        assert_eq!(a.lines[1].t, None);
        assert_eq!(a.lines[2].t, Some(15.0));
        assert_eq!(a.coverage(), 1.0);
    }

    #[test]
    fn repeated_chorus_maps_in_order() {
        let lines = ["na na na", "verse words here", "na na na"];
        let h = heard(&[
            ("na", 1.0),
            ("na", 1.3),
            ("na", 1.6),
            ("verse", 5.0),
            ("words", 5.3),
            ("here", 5.6),
            ("na", 9.0),
            ("na", 9.3),
            ("na", 9.6),
        ]);
        let a = exact(&lines, &h);
        let t: Vec<Option<f64>> = a.lines.iter().map(|l| l.t).collect();
        assert_eq!(t, [Some(1.0), Some(5.0), Some(9.0)]);
    }

    #[test]
    fn noise_at_the_ends_is_free_and_missed_lines_interpolate() {
        let lines = ["first line", "never heard at all", "last line"];
        let h = heard(&[
            ("thank", 0.0),
            ("you", 0.2),
            ("first", 4.0),
            ("line", 4.4),
            ("last", 12.0),
            ("line", 12.4),
            ("subscribe", 30.0),
        ]);
        let a = exact(&lines, &h);
        assert_eq!(a.lines[0].t, Some(4.0));
        assert!(a.lines[1].interpolated);
        let mid = a.lines[1].t.unwrap();
        assert!(mid > 4.0 && mid < 12.0, "{mid}");
        assert_eq!(a.lines[2].t, Some(12.0));
    }

    #[test]
    fn unmatched_first_words_step_back() {
        let lines = ["oh yeah here we go"];
        let h = heard(&[("here", 10.0), ("we", 10.3), ("go", 10.6)]);
        let a = exact(&lines, &h);
        let t = a.lines[0].t.unwrap();
        assert!((t - (10.0 - 2.0 * 0.3)).abs() < 1e-9, "{t}");
    }

    #[test]
    fn the_default_lead_moves_every_line_earlier() {
        let lines = ["first line"];
        let h = heard(&[("first", 10.0), ("line", 10.4)]);
        assert_eq!(
            align(&lines, &h).lines[0].t,
            Some(10.0 - Params::default().lead)
        );
    }

    #[test]
    fn one_stray_word_does_not_anchor_a_long_line() {
        // "it's" is heard once, and belongs to the first line; the second line must not be
        // pinned to it, or every line interpolated around it moves too.
        let lines = [
            "because I just can't look it's killing me",
            "but it's just the price I pay",
        ];
        let h = heard(&[
            ("because", 1.0),
            ("I", 1.3),
            ("just", 1.6),
            ("can't", 1.9),
            ("look", 2.2),
            ("killing", 2.8),
            ("me", 3.1),
            ("it's", 30.0),
        ]);
        let a = exact(&lines, &h);
        assert_eq!(a.lines[0].t, Some(1.0));
        assert_eq!(a.lines[1].matched, 1);
        assert_eq!(a.lines[1].t, None);
    }

    #[test]
    fn a_short_line_fully_matched_still_anchors() {
        let lines = ["jealousy", "turning saints into the sea"];
        let h = heard(&[
            ("jealousy", 5.0),
            ("turning", 7.0),
            ("saints", 7.4),
            ("into", 7.8),
            ("the", 8.0),
            ("sea", 8.2),
        ]);
        assert_eq!(exact(&lines, &h).lines[0].t, Some(5.0));
    }
}
