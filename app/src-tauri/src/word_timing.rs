//! Word starts for lyrics that time only their lines (`plans/word-timings-for-line-timed-lyrics.md`).
//!
//! An LRC times lines. Word-by-word highlighting needs `Lyrics::timing` to carry, for each LRC
//! line (same start, same text), its words' starts. Where those are missing they are filled in
//! here from timed words that cover the song another way:
//!
//! - the record's own Lyricsfile, when its lines don't match its LRC's (common on LRCLIB: the
//!   two are often different transcriptions, lines joined or split, ad-libs added), or
//! - what the speech engine heard of the track, when auto-sync has heard it.
//!
//! The LRC's line starts are never moved: whoever set them, LRCLIB's uploader or the listener,
//! is trusted. Only the words inside each line are placed.
//!
//! **Sharper word timing.** With the optional forced aligner installed, auto-sync also refines
//! this placement against the audio (`autosync::refine`) and keeps the result per line
//! ([`RefinedLine`]). [`fill`] uses a line's refined starts while its text and start are still
//! the ones they were made for, and this module's own placement otherwise.

use autosync::words::{time_words, Line, LineWords, Params};
use autosync::Heard;
use serde::{Deserialize, Serialize};

use crate::lyricsfile::{TimedLine, TimedWord};

/// Below this share of their words (first words aside) matched, the engine's word times for a
/// song are guesses spread over lines, and none are shown.
const ENGINE_GATE: f64 = 0.35;

/// How long a song's last line is taken to last, when nothing follows it.
const LAST_LINE_SECONDS: f64 = 6.0;

/// One line's word starts as the forced aligner refined them, for the line with this text and
/// start (as the LRC had it when they were made).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RefinedLine {
    pub text: String,
    pub start: f64,
    pub starts: Vec<f64>,
}

impl RefinedLine {
    fn fits(&self, t: f64, text: &str) -> bool {
        (self.start - t).abs() < 0.011
            && self.text.trim() == text.trim()
            && self.starts.len() == text.split_whitespace().count()
    }
}

/// What [`fill`] places, before it is written into the lyrics: every LRC line as a span, the
/// word starts placed for each, and which lines it fills (lines that already have their words
/// keep them).
pub struct Placement {
    pub lines: Vec<(String, f64, f64)>,
    pub placed: Vec<LineWords>,
    pub fills: Vec<bool>,
}

impl Placement {
    /// The lines as `autosync` takes them.
    pub fn spans(&self) -> Vec<Line<'_>> {
        self.lines
            .iter()
            .map(|(text, start, end)| Line {
                text,
                start: *start,
                end: *end,
            })
            .collect()
    }

    /// Whether `refined` has every line this fills with more than one word.
    pub fn refined_by(&self, refined: &[RefinedLine]) -> bool {
        self.lines
            .iter()
            .zip(&self.fills)
            .filter(|((text, _, _), fills)| **fills && needs_words(text))
            .all(|((text, start, _), _)| refined.iter().any(|r| r.fits(*start, text)))
    }
}

