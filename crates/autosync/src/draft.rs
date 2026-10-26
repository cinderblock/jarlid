//! Lyrics from nothing: the words the speech model heard, broken into timed lines for a person
//! to review.
//!
//! When LRCLIB has no lyrics at all there is nothing to align, so the heard words *are* the
//! lyrics — a draft. On sung vocals the model gets some words wrong, which is why a draft is only
//! ever shown as one and is reviewed before it goes anywhere (`plans/draft-lyrics.md`). Two things
//! make that review fast, and both come from here: lines that start where the singer starts them,
//! so timings mostly need checking rather than making, and the words the model was unsure of,
//! marked, which is where its mistakes are.
//!
//! A line ends where the singer pauses. Sung phrases are separated by breaths, and Parakeet's
//! sentence punctuation agrees with them often enough to tip a short pause into a break. A line
//! that runs on without a pause is split at its longest internal gap. A long silence becomes a
//! blank line, which the pane shows as an instrumental break.

use crate::Heard;

/// Bumped when drafting changes, so stored drafts are redone from the words kept with them.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    /// A pause at least this long (seconds) always ends a line.
    pub pause: f64,
    /// After sentence punctuation, a pause this long is enough.
    pub punct_pause: f64,
    /// A line with more words than this is split at its longest gap.
    pub max_words: usize,
    /// …and so is one lasting longer than this (seconds).
    pub max_seconds: f64,
    /// Neither half of a forced split may be shorter than this many words.
    pub min_words: usize,
    /// A silence this long between lines gets a blank line, to clear the display.
    pub gap_line: f64,
    /// Words heard with less confidence than this are marked for the reviewer.
    pub doubt: f32,
    /// Seconds each line starts ahead of its first word. Lines light up a beat before they are
    /// sung, and Parakeet's word starts run late (`align::Params::lead`).
    pub lead: f64,
    /// Fewer words than this in the whole song is not singing worth a draft: an instrumental,
    /// or a spoken sample the model caught.
    pub min_song_words: usize,
    /// Below this mean confidence over the whole song, the model could not make out the words —
    /// too buried in the mix, or a language it does not know — and a draft would be mostly wrong.
    pub min_confidence: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            // Tuned on the 56-track corpus (`examples/draft.rs`; `plans/draft-lyrics.md`).
            pause: 0.9,
            punct_pause: 0.15,
            max_words: 12,
            max_seconds: 4.0,
            min_words: 3,
            gap_line: 6.0,
            doubt: 0.8,
            lead: 0.25,
            min_song_words: 25,
            min_confidence: 0.75,
        }
    }
}

/// One word of a draft line.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub p: f32,
}

/// One line of a draft. No words means a blank line: an instrumental break.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub t: f64,
    pub words: Vec<Word>,
}

impl Line {
    /// The line as it should read: first letter capitalised, and no trailing comma or full stop,
    /// the way lyric sites write lines. Questions and exclamations keep their marks.
    pub fn text(&self) -> String {
        let joined = self
            .words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let trimmed = joined.trim_end_matches(['.', ',', ';', ':']);
        let mut chars = trimmed.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => String::new(),
        }
    }

    /// Which of the line's words were heard with low confidence, by position.
    pub fn doubtful(&self, params: &Params) -> Vec<usize> {
        self.words
            .iter()
            .enumerate()
            .filter(|(_, w)| w.p < params.doubt)
            .map(|(i, _)| i)
            .collect()
    }
}

/// Why there is no draft.
#[derive(Debug, Clone, PartialEq)]
pub enum NoDraft {
    /// Too few words to be singing: an instrumental, or a sample the model caught.
    NoSinging { words: usize },
    /// Singing, but the model was unsure of too much of it for a draft to be worth reviewing.
    Unclear { confidence: f32 },
}

/// The draft for everything heard.
pub fn draft(heard: &[Heard]) -> Result<Vec<Line>, NoDraft> {
    draft_with(heard, &Params::default())
}

