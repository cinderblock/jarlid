//! Timing known lyric lines from heard words, without ever changing the lyrics.
//!
//! The lyrics are the truth: every line is sung, in the order written, and takes time to sing.
//! What the speech model heard is evidence of *when*, never of *what*. It mishears sung words
//! often, hears plenty that isn't in the lyrics (ad-libs, backing vocals, a hallucinated "Thank
//! you for watching" over an outro), and hears nothing at all through some passages.
//!
//! The word alignment is a global sequence alignment (Needleman–Wunsch) with free leading and
//! trailing *heard* words, so what the model heard outside the lyrics costs nothing at the ends.
//! Lyric words the model missed are a normal gap, not a free one, and they still have to be sung
//! somewhere: a run of them costs far more when the silence it is put in is too short to sing it.
//! Without that, one heard "I want … like this" could anchor two copies of the line with thirty
//! words between them, squeezing those thirty words into half a second.
//!
//! Order is what makes this work on songs at all. A chorus repeats the same words four times, so
//! a word on its own says nothing about *which* chorus it is; aligning the whole sequence at once
//! does.

use std::collections::VecDeque;

use crate::text::{similarity, words};
use crate::Heard;

/// Fallback seconds-per-word, when too few words matched to measure it.
const DEFAULT_WORD_SECONDS: f64 = 0.35;

/// Slack, seconds, added to the room in every silence: heard word times are good to about a
/// tenth of a second, and a short word can hide between two heard back to back. 0 and 0.5 both
/// did a little worse on the corpus.
const SLACK: f64 = 0.25;

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
    /// evidence, and a line anchored on it drags every line placed around it.
    pub min_anchor_words: usize,
    /// Seconds subtracted from every line. Lyrics are stamped where the line *begins to be
    /// heard*, and a highlight that lands a beat early reads better than one a beat late.
    /// Measured: Parakeet's windowed word starts run 0.15 s behind LRCLIB's hand-synced lines on
    /// the 56-track corpus, and taking 0.15 s off moved lines within 0.3 s from 52% to 59%. Other
    /// engines will need their own figure.
    pub lead: f64,
    /// Seconds each unheard lyric word is taken to need: a silence between two heard words has
    /// room for that many of them. Not the fastest anyone sings, which would make room for far
    /// more, but a typical pace; words beyond it aren't forbidden, only costly (`crowding`).
    /// Measured on the corpus, 0.2 and 0.4 did equally well on songs that pass the coverage gate,
    /// and 0.4 much better on songs the model barely heard (p90 2.75 → 2.45 s overall), which is
    /// what a song looks like partway through being heard.
    pub unheard_word_seconds: f64,
    /// Extra cost of each unheard lyric word beyond the room in the silence it was put in.
    pub crowding: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            match_sim: 0.6,
            gap: -0.4,
            mismatch: -1.0,
            min_anchor_words: 2,
            lead: 0.15,
            unheard_word_seconds: 0.4,
            crowding: -2.0,
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
    /// Which stanza the line is in: blank lines separate them.
    pub stanza: usize,
}

/// The result for a whole song.
#[derive(Debug, Clone)]
pub struct Alignment {
    /// One entry per input line, blank lines included.
    pub lines: Vec<LineTime>,
    pub lyric_words: usize,
    pub matched_words: usize,
    /// Seconds per sung word, measured from consecutive matched words.
    pub word_seconds: f64,
    /// Seconds from one line to the next (see [`line_pace`]), or `None` when too few lines
    /// anchored to measure it and each line is given its words' worth instead.
    pub line_seconds: Option<f64>,
}

/// Pause assumed between lines when spacing out lines nothing anchors.
const LINE_GAP: f64 = 0.5;

/// Seconds from the start of `line` to the next line's, for placing lines nothing anchored.
fn step(line_seconds: Option<f64>, word_seconds: f64, line: &LineTime) -> f64 {
    line_seconds.unwrap_or(line.words as f64 * word_seconds + LINE_GAP)
}

