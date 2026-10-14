//! Cutting a track into overlapping windows for the speech model, and stitching the words back.
//!
//! Parakeet run over a whole song stops transcribing reliably after about three minutes — every
//! later line was wrong in the spike. Thirty-second windows overlapping by six fixed that and were
//! faster too (`plans/auto-synced-lyrics.md`, "Sweep results").
//!
//! Where two windows overlap, each keeps only the words that start in its half of the overlap.
//! Every word is then taken from the window where it sits furthest from an edge, so none is
//! counted twice and none comes from the very end of a window, where it may be cut off.

use crate::Heard;

/// Samples per second the speech model takes.
pub const RATE: usize = 16_000;
/// Window length. See the module docs for why it is not the whole track.
pub const SIZE: usize = 30 * RATE;
/// How much consecutive windows share.
pub const OVERLAP: usize = 6 * RATE;

/// One window, in samples from the start of the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: usize,
    pub end: usize,
}

impl Window {
    /// Where the window starts, in seconds.
    pub fn start_seconds(&self) -> f64 {
        self.start as f64 / RATE as f64
    }
}

/// The windows covering `total` samples, in order.
pub fn plan(total: usize, size: usize, overlap: usize) -> Vec<Window> {
    let step = size.saturating_sub(overlap).max(1);
    let mut out = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + size).min(total);
        out.push(Window { start, end });
        if end >= total {
            break;
        }
        start += step;
    }
    out
}

/// The span of track time, in seconds, whose words window `k` keeps.
pub fn keep(windows: &[Window], k: usize) -> (f64, f64) {
    let seconds = |samples: usize| samples as f64 / RATE as f64;
    let lo = match k.checked_sub(1).map(|p| windows[p]) {
        Some(prev) => seconds(windows[k].start + prev.end) / 2.0,
        None => f64::NEG_INFINITY,
    };
    let hi = match windows.get(k + 1) {
        Some(next) => seconds(next.start + windows[k].end) / 2.0,
        None => f64::INFINITY,
    };
    (lo, hi)
}

/// The order to work through `windows` so the one holding `from_seconds` comes first, then the
/// rest of the track after it, then what came before.
///
/// For a song that is already playing: lines behind the playhead are worth the least, so they
/// are done last rather than first.
pub fn order_from(windows: &[Window], from_seconds: f64) -> Vec<usize> {
    let at = (from_seconds.max(0.0) * RATE as f64) as usize;
    let first = windows.iter().rposition(|w| w.start <= at).unwrap_or(0);
    (first..windows.len()).chain(0..first).collect()
}

/// Move one window's words onto the track's clock and keep only its share of any overlap.
pub fn place(windows: &[Window], k: usize, words: Vec<Heard>) -> Vec<Heard> {
    let offset = windows[k].start_seconds();
    let (lo, hi) = keep(windows, k);
    words
        .into_iter()
        .map(|mut w| {
            w.start += offset;
            w.end += offset;
            w
        })
        .filter(|w| w.start >= lo && w.start < hi)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f64) -> Heard {
        Heard {
            text: text.into(),
            start,
            end: start + 0.2,
            p: 1.0,
        }
    }

    #[test]
    fn plans_overlapping_windows_to_the_end() {
        let w = plan(70 * RATE, SIZE, OVERLAP);
        let starts: Vec<usize> = w.iter().map(|w| w.start / RATE).collect();
        assert_eq!(starts, [0, 24, 48]);
        assert_eq!(w.last().unwrap().end, 70 * RATE);
    }

    #[test]
    fn a_short_track_is_one_window() {
        assert_eq!(
            plan(10 * RATE, SIZE, OVERLAP),
            [Window {
                start: 0,
                end: 10 * RATE
            }]
        );
    }

    #[test]
    fn overlaps_split_down_the_middle() {
        let w = plan(70 * RATE, SIZE, OVERLAP);
        // Window 0 is 0–30, window 1 is 24–54: they share 24–30 and split it at 27.
        assert_eq!(keep(&w, 0), (f64::NEG_INFINITY, 27.0));
        assert_eq!(keep(&w, 1), (27.0, 51.0));
        assert_eq!(keep(&w, 2), (51.0, f64::INFINITY));
    }

    #[test]
    fn placing_shifts_to_track_time_and_drops_the_other_half() {
        let w = plan(70 * RATE, SIZE, OVERLAP);
        // In window 1 (starts at 24 s): 1.0 → 25 s (window 0's half), 5.0 → 29 s (ours).
        let kept = place(&w, 1, vec![word("early", 1.0), word("ours", 5.0)]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "ours");
        assert_eq!(kept[0].start, 29.0);
    }

    #[test]
    fn order_starts_at_the_playhead_and_wraps() {
        let w = plan(100 * RATE, SIZE, OVERLAP);
        assert_eq!(order_from(&w, 0.0), [0, 1, 2, 3]);
        assert_eq!(order_from(&w, 50.0), [2, 3, 0, 1]);
    }
}