/// Give `lyrics.timing` word starts for every LRC line that has none, from the record's own
/// Lyricsfile words and `heard` (the engine's), or from `refined` for the lines it has. Leaves
/// it alone when there's no LRC, when every line already has its words, or when there's nothing
/// to place them by.
pub fn fill(
    lyrics: &mut crate::lyrics::Lyrics,
    heard: impl FnOnce() -> Option<Vec<Heard>>,
    refined: impl FnOnce() -> Option<Vec<RefinedLine>>,
) {
    let Some(p) = placement(lyrics, heard) else {
        return;
    };
    let refined = refined().unwrap_or_default();
    let existing = lyrics.timing.as_deref().unwrap_or_default();
    let lrc = autosync::lrc::parse(lyrics.synced.as_deref().unwrap_or_default());
    let mut out: Vec<TimedLine> = Vec::with_capacity(lrc.len());
    for ((l, fills), t) in lrc.iter().zip(&p.fills).zip(&p.placed) {
        if !fills {
            if let Some(h) = exact(existing, l.t, &l.text) {
                out.push(h.clone());
            }
            continue;
        }
        let starts = refined
            .iter()
            .find(|r| r.fits(l.t, &l.text))
            .map(|r| &r.starts)
            .unwrap_or(&t.starts);
        let words: Vec<TimedWord> = l
            .text
            .split_whitespace()
            .zip(starts)
            .map(|(w, &start)| TimedWord {
                text: w.to_string(),
                start,
                end: None,
            })
            .collect();
        out.push(TimedLine {
            text: l.text.clone(),
            start: l.t,
            end: None,
            words: if needs_words(&l.text) {
                words
            } else {
                Vec::new()
            },
        });
    }
    // Pauses the old timing marked (empty lines) keep their places between the lines.
    for p in existing.iter().filter(|l| l.text.trim().is_empty()) {
        if !out.iter().any(|o| (o.start - p.start).abs() < 0.011) {
            out.push(p.clone());
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    lyrics.timing = Some(out);
}

/// The placement [`fill`] makes, or `None` when it would leave the lyrics alone.
pub fn placement(
    lyrics: &crate::lyrics::Lyrics,
    heard: impl FnOnce() -> Option<Vec<Heard>>,
) -> Option<Placement> {
    if lyrics.draft.is_some() {
        return None;
    }
    let lrc = autosync::lrc::parse(lyrics.synced.as_deref()?);
    let existing = lyrics.timing.as_deref().unwrap_or_default();
    let have: Vec<bool> = lrc
        .iter()
        .map(|l| exact(existing, l.t, &l.text).is_some())
        .collect();
    let wants = lrc
        .iter()
        .zip(&have)
        .any(|(l, h)| needs_words(&l.text) && !h);
    if !wants {
        return None;
    }

    // The record's own words and the engine's, together: a Lyricsfile can time only some lines.
    // Both are taken as they are (`Params::lead` is 0: the engine's word starts land on sung
    // words' starts, measured on the ground-truth songs in the plan).
    let mut evidence: Vec<Heard> = lyricsfile_words(existing);
    let gate = if evidence.is_empty() {
        ENGINE_GATE
    } else {
        0.0
    };
    evidence.extend(heard().unwrap_or_default());
    if evidence.is_empty() {
        return None;
    }
    evidence.sort_by(|a, b| a.start.total_cmp(&b.start));
    let params = Params::default();

    let lines = lines_of(&lrc, lyrics.duration);
    let timed = time_words(&lines, &evidence, &params);

    let placeable: usize = lines
        .iter()
        .zip(&have)
        .filter(|(_, h)| !**h)
        .map(|(l, _)| l.text.split_whitespace().count().saturating_sub(1))
        .sum();
    let matched: usize = timed
        .iter()
        .zip(&have)
        .filter(|(_, h)| !**h)
        .map(|(t, _)| t.matched)
        .sum();
    if placeable > 0 && (matched as f64) < gate * placeable as f64 {
        return None;
    }
    Some(Placement {
        lines: lines
            .iter()
            .map(|l| (l.text.to_string(), l.start, l.end))
            .collect(),
        placed: timed,
        fills: have.iter().map(|h| !h).collect(),
    })
}

/// Whether hearing the song would give `lyrics` word times it lacks: it has timed lines without
/// their words that its own Lyricsfile words can't place either.
pub fn wants_hearing(lyrics: &crate::lyrics::Lyrics) -> bool {
    if lyrics.draft.is_some() {
        return false;
    }
    let Some(synced) = lyrics.synced.as_deref() else {
        return false;
    };
    let existing = lyrics.timing.as_deref().unwrap_or_default();
    let lrc = autosync::lrc::parse(synced);
    let own = lyricsfile_words(existing);
    let timed = time_words(&lines_of(&lrc, lyrics.duration), &own, &Params::default());
    lrc.iter().zip(&timed).any(|(l, t)| {
        needs_words(&l.text) && exact(existing, l.t, &l.text).is_none() && t.matched == 0
    })
}

/// The LRC's lines as spans: each lasts until the next starts.
fn lines_of(lrc: &[autosync::lrc::Line], duration: Option<f64>) -> Vec<Line<'_>> {
    lrc.iter()
        .enumerate()
        .map(|(i, l)| Line {
            text: &l.text,
            start: l.t,
            end: lrc
                .get(i + 1)
                .map(|n| n.t)
                .or(duration.filter(|&d| d > l.t))
                .unwrap_or(l.t + LAST_LINE_SECONDS)
                .min(l.t + 30.0),
        })
        .collect()
}

/// A line worth timing word by word: one with words, more than one of them.
fn needs_words(text: &str) -> bool {
    text.split_whitespace().nth(1).is_some()
}

/// The timing line for an LRC line, when there is one with its start and text and a start for
/// every word. LRC keeps hundredths; timing keeps milliseconds.
fn exact<'a>(timing: &'a [TimedLine], t: f64, text: &str) -> Option<&'a TimedLine> {
    let n = text.split_whitespace().count();
    timing.iter().find(|l| {
        (l.start - t).abs() < 0.011 && l.text.trim() == text.trim() && l.words.len() == n && n > 0
    })
}

