//! Sharper word starts: [`crate::words`]' placement, corrected by forced alignment.
//!
//! [`crate::words::time_words`] trusts the words the speech engine heard and spreads the rest
//! between them by syllables. Most of its large misses are those spread words. Given the
//! acoustic model's emission over each line ([`crate::ctc`]), this:
//!
//! 1. keeps every heard word as a pin, and force-aligns just the unheard words in each gap on
//!    the emission between the pins around them (after the last pin, with a star to absorb the
//!    rest of the line), taking each placement the model is at least [`Params::threshold`] sure
//!    of;
//! 2. aligns the whole line once too, and moves each heard word to the aligner's start for it
//!    when the two agree within [`Params::window`] (they then both point at the same sung
//!    word, and the aligner's 20 ms frames are finer than the engine's word times).
//!
//! On the 27 credible ground-truth songs this takes words within 0.1 s from 56% to 67% and the
//! 90th-percentile error from 0.40 s to 0.34 s (`plans/forced-alignment-experiment.md`).

use crate::ctc;
use crate::words::{Line, LineWords};

/// The model's view of one line: an emission over `[t0, t0 + frames × ratio)`.
#[derive(Debug, Clone)]
pub struct LineEmission {
    /// Seconds from the start of the track of frame 0.
    pub t0: f64,
    /// Seconds per frame (about 0.02).
    pub ratio: f64,
    /// `frames × ctc::CLASSES` log-probabilities, star column included.
    pub data: Vec<f32>,
}

impl LineEmission {
    pub fn frames(&self) -> usize {
        self.data.len() / ctc::CLASSES
    }

    fn frame(&self, t: f64) -> usize {
        (((t - self.t0) / self.ratio).round().max(0.0) as usize).min(self.frames())
    }

    fn time(&self, frame: usize) -> f64 {
        self.t0 + frame as f64 * self.ratio
    }
}

/// How much audio around a line's span its emission should cover, seconds: a little before
/// (a tapped start lands late), more after (the last word rings past the next line's start).
pub const PAD_BEFORE: f64 = 0.15;
pub const PAD_AFTER: f64 = 0.35;

/// The span of audio to compute a line's emission over.
pub fn window(line: &Line, duration: f64) -> (f64, f64) {
    let t0 = (line.start - PAD_BEFORE).max(0.0);
    let t1 = (line.end + PAD_AFTER).min(if duration > 0.0 { duration } else { f64::MAX });
    (t0, t1.max(t0))
}

#[derive(Debug, Clone)]
pub struct Params {
    /// The least mean probability an unheard word's placement needs to be taken.
    pub threshold: f32,
    /// Seconds within which a heard word moves to the aligner's start for it.
    pub window: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            threshold: 0.05,
            window: 0.25,
        }
    }
}

const MIN_GAP: f64 = 0.03;

/// `placed` (from [`crate::words::time_words`] on the same `lines`) refined by `emissions`, one
/// per line (`None` where there is none). Lines without an emission come back unchanged.
pub fn refine(
    lines: &[Line],
    placed: &[LineWords],
    emissions: &[Option<LineEmission>],
    p: &Params,
) -> Vec<LineWords> {
    lines
        .iter()
        .zip(placed)
        .zip(emissions.iter().chain(std::iter::repeat(&None)))
        .map(|((line, lw), em)| match em {
            Some(em) => refine_line(line, lw, em, p),
            None => lw.clone(),
        })
        .collect()
}