impl Alignment {
    /// Every non-blank line with a time, in order, for display.
    ///
    /// Lines between anchors already have one. Lines before the first anchor or after the last
    /// are stepped out from it at the song's line pace, inside `0..=duration`. Without this they
    /// would vanish from the pane, which shows only timed lines. Empty if nothing anchored at all.
    pub fn timed(&self, lines: &[&str], duration: f64) -> Vec<crate::lrc::Line> {
        lines
            .iter()
            .zip(self.placed(duration))
            .filter_map(|(text, t)| {
                t.map(|t| crate::lrc::Line {
                    t,
                    text: text.trim().to_string(),
                })
            })
            .collect()
    }

    /// One time per input line, as [`Alignment::timed`] shows them: `None` only for blank lines,
    /// or for every line when nothing anchored.
    pub fn placed(&self, duration: f64) -> Vec<Option<f64>> {
        let mut t: Vec<Option<f64>> = self.lines.iter().map(|l| l.t).collect();
        let step = |i: usize| step(self.line_seconds, self.word_seconds, &self.lines[i]);
        let sung = |i: &usize| self.lines[*i].words > 0;
        let Some(first) = t.iter().position(Option::is_some) else {
            return t;
        };
        let last = t.iter().rposition(Option::is_some).unwrap_or(first);

        // Back from the first anchor, squeezed into the time before it if they don't fit.
        let before: Vec<usize> = (0..first).filter(sung).collect();
        let t_first = t[first].unwrap_or_default();
        let mut at = t_first;
        for &i in before.iter().rev() {
            at -= step(i);
            t[i] = Some(at);
        }
        if at < 0.0 {
            for &i in &before {
                t[i] = t[i].map(|x| t_first * (x - at) / (t_first - at));
            }
        }

        // On from the last anchor, squeezed into what is left of the song if they don't fit.
        let after: Vec<usize> = (last + 1..t.len()).filter(sung).collect();
        let t_last = t[last].unwrap_or_default();
        let (mut at, mut prev) = (t_last, last);
        for &i in &after {
            at += step(prev);
            t[i] = Some(at);
            prev = i;
        }
        let end = at + step(prev);
        if end > duration && duration > t_last {
            for &i in &after {
                t[i] = t[i].map(|x| t_last + (x - t_last) * (duration - t_last) / (end - t_last));
            }
        }

        for (t, line) in t.iter_mut().zip(&self.lines) {
            if line.words == 0 {
                *t = None;
            }
        }
        t
    }

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

    let pairs = match_words(&lyric, heard, &heard_words, params);
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

    let mut stanza = 0;
    let mut out: Vec<LineTime> = lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                stanza += 1;
            }
            LineTime {
                t: None,
                words: 0,
                matched: 0,
                interpolated: false,
                stanza,
            }
        })
        .collect();
    // Each line from its own first matched word, stepped back over any unmatched words before it.
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
    let line_seconds = line_pace(&out);
    interpolate(&mut out, |line| step(line_seconds, word_seconds, line));

    Alignment {
        lines: out,
        lyric_words: lyric.len(),
        matched_words,
        word_seconds,
        line_seconds,
    }
}