/// A Lyricsfile's word times as timed words, each lasting until the next (or its line's end).
fn lyricsfile_words(timing: &[TimedLine]) -> Vec<Heard> {
    let mut out = Vec::new();
    for l in timing {
        for (k, w) in l.words.iter().enumerate() {
            let end = w
                .end
                .or_else(|| l.words.get(k + 1).map(|n| n.start))
                .or(l.end)
                .unwrap_or(w.start + 0.4);
            out.push(Heard {
                text: w.text.clone(),
                start: w.start,
                end,
                p: 1.0,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::Lyrics;

    fn word(text: &str, start: f64) -> TimedWord {
        TimedWord {
            text: text.into(),
            start,
            end: None,
        }
    }

    fn lyrics(synced: &str, timing: Option<Vec<TimedLine>>) -> Lyrics {
        Lyrics {
            synced: Some(synced.into()),
            timing,
            ..Default::default()
        }
    }

    #[test]
    fn a_lyricsfile_of_other_lines_times_the_lrc_words() {
        // The Lyricsfile joins the LRC's two lines into one and starts it a little early.
        let mut l = lyrics(
            "[00:10.00] I took a walk\n[00:12.00] around the world\n",
            Some(vec![TimedLine {
                text: "I took a walk around the world".into(),
                start: 9.8,
                end: None,
                words: vec![
                    word("I", 9.8),
                    word("took", 10.4),
                    word("a", 10.8),
                    word("walk", 11.1),
                    word("around", 12.1),
                    word("the", 12.6),
                    word("world", 12.9),
                ],
            }]),
        );
        fill(&mut l, || None, || None);
        let t = l.timing.unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].start, 10.0);
        let starts: Vec<f64> = t[0].words.iter().map(|w| w.start).collect();
        assert_eq!(starts, vec![10.0, 10.4, 10.8, 11.1]);
        let starts: Vec<f64> = t[1].words.iter().map(|w| w.start).collect();
        assert_eq!(starts, vec![12.0, 12.6, 12.9]);
    }

    #[test]
    fn lines_that_already_have_their_words_keep_them() {
        let own = TimedLine {
            text: "one two".into(),
            start: 1.0,
            end: Some(2.5),
            words: vec![word("one", 1.0), word("two", 1.7)],
        };
        let mut l = lyrics(
            "[00:01.00] one two\n[00:03.00] three four\n",
            Some(vec![own.clone()]),
        );
        fill(
            &mut l,
            || {
                Some(vec![Heard {
                    text: "four".into(),
                    start: 3.65,
                    end: 4.0,
                    p: 0.9,
                }])
            },
            || None,
        );
        let t = l.timing.unwrap();
        assert_eq!(t[0], own);
        assert_eq!(t[1].words.len(), 2);
        assert_eq!(t[1].words[0].start, 3.0);
        assert!(
            (t[1].words[1].start - 3.65).abs() < 1e-9,
            "{:?}",
            t[1].words
        );
    }

    #[test]
    fn engine_words_below_the_gate_are_not_shown() {
        let mut l = lyrics("[00:01.00] a b c d e f\n[00:05.00] g h i j k l\n", None);
        fill(
            &mut l,
            || {
                Some(vec![Heard {
                    text: "b".into(),
                    start: 1.6,
                    end: 1.8,
                    p: 1.0,
                }])
            },
            || None,
        );
        assert!(l.timing.is_none());
    }

    #[test]
    fn refined_starts_are_used_only_for_the_line_they_were_made_for() {
        let heard = || {
            Some(vec![
                Heard {
                    text: "two".into(),
                    start: 1.5,
                    end: 1.8,
                    p: 1.0,
                },
                Heard {
                    text: "four".into(),
                    start: 3.5,
                    end: 3.8,
                    p: 1.0,
                },
            ])
        };
        let refined = vec![
            RefinedLine {
                text: "one two".into(),
                start: 1.0,
                starts: vec![1.0, 1.42],
            },
            // Made when the second line started earlier: not used.
            RefinedLine {
                text: "three four".into(),
                start: 2.5,
                starts: vec![2.5, 3.1],
            },
        ];
        let mut l = lyrics("[00:01.00] one two\n[00:03.00] three four\n", None);
        let p = placement(&l, heard).unwrap();
        assert!(!p.refined_by(&refined));
        assert!(p.refined_by(
            &refined[..1]
                .iter()
                .cloned()
                .chain([RefinedLine {
                    text: "three four".into(),
                    start: 3.0,
                    starts: vec![3.0, 3.4],
                }])
                .collect::<Vec<_>>()
        ));
        fill(&mut l, heard, || Some(refined.clone()));
        let t = l.timing.unwrap();
        assert_eq!(t[0].words[1].start, 1.42);
        assert_eq!(t[1].words[1].start, 3.5);
    }

    #[test]
    fn hearing_is_wanted_only_for_lines_the_records_own_words_cant_place() {
        let partial = lyrics(
            "[00:01.00] one two
[00:03.00] three four
",
            Some(vec![TimedLine {
                text: "one two".into(),
                start: 1.2,
                end: None,
                words: vec![word("one", 1.2), word("two", 1.7)],
            }]),
        );
        assert!(wants_hearing(&partial));
        let whole = lyrics(
            "[00:01.00] one two
[00:03.00] three four
",
            Some(vec![TimedLine {
                text: "one two three four".into(),
                start: 1.2,
                end: None,
                words: vec![
                    word("one", 1.2),
                    word("two", 1.7),
                    word("three", 3.1),
                    word("four", 3.6),
                ],
            }]),
        );
        assert!(!wants_hearing(&whole));
        assert!(wants_hearing(&lyrics(
            "[00:01.00] one two
",
            None
        )));
        assert!(!wants_hearing(&lyrics(
            "[00:01.00] one
",
            None
        )));
    }

    #[test]
    fn nothing_to_go_on_leaves_it_alone() {
        let mut l = lyrics("[00:01.00] a b\n", None);
        fill(&mut l, || None, || None);
        assert!(l.timing.is_none());
    }
}