fn refine_line(line: &Line, lw: &LineWords, em: &LineEmission, p: &Params) -> LineWords {
    let words: Vec<&str> = line.text.split_whitespace().collect();
    let n = words.len();
    if n != lw.starts.len() || n != lw.heard.len() || em.frames() == 0 {
        return lw.clone();
    }
    let tokens: Vec<Vec<usize>> = words
        .iter()
        .map(|w| ctc::tokens(&ctc::romanize(w)))
        .collect();
    let mut starts = lw.starts.clone();

    // 1. Unheard words, gap by gap between pins.
    let mut bounds: Vec<usize> = std::iter::once(0)
        .chain((1..n).filter(|&k| lw.heard[k]))
        .collect();
    bounds.push(n);
    for pair in bounds.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b - a < 2 {
            continue; // nothing unheard between these pins
        }
        let seg: Vec<usize> = (a..b).filter(|&k| !tokens[k].is_empty()).collect();
        // The pin itself must be alignable: it is what holds the gap to the audio.
        if seg.first() != Some(&a) {
            continue;
        }
        let f0 = em.frame(lw.starts[a]);
        let f1 = if b < n {
            em.frame(lw.starts[b])
        } else {
            em.frames()
        };
        let chars: usize = seg.iter().map(|&k| tokens[k].len()).sum();
        if f1 <= f0 || f1 - f0 < 2 * chars {
            continue;
        }
        let gap = &em.data[f0 * ctc::CLASSES..f1 * ctc::CLASSES];
        let toks: Vec<Vec<usize>> = seg.iter().map(|&k| tokens[k].clone()).collect();
        let Some(spans) = ctc::align(gap, &toks, false, b == n) else {
            continue;
        };
        for (&k, s) in seg.iter().zip(&spans).skip(1) {
            if s.score >= p.threshold {
                starts[k] = em.time(f0 + s.start);
            }
        }
    }
    for k in 1..n {
        starts[k] = starts[k].max(starts[k - 1] + MIN_GAP);
    }

    // 2. Heard words, toward the whole line's alignment where the two agree.
    let keep: Vec<usize> = (0..n).filter(|&k| !tokens[k].is_empty()).collect();
    let toks: Vec<Vec<usize>> = keep.iter().map(|&k| tokens[k].clone()).collect();
    if let Some(spans) = ctc::align(&em.data, &toks, true, true) {
        for (&k, s) in keep.iter().zip(&spans) {
            if k > 0 && lw.heard[k] {
                let t = em.time(s.start);
                if (t - lw.starts[k]).abs() < p.window {
                    starts[k] = t;
                }
            }
        }
        for k in 1..n {
            starts[k] = starts[k].max(starts[k - 1] + MIN_GAP);
        }
    }

    LineWords {
        starts,
        matched: lw.matched,
        heard: lw.heard.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An emission that is sure of `path` (one class per frame), unsure of everything else.
    fn emission(path: &[usize], t0: f64) -> LineEmission {
        let mut data = vec![(1e-4f32).ln(); path.len() * ctc::CLASSES];
        for (f, &c) in path.iter().enumerate() {
            data[f * ctc::CLASSES + c] = 0.0;
            data[f * ctc::CLASSES + ctc::STAR] = 0.0;
        }
        LineEmission {
            t0,
            ratio: 0.02,
            data,
        }
    }

    /// A path singing each `(word, frames)` from its first frame, three frames a letter (a blank
    /// between doubled letters), then blanks to fill its frames; `""` is silence.
    fn frames_of(spec: &[(&str, usize)]) -> Vec<usize> {
        let mut path = Vec::new();
        for (text, frames) in spec {
            let start = path.len();
            let toks = ctc::tokens(text);
            for (i, &t) in toks.iter().enumerate() {
                if i > 0 && toks[i - 1] == t {
                    path.push(ctc::BLANK);
                }
                path.extend([t; 3]);
            }
            while path.len() - start < *frames {
                path.push(ctc::BLANK);
            }
        }
        path
    }

    #[test]
    fn an_unheard_word_moves_to_where_it_is_sung() {
        // "one two three": "one" at 0 s, "two" sung at 0.6 s, "three" at 1.0 s and heard there.
        let path = frames_of(&[("one", 15), ("", 15), ("two", 20), ("three", 20)]);
        let em = emission(&path, 0.0);
        let line = Line {
            text: "one two three",
            start: 0.0,
            end: 1.4,
        };
        let placed = LineWords {
            starts: vec![0.0, 0.5, 1.0],
            matched: 1,
            heard: vec![false, false, true],
        };
        let out = refine(&[line], &[placed], &[Some(em)], &Params::default());
        assert!((out[0].starts[1] - 0.6).abs() < 0.03, "{:?}", out[0].starts);
        assert!((out[0].starts[2] - 1.0).abs() < 0.03);
    }

    #[test]
    fn no_emission_changes_nothing() {
        let line = Line {
            text: "a b",
            start: 0.0,
            end: 1.0,
        };
        let placed = LineWords {
            starts: vec![0.0, 0.5],
            matched: 0,
            heard: vec![false, false],
        };
        let out = refine(
            &[line],
            std::slice::from_ref(&placed),
            &[],
            &Params::default(),
        );
        assert_eq!(out[0], placed);
    }

    #[test]
    fn windows_pad_the_line_and_stay_inside_the_track() {
        let line = Line {
            text: "x",
            start: 0.1,
            end: 3.0,
        };
        assert_eq!(window(&line, 3.2), (0.0, 3.2));
        let (a, b) = window(&line, 0.0);
        assert_eq!(a, 0.0);
        assert!((b - 3.35).abs() < 1e-9);
    }
}
