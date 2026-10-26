//! Synced lyrics from audio.
//!
//! A speech model run over a song hears *something* — on sung vocals, often not quite the words.
//! When the real words are known (LRCLIB's plain lyrics), they do not need to be heard correctly,
//! only found: this crate lines the known words up against the heard ones and reads each lyric
//! line's start time off the words that matched.
//!
//! The model is not in here. It runs in a separately downloaded sidecar, so this crate deals only
//! in its output: timed words ([`Heard`]).
//!
//! - [`text`] — the one normalisation both sides are compared in.
//! - [`align`] — lyric words ↔ heard words, then line start times.
//! - [`draft`] — when there are no lyric words to align, the heard words as timed lines.
//! - [`lrc`] — reading and writing LRC.
//! - [`score`] — how close a result is to a known-good synced copy.
//! - [`window`] — cutting a track into overlapping windows for the model, and stitching back.

pub mod align;
pub mod draft;
pub mod lrc;
pub mod score;
pub mod text;
pub mod window;

/// One word the speech model heard, with when it heard it.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    pub text: String,
    /// Seconds from the start of the track.
    pub start: f64,
    pub end: f64,
    /// The model's confidence, 0–1. Informational; alignment does not depend on it.
    pub p: f32,
}