pub fn draft_with(heard: &[Heard], params: &Params) -> Result<Vec<Line>, NoDraft> {
    let mut words: Vec<Word> = heard
        .iter()
        .filter(|h| !h.text.trim().is_empty())
        .map(|h| Word {
            text: h.text.trim().to_string(),
            start: h.start,
            end: h.end.max(h.start),
            p: h.p,
        })
        .collect();
    words.sort_by(|a, b| a.start.total_cmp(&b.start));
    if words.len() < params.min_song_words {
        return Err(NoDraft::NoSinging { words: words.len() });
    }
    let confidence = words.iter().map(|w| w.p).sum::<f32>() / words.len() as f32;
    if confidence < params.min_confidence {
        return Err(NoDraft::Unclear { confidence });
    }

    // Phrases: runs of words with no pause the singer would breathe in.
    let mut phrases: Vec<Vec<Word>> = Vec::new();
    for w in words {
        let breaks = phrases.last().and_then(|p| p.last()).is_none_or(|prev| {
            let gap = w.start - prev.end;
            gap >= params.pause || (ends_sentence(&prev.text) && gap >= params.punct_pause)
        });
        if breaks {
            phrases.push(vec![w]);
        } else {
            phrases.last_mut().unwrap().push(w);
        }
    }

    let mut lines: Vec<Line> = Vec::new();
    for phrase in phrases {
        for words in split_long(phrase, params) {
            let floor = lines.last().map_or(0.0, |l| l.t);
            let t = (words[0].start - params.lead).max(floor);
            if let Some(prev_end) = lines.last().and_then(|l| l.words.last()).map(|w| w.end) {
                if t - prev_end >= params.gap_line {
                    // Clear the display a moment after the last word, not the instant it ends.
                    lines.push(Line {
                        t: prev_end + 1.0,
                        words: Vec::new(),
                    });
                }
            }
            lines.push(Line { t, words });
        }
    }
    Ok(lines)
}

/// Whether a word ends a sentence or clause, as the model punctuated it.
fn ends_sentence(word: &str) -> bool {
    word.ends_with(['.', '?', '!', ',', ';', ':'])
}

/// Split a phrase that is too long to read as one line, each time at its longest internal gap,
/// keeping both sides at least `min_words` long.
fn split_long(phrase: Vec<Word>, params: &Params) -> Vec<Vec<Word>> {
    let too_long = |p: &[Word]| {
        p.len() > params.max_words
            || (p.len() >= 2 * params.min_words
                && p.last().unwrap().end - p[0].start > params.max_seconds)
    };
    if !too_long(&phrase) || phrase.len() < 2 * params.min_words {
        return vec![phrase];
    }
    // Prefer a cut after punctuation; among those (or all, if none), the longest gap.
    let cut = (params.min_words..=phrase.len() - params.min_words)
        .max_by(|&a, &b| {
            let score = |i: usize| {
                let gap = phrase[i].start - phrase[i - 1].end;
                gap + if ends_sentence(&phrase[i - 1].text) {
                    1.0
                } else {
                    0.0
                }
            };
            score(a).total_cmp(&score(b))
        })
        .unwrap();
    let mut rest = phrase;
    let tail = rest.split_off(cut);
    let mut out = split_long(rest, params);
    out.extend(split_long(tail, params));
    out
}

