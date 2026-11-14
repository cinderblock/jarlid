//! CTC forced alignment: where in a stretch of audio each known word is sung.
//!
//! The acoustic model (Meta's MMS forced-alignment model, run in the sidecar) turns audio into an
//! *emission*: for every 20 ms frame, the log-probability of each character of a small Latin
//! alphabet, plus a CTC blank. Given the words that are sung there, [`align`] finds the most
//! probable path through them (Viterbi), and with it the frame each character, and so each word,
//! starts on. This is `torchaudio.functional.forced_align` plus the `MMS_FA` bundle's tokenizer
//! and aligner, rewritten so the app needs no Python and the sidecar only runs the network.
//!
//! A *star* token stands for anything that isn't the words: put one before or after them and
//! audio there (an instrumental, a backing vocal, the line before running over) is absorbed by it
//! instead of being forced onto a word. Its column of the emission is all zeros (probability 1),
//! as torchaudio's `with_star` model has it.

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// The model's output classes, in order: the CTC blank, 27 characters, then the star (added by
/// [`with_star`]). Mirrors `torchaudio.pipelines.MMS_FA.get_labels()`.
pub const LABELS: [char; 29] = [
    '-', 'a', 'i', 'e', 'n', 'o', 'u', 't', 's', 'r', 'm', 'k', 'l', 'd', 'g', 'h', 'y', 'b', 'p',
    'w', 'c', 'v', 'j', 'z', 'f', '\'', 'q', 'x', '*',
];
/// Classes the network itself outputs (no star).
pub const MODEL_CLASSES: usize = 28;
/// Classes in an emission, star included.
pub const CLASSES: usize = 29;
pub const BLANK: usize = 0;
pub const STAR: usize = 28;

/// A word in the model's alphabet: case folded, accents stripped (NFKD, marks dropped), the
/// typographic apostrophe made plain, and anything else outside the alphabet dropped — digits,
/// punctuation, and `-`, which is the blank and would make the word unalignable. May be empty
/// (a word of digits or symbols); such a word can't be aligned.
pub fn romanize(word: &str) -> String {
    word.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c == '’' { '\'' } else { c })
        .filter(|c| *c != '-' && *c != '*' && LABELS.contains(c))
        .collect()
}

/// A romanized word as class indices.
pub fn tokens(romanized: &str) -> Vec<usize> {
    romanized
        .chars()
        .filter_map(|c| LABELS.iter().position(|l| *l == c))
        .collect()
}

/// The network's raw output, `frames × MODEL_CLASSES` logits, as an emission: log-softmax over
/// each frame, then a star column of zeros. Row-major, `frames × CLASSES`.
pub fn with_star(logits: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(logits.len() / MODEL_CLASSES * CLASSES);
    for row in logits.as_chunks::<MODEL_CLASSES>().0 {
        let max = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let sum: f32 = row.iter().map(|x| (x - max).exp()).sum();
        let log_sum = max + sum.ln();
        out.extend(row.iter().map(|x| x - log_sum));
        out.push(0.0);
    }
    out
}

/// One token's run of frames on the best path: `[start, end)` and its mean probability.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub token: usize,
    pub start: usize,
    pub end: usize,
    pub score: f32,
}

/// The best path for `targets` through `emission` (`frames × CLASSES`, log-probabilities): the
/// class of every frame. `None` when there are too few frames to fit them (each token needs a
/// frame, and a repeated character a blank between).
///
/// Ties go the way torchaudio's CPU kernel breaks them (stay, then advance one, then skip), so
/// paths match it frame for frame.
pub fn viterbi(emission: &[f32], targets: &[usize]) -> Option<Vec<usize>> {
    let t_len = emission.len() / CLASSES;
    let l = targets.len();
    if l == 0 || t_len == 0 {
        return None;
    }
    let repeats = targets.windows(2).filter(|w| w[0] == w[1]).count();
    if t_len < l + repeats {
        return None;
    }
    let s_len = 2 * l + 1;
    let label = |s: usize| {
        if s.is_multiple_of(2) {
            BLANK
        } else {
            targets[s / 2]
        }
    };
    let at = |t: usize, c: usize| emission[t * CLASSES + c];
    let neg = f32::NEG_INFINITY;

    let mut alpha = vec![neg; s_len];
    let mut back = vec![0u8; t_len * s_len];
    alpha[0] = at(0, BLANK);
    alpha[1] = at(0, label(1));
    let mut next = vec![neg; s_len];
    for t in 1..t_len {
        for s in 0..s_len {
            let x0 = alpha[s];
            let x1 = if s >= 1 { alpha[s - 1] } else { neg };
            let x2 = if s >= 2 && !s.is_multiple_of(2) && label(s) != label(s - 2) {
                alpha[s - 2]
            } else {
                neg
            };
            let (best, from) = if x2 > x1 && x2 > x0 {
                (x2, 2)
            } else if x1 > x0 && x1 > x2 {
                (x1, 1)
            } else {
                (x0, 0)
            };
            next[s] = if best == neg {
                neg
            } else {
                best + at(t, label(s))
            };
            back[t * s_len + s] = from;
        }
        std::mem::swap(&mut alpha, &mut next);
    }
    let mut s = if alpha[s_len - 1] > alpha[s_len - 2] {
        s_len - 1
    } else {
        s_len - 2
    };
    if alpha[s] == neg {
        return None;
    }
    let mut path = vec![0; t_len];
    for t in (0..t_len).rev() {
        path[t] = label(s);
        s -= back[t * s_len + s] as usize;
    }
    Some(path)
}