/// For each lyric word, the index of the heard word it aligned to, if any.
///
/// Needleman–Wunsch with one addition: lyric words the model didn't hear still have to be sung
/// somewhere, and the only place for them is the silence between two heard words. A run of them
/// placed in a silence too short to sing them in pays [`Params::crowding`] for every word that
/// doesn't fit. That is what stops one heard "I want … like this" being split across two copies
/// of the line with thirty words between them.
fn match_words(
    lyric: &[(usize, String)],
    heard: &[Heard],
    heard_words: &[String],
    params: &Params,
) -> Vec<Option<usize>> {
    let (n, m) = (lyric.len(), heard.len());
    let width = m + 1;
    let at = |i: usize, j: usize| i * width + j;

    // How many unheard words fit before heard word `j` (column `j`): between the previous heard
    // word and this one, or from the top of the song for a run that starts the song. After the
    // last heard word there's no limit.
    let fit = |seconds: f64| ((seconds.max(0.0) + SLACK) / params.unheard_word_seconds) as usize;
    let room: Vec<usize> = (0..=m)
        .map(|j| match j {
            _ if j == m => usize::MAX,
            0 => fit(heard[0].start),
            _ => fit(heard[j].start - heard[j - 1].end),
        })
        .collect();
    let room_from_top: Vec<usize> = (0..=m)
        .map(|j| {
            if j == m {
                usize::MAX
            } else {
                fit(heard[j].start)
            }
        })
        .collect();
    // The cost of `r` unheard words where `room` fit.
    let run = |r: usize, room: usize| {
        r as f64 * params.gap + r.saturating_sub(room) as f64 * params.crowding
    };

    let pair = |i: usize, j: usize| {
        let s = similarity(&lyric[i].1, &heard_words[j]);
        if s >= params.match_sim {
            2.0 * s - 0.5
        } else {
            params.mismatch
        }
    };

    // `s`: best score ending on a matched pair or a skipped heard word. `u`: ending on a run of
    // skipped lyric words, which `u_from` says the row of.
    let mut s = vec![f64::NEG_INFINITY; (n + 1) * width];
    let mut u = vec![f64::NEG_INFINITY; (n + 1) * width];
    // 0 = diagonal from s, 1 = diagonal from u, 2 = heard gap from s, 3 = heard gap from u.
    let mut s_from = vec![0u8; (n + 1) * width];
    let mut u_from = vec![0u32; (n + 1) * width];
    // Row 0 is the top of the song: heard words before the first lyric word are free.
    for j in 0..=m {
        s[at(0, j)] = 0.0;
    }

    // A run of `r` words ending at row `i` costs `r × gap` while it fits the room and
    // `r × (gap + crowding)` past it, so the best start row for each column can be kept as it
    // goes: a sliding window over the rows that fit, and a running best over the rows before.
    let over = params.gap + params.crowding;
    let mut window: Vec<VecDeque<(usize, f64)>> = vec![VecDeque::new(); width];
    let mut before: Vec<(usize, f64)> = vec![(0, f64::NEG_INFINITY); width];
    let mut next_before: Vec<usize> = vec![1; width];

    for i in 1..=n {
        for j in 0..=m {
            // Runs from rows 1.. (row 0 is the top of the song; its runs are taken below).
            let room_here = room[j];
            if i >= 2 {
                let k = i - 1;
                let value = s[at(k, j)] - k as f64 * params.gap;
                if value.is_finite() {
                    let w = &mut window[j];
                    while w.back().is_some_and(|&(_, v)| v <= value) {
                        w.pop_back();
                    }
                    w.push_back((k, value));
                }
            }
            let lowest = i.saturating_sub(room_here).max(1);
            while window[j].front().is_some_and(|&(k, _)| k < lowest) {
                window[j].pop_front();
            }
            while next_before[j] < lowest {
                let k = next_before[j];
                let value = s[at(k, j)] - k as f64 * over;
                if value > before[j].1 {
                    before[j] = (k, value);
                }
                next_before[j] += 1;
            }
            let mut best_u = f64::NEG_INFINITY;
            let mut best_k = 0;
            if let Some(&(k, v)) = window[j].front() {
                best_u = v + i as f64 * params.gap;
                best_k = k;
            }
            let (k, v) = before[j];
            if v.is_finite() {
                let value = v + i as f64 * over - room_here as f64 * params.crowding;
                if value > best_u {
                    (best_u, best_k) = (value, k);
                }
            }
            let from_top = run(i, room_from_top[j]);
            if from_top > best_u {
                (best_u, best_k) = (from_top, 0);
            }
            u[at(i, j)] = best_u;
            u_from[at(i, j)] = best_k as u32;

            if j > 0 {
                let p = pair(i - 1, j - 1);
                let options = [
                    s[at(i - 1, j - 1)] + p,
                    u[at(i - 1, j - 1)] + p,
                    s[at(i, j - 1)] + params.gap,
                    u[at(i, j - 1)] + params.gap,
                ];
                let mut dir = 0;
                for (d, &v) in options.iter().enumerate() {
                    if v > options[dir] {
                        dir = d;
                    }
                }
                s[at(i, j)] = options[dir];
                s_from[at(i, j)] = dir as u8;
            }
        }
    }

    // Heard words after the last lyric word are free too, and lyric words after the last match
    // have all the room they need: the song carries on past whatever heard words are left over.
    // So the alignment can end at any column, on any row, the rest of the lyrics costing a gap
    // each.
    let mut end = (0, n, f64::NEG_INFINITY);
    for j in 0..=m {
        for k in 0..=n {
            let v = s[at(k, j)] + (n - k) as f64 * params.gap;
            if v > end.2 {
                end = (j, k, v);
            }
        }
    }
    let (mut j, mut i, _) = end;
    let mut in_run = false;
    let mut out = vec![None; n];
    while i > 0 {
        if in_run {
            i = u_from[at(i, j)] as usize;
            in_run = false;
            continue;
        }
        match s_from[at(i, j)] {
            d @ (0 | 1) if j > 0 => {
                if similarity(&lyric[i - 1].1, &heard_words[j - 1]) >= params.match_sim {
                    out[i - 1] = Some(j - 1);
                }
                in_run = d == 1;
                i -= 1;
                j -= 1;
            }
            d @ (2 | 3) if j > 0 => {
                in_run = d == 3;
                j -= 1;
            }
            _ => break,
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

/// Seconds from one line to the next, measured between lines of the same stanza that both
/// anchored, one straight after the other. `None` when fewer than three pairs did.
///
/// Song lines land on bars, so the gap from one line to the next stays much the same whatever
/// the words: "Hip-hip" takes a bar as surely as "We'll be playing and having fun" does.
fn line_pace(lines: &[LineTime]) -> Option<f64> {
    let sung: Vec<&LineTime> = lines.iter().filter(|l| l.words > 0).collect();
    let mut gaps: Vec<f64> = sung
        .windows(2)
        .filter(|w| w[0].stanza == w[1].stanza)
        .filter_map(|w| Some(w[1].t? - w[0].t?))
        .filter(|&g| g > 0.0)
        .collect();
    if gaps.len() < 3 {
        return None;
    }
    median(&mut gaps)
}

/// Give unanchored, non-blank lines a time between their anchored neighbours.
///
/// A stanza is sung straight through, and breaks fall between stanzas. So lines between two
/// anchors of one stanza are spread evenly between them. Otherwise the rest of the earlier
/// anchor's stanza follows it a step at a time, the start of the later anchor's stanza leads into
/// it the same way, and whole stanzas between the two go in the middle of the time left, since
/// nothing says which side of them the break fell. Lines that don't fit like that are spread
/// evenly too.
fn interpolate(lines: &mut [LineTime], step: impl Fn(&LineTime) -> f64) {
    let anchors: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].t.is_some()).collect();
    for pair in anchors.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let between: Vec<usize> = (a + 1..b).filter(|&i| lines[i].words > 0).collect();
        if between.is_empty() {
            continue;
        }
        let (ta, tb) = (
            lines[a].t.unwrap_or_default(),
            lines[b].t.unwrap_or_default(),
        );
        let order: Vec<usize> = std::iter::once(a).chain(between.iter().copied()).collect();
        let mut times = vec![0.0; between.len()];
        // Within one stanza there's no break for the spare time to go into: spread them.
        let mut fits = lines[a].stanza != lines[b].stanza;
        if fits {
            // The ends of the stanzas either side, then whole stanzas between them in the middle
            // of what time is left: nothing says which side of them the break was.
            let following = between
                .iter()
                .take_while(|&&i| lines[i].stanza == lines[a].stanza)
                .count();
            let leading = between
                .iter()
                .rev()
                .take_while(|&&i| lines[i].stanza == lines[b].stanza)
                .count();
            let middle = following..between.len() - leading;

            let mut t = ta;
            for k in 0..following {
                t += step(&lines[order[k]]);
                times[k] = t;
            }
            let after_following = t + step(&lines[order[following]]);
            let mut t = tb;
            for k in (between.len() - leading..between.len()).rev() {
                t -= step(&lines[between[k]]);
                times[k] = t;
            }
            if !middle.is_empty() {
                let needed: f64 = middle.clone().map(|k| step(&lines[between[k]])).sum();
                let mut t = after_following + ((t - after_following - needed) / 2.0).max(0.0);
                for k in middle {
                    times[k] = t;
                    t += step(&lines[between[k]]);
                }
            }

            // Every line, `a` and `b` included, at least most of a step after the one before.
            let at: Vec<f64> = std::iter::once(ta)
                .chain(times.iter().copied())
                .chain(std::iter::once(tb))
                .collect();
            fits = (0..order.len()).all(|k| at[k + 1] - at[k] >= 0.75 * step(&lines[order[k]]));
        }
        if !fits {
            let weights: Vec<f64> = order.iter().map(|&i| step(&lines[i])).collect();
            let total: f64 = weights.iter().sum();
            let mut done = 0.0;
            for (k, w) in weights.iter().enumerate().take(between.len()) {
                done += w;
                times[k] = ta + (tb - ta) * done / total;
            }
        }
        for (&i, &t) in between.iter().zip(&times) {
            lines[i].t = Some(t);
            lines[i].interpolated = true;
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

    #[test]
    fn timed_gives_every_line_a_time_in_order() {
        let lines = [
            "before one",
            "",
            "the anchor line",
            "after one",
            "after two",
        ];
        let h = heard(&[("the", 20.0), ("anchor", 20.4), ("line", 20.8)]);
        let a = exact(&lines, &h);
        let timed = a.timed(&lines, 30.0);
        let texts: Vec<&str> = timed.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            ["before one", "the anchor line", "after one", "after two"]
        );
        assert!(timed.windows(2).all(|w| w[0].t <= w[1].t), "{timed:?}");
        assert!(timed[0].t < 20.0 && timed[0].t >= 0.0);
        assert_eq!(timed[1].t, 20.0);
        assert!(timed[3].t <= 30.0);
    }

    #[test]
    fn timed_is_empty_when_nothing_anchored() {
        let lines = ["never heard"];
        assert!(exact(&lines, &[]).timed(&lines, 30.0).is_empty());
    }

    /// Words one after another from `start`, `gap` seconds apart.
    fn sung(text: &str, start: f64, gap: f64) -> Vec<Heard> {
        text.split(' ')
            .enumerate()
            .map(|(k, w)| Heard {
                text: w.into(),
                start: start + k as f64 * gap,
                end: start + k as f64 * gap + 0.25,
                p: 1.0,
            })
            .collect()
    }

    #[test]
    fn a_phrase_heard_once_is_not_split_across_two_copies_of_its_line() {
        // Something Just Like This, as Parakeet heard it: "I want some bites like this" once, at
        // 73 s. Word for word, giving "I want" to the first copy of the line below and "like
        // this" to the second scores the same as giving all four to one of them, and the old
        // aligner did: it squeezed the six lines between into half a second.
        let lines = [
            "Some superhero",
            "Some fairy tale bliss",
            "Just something I can turn to",
            "Somebody I can kiss\"",
            "I want something just like this",
            "Doo-doo-doo, doo-doo-doo",
            "Doo-doo-doo, doo, doo-doo",
            "Doo-doo-doo, doo-doo-doo",
            "Oh, I want something just like this",
            "Doo-doo-doo, doo-doo-doo",
            "Doo-doo-doo, doo, doo-doo",
            "Da-da-da, da-da-da",
            "Oh, I want something just like this",
            "",
            "I want something just like this",
            "",
            "I've been reading books of old",
        ];
        let h: Vec<Heard> = [
            ("some", 36.08, 36.32),
            ("superhero,", 36.56, 37.60),
            ("some", 38.32, 38.48),
            ("fairy", 38.80, 39.28),
            ("tale", 39.28, 39.76),
            ("bliss,", 39.92, 40.32),
            ("just", 40.80, 40.96),
            ("something", 41.20, 41.68),
            ("I", 41.68, 42.00),
            ("can", 42.00, 42.32),
            ("turn", 42.32, 42.56),
            ("to,", 42.56, 42.88),
            ("somebody", 43.20, 43.92),
            ("I", 43.92, 44.24),
            ("can", 44.24, 44.56),
            ("kiss.", 44.56, 44.96),
            ("I", 45.20, 45.52),
            ("want", 45.52, 45.84),
            ("something", 45.84, 46.48),
            ("just", 46.48, 46.80),
            ("like", 46.80, 47.12),
            ("this.", 47.12, 47.44),
            ("Do", 47.84, 48.00),
            ("you", 50.56, 50.72),
            ("do", 50.72, 50.88),
            ("I", 73.04, 73.36),
            ("want", 73.36, 73.52),
            ("some", 73.68, 73.84),
            ("bites", 74.16, 74.48),
            ("like", 74.64, 74.80),
            ("this", 75.20, 75.52),
            ("I've", 82.40, 82.72),
            ("been", 82.72, 82.88),
            ("reading", 83.04, 83.36),
            ("bits", 83.60, 83.84),
            ("and", 84.00, 84.16),
            ("bold,", 84.32, 84.88),
        ]
        .iter()
        .map(|&(text, start, end)| Heard {
            text: text.into(),
            start,
            end,
            p: 1.0,
        })
        .collect();
        let a = exact(&lines, &h);
        let t: Vec<f64> = a.placed(240.0).into_iter().flatten().collect();
        assert!(
            t.windows(2).all(|w| w[1] - w[0] >= 1.0),
            "lines squeezed together: {t:?}"
        );
    }

    #[test]
    fn unheard_lines_follow_their_stanza_at_the_line_pace() {
        // Island in the Sun: the last lines of a stanza go unheard, and a long break follows
        // them. They come a bar apart after the last line heard, not spread over the break.
        let lines = [
            "when you're on a holiday",
            "you can't find the words to say",
            "all the things that come to you",
            "and I wanna feel it too",
            "hip hip",
            "hip hip",
            "",
            "on an island in the sun",
        ];
        let mut h = Vec::new();
        for (k, line) in lines[..4].iter().enumerate() {
            h.extend(sung(line, 10.0 + 4.0 * k as f64, 0.4));
        }
        h.extend(sung("on an island in the sun", 60.0, 0.4));
        let a = exact(&lines, &h);
        assert_eq!(a.line_seconds, Some(4.0));
        let t = a.placed(90.0);
        assert!((t[4].unwrap() - 26.0).abs() < 1e-9, "{t:?}");
        assert!((t[5].unwrap() - 30.0).abs() < 1e-9, "{t:?}");
    }

    #[test]
    fn unheard_lines_inside_a_stanza_are_spread_between_its_anchors() {
        // No break inside a stanza: whatever time the anchors leave, the lines between share it.
        let lines = [
            "one line heard here",
            "and another heard here",
            "a third one heard",
            "the fourth one heard",
            "never heard",
            "never heard either",
            "the last line heard",
        ];
        let mut h = Vec::new();
        for (k, &line) in lines[..4].iter().enumerate() {
            h.extend(sung(line, 10.0 + 2.0 * k as f64, 0.3));
        }
        h.extend(sung(lines[6], 25.0, 0.3));
        let t = exact(&lines, &h).placed(60.0);
        assert!((t[4].unwrap() - 19.0).abs() < 1e-9, "{t:?}");
        assert!((t[5].unwrap() - 22.0).abs() < 1e-9, "{t:?}");
    }
}