/// The draft as LRC lines, for the lyrics pane.
pub fn to_lrc(lines: &[Line]) -> Vec<crate::lrc::Line> {
    lines
        .iter()
        .map(|l| crate::lrc::Line {
            t: l.t,
            text: l.text(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words spoken back to back from `start`, `dur` seconds each.
    fn run(text: &str, start: f64, dur: f64) -> Vec<Heard> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, w)| Heard {
                text: w.into(),
                start: start + i as f64 * dur,
                end: start + (i + 1) as f64 * dur,
                p: 0.95,
            })
            .collect()
    }

    fn params() -> Params {
        // No lead, so tests can assert exact times.
        Params {
            min_song_words: 1,
            lead: 0.0,
            ..Params::default()
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn a_pause_ends_a_line() {
        let mut heard = run("I took a walk", 10.0, 0.3);
        heard.extend(run("around the world", 13.0, 0.3));
        let d = draft_with(&heard, &params()).unwrap();
        assert_eq!(texts(&d), ["I took a walk", "Around the world"]);
        assert_eq!(d[1].t, 13.0);
    }

    #[test]
    fn punctuation_tips_a_short_pause_into_a_break() {
        let mut heard = run("to ease my troubled mind.", 0.0, 0.3);
        heard.extend(run("I left my body", 1.5 + 0.4, 0.3));
        let d = draft_with(&heard, &params()).unwrap();
        assert_eq!(texts(&d), ["To ease my troubled mind", "I left my body"]);

        // The same pause without the full stop is just a breath inside the line.
        let mut heard = run("to ease my troubled mind", 0.0, 0.3);
        heard.extend(run("I left my body", 1.5 + 0.4, 0.3));
        assert_eq!(draft_with(&heard, &params()).unwrap().len(), 1);
    }

    #[test]
    fn a_run_on_line_splits_at_its_longest_gap() {
        let mut heard = run("one two three four five six seven", 0.0, 0.3);
        heard.extend(run("eight nine ten eleven twelve thirteen", 2.1 + 0.5, 0.3));
        let d = draft_with(&heard, &params()).unwrap();
        assert_eq!(
            texts(&d),
            [
                "One two three four five six seven",
                "Eight nine ten eleven twelve thirteen"
            ]
        );
    }

    #[test]
    fn lines_start_a_little_ahead_of_their_first_word() {
        let mut heard = run("first line here", 0.1, 0.3);
        heard.extend(run("second line here", 5.0, 0.3));
        let p = Params {
            lead: 0.25,
            ..params()
        };
        let d = draft_with(&heard, &p).unwrap();
        // Never before the start of the track, and never before the line above.
        assert_eq!(d[0].t, 0.0);
        assert!((d[1].t - 4.75).abs() < 1e-9);
    }

    #[test]
    fn a_long_silence_gets_a_blank_line() {
        let mut heard = run("first verse ends here", 0.0, 0.3);
        heard.extend(run("second verse starts", 30.0, 0.3));
        let d = draft_with(&heard, &params()).unwrap();
        assert_eq!(
            texts(&d),
            ["First verse ends here", "", "Second verse starts"]
        );
        assert!((d[1].t - 2.2).abs() < 1e-9);
    }

    #[test]
    fn too_few_words_is_not_a_song() {
        let heard = run("yeah", 40.0, 0.3);
        assert_eq!(draft(&heard), Err(NoDraft::NoSinging { words: 1 }));
    }

    #[test]
    fn words_the_model_could_not_make_out_are_not_a_draft() {
        let mut heard = run(&"la ".repeat(40), 0.0, 0.3);
        for w in &mut heard {
            w.p = 0.5;
        }
        assert!(matches!(draft(&heard), Err(NoDraft::Unclear { .. })));
    }

    #[test]
    fn doubtful_words_are_marked() {
        let mut heard = run("float to the dark side", 0.0, 0.3);
        heard[0].p = 0.48;
        heard[3].p = 0.44;
        let d = draft_with(&heard, &params()).unwrap();
        assert_eq!(d[0].doubtful(&params()), [0, 3]);
    }

    #[test]
    fn lines_read_like_lyrics() {
        let d = draft_with(
            &run("will you still call me Superman?", 0.0, 0.3),
            &params(),
        )
        .unwrap();
        assert_eq!(d[0].text(), "Will you still call me Superman?");
        let d = draft_with(&run("and the sands of time,", 0.0, 0.3), &params()).unwrap();
        assert_eq!(d[0].text(), "And the sands of time");
    }
}