/// The path's runs of non-blank tokens, each with its mean probability over its frames.
pub fn spans(emission: &[f32], path: &[usize]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut start = 0;
    for t in 1..=path.len() {
        if t == path.len() || path[t] != path[start] {
            let token = path[start];
            if token != BLANK {
                let score = (start..t)
                    .map(|f| emission[f * CLASSES + token].exp())
                    .sum::<f32>()
                    / (t - start) as f32;
                out.push(Span {
                    token,
                    start,
                    end: t,
                    score,
                });
            }
            start = t;
        }
    }
    out
}

/// Where each word starts, and how sure the model is of it.
#[derive(Debug, Clone, PartialEq)]
pub struct WordSpan {
    /// The frame its first character starts on.
    pub start: usize,
    /// The frame after its last character.
    pub end: usize,
    /// Mean probability over all its characters' frames, 0–1.
    pub score: f32,
}

/// Align `words` (each already [`tokens`], none empty) on `emission`, with a star before and/or
/// after them. Returns one span per word, star excluded; `None` if they don't fit.
pub fn align(
    emission: &[f32],
    words: &[Vec<usize>],
    star_before: bool,
    star_after: bool,
) -> Option<Vec<WordSpan>> {
    if words.iter().any(Vec::is_empty) {
        return None;
    }
    let mut targets = Vec::new();
    if star_before {
        targets.push(STAR);
    }
    for w in words {
        targets.extend_from_slice(w);
    }
    if star_after {
        targets.push(STAR);
    }
    let path = viterbi(emission, &targets)?;
    let mut spans = spans(emission, &path).into_iter();
    if star_before {
        spans.next()?;
    }
    let mut out = Vec::with_capacity(words.len());
    for w in words {
        let chars: Vec<Span> = spans.by_ref().take(w.len()).collect();
        if chars.len() != w.len() {
            return None;
        }
        let frames: usize = chars.iter().map(|s| s.end - s.start).sum();
        let weighted: f32 = chars
            .iter()
            .map(|s| s.score * (s.end - s.start) as f32)
            .sum();
        out.push(WordSpan {
            start: chars[0].start,
            end: chars[chars.len() - 1].end,
            score: weighted / frames as f32,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/data/ctc-wings.json")).unwrap()
    }

    #[test]
    fn romanizes_like_the_experiment() {
        for (word, want) in fixture()["romanize"].as_object().unwrap() {
            assert_eq!(romanize(word), want.as_str().unwrap(), "{word}");
        }
    }

    /// The same emission and words through torchaudio's `MMS_FA` aligner (saved by the
    /// experiment): every character's frames and score must come out the same.
    #[test]
    fn matches_torchaudio() {
        let f = fixture();
        let emission: Vec<f32> = f["emission"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        assert_eq!(
            emission.len(),
            f["frames"].as_u64().unwrap() as usize * CLASSES
        );
        for case in f["cases"].as_array().unwrap() {
            let words: Vec<&str> = case["words"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w.as_str().unwrap())
                .collect();
            let targets: Vec<usize> = words.iter().flat_map(|w| tokens(w)).collect();
            let path = viterbi(&emission, &targets).expect("fits");
            let got = spans(&emission, &path);
            let want: Vec<&Value> = case["spans"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|w| w.as_array().unwrap())
                .collect();
            assert_eq!(got.len(), want.len(), "{words:?}");
            for (g, w) in got.iter().zip(want) {
                let w = w.as_array().unwrap();
                assert_eq!(g.token, w[0].as_u64().unwrap() as usize);
                assert_eq!(g.start, w[1].as_u64().unwrap() as usize);
                assert_eq!(g.end, w[2].as_u64().unwrap() as usize);
                assert!((g.score - w[3].as_f64().unwrap() as f32).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn word_spans_start_on_their_first_character() {
        let f = fixture();
        let emission: Vec<f32> = f["emission"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        let case = &f["cases"][0];
        let words: Vec<Vec<usize>> = case["words"].as_array().unwrap()[1..]
            .iter()
            .map(|w| tokens(w.as_str().unwrap()))
            .filter(|t| !t.is_empty() && t != &[STAR])
            .collect();
        let got = align(&emission, &words, true, true).unwrap();
        let want = case["spans"].as_array().unwrap();
        for (g, w) in got.iter().zip(&want[1..]) {
            assert_eq!(g.start as u64, w[0][1].as_u64().unwrap());
        }
    }

    #[test]
    fn too_short_to_fit_is_none() {
        let e = vec![0.0; 2 * CLASSES];
        assert_eq!(viterbi(&e, &tokens("abc")), None);
        // A doubled letter needs a blank between its two runs.
        let e = vec![0.0; 2 * CLASSES];
        assert_eq!(viterbi(&e, &tokens("ll")), None);
    }

    #[test]
    fn log_softmax_rows_sum_to_one_and_star_is_certain() {
        let logits: Vec<f32> = (0..2 * MODEL_CLASSES).map(|i| (i % 7) as f32).collect();
        let e = with_star(&logits);
        assert_eq!(e.len(), 2 * CLASSES);
        for row in e.as_chunks::<CLASSES>().0 {
            let p: f32 = row[..MODEL_CLASSES].iter().map(|x| x.exp()).sum();
            assert!((p - 1.0).abs() < 1e-5);
            assert_eq!(row[STAR], 0.0);
        }
    }
}
