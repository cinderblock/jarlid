//! Fetching the start of a track early, so a blend can begin before it is playing.
//!
//! Blending needs two things a normal player never has: audio from the *next* song while the
//! current one is still going, and its tempo before a single frame of it has been heard. Both are
//! impossible from the streaming path, which is deliberately throttled to roughly playback speed
//! and so knows nothing about a track until it is already playing it.
//!
//! Decoding is far faster than that when nothing throttles it. Measured against Pandora's own
//! CDN: **769 KB in 0.4 s**, about 48 seconds of 128 kbit/s audio at ~120x realtime. So the start
//! of the next track can be pulled into memory in well under a second, which is what makes the
//! whole feature affordable — the two-connection window is a fraction of a second rather than the
//! length of a crossfade. See `crates/engine/examples/concurrent-streams.rs` for the measurement,
//! and `plans/dj-blend-mode.md` for why that mattered.
//!
//! Thirty seconds of 48 kHz stereo is under 6 MB, so holding it is not worth optimising.

use std::time::Duration;

use crate::{Decoder, Format, Result, Source, Tempo, TempoTracker};

/// The start of a track, decoded and measured.
pub struct Prefetched {
    /// Interleaved PCM in `format`, starting at the beginning of the track.
    pub pcm: Vec<i16>,
    pub format: Format,
    /// What the container held. Same measured-not-labelled answer the technical readout uses.
    pub source: Option<Source>,
    /// The tempo, if this much of the track had a clear enough pulse to give one.
    ///
    /// `None` is a perfectly ordinary answer and the caller must have a plan for it: a track that
    /// opens on a fade, a spoken intro, or anything without a steady beat. It means "do not
    /// beat-match this", not "something went wrong".
    pub tempo: Option<Tempo>,
}

impl Prefetched {
    /// How long the buffered audio plays for.
    pub fn duration(&self) -> Duration {
        let frames = self.pcm.len() / self.format.channels.max(1) as usize;
        Duration::from_secs_f64(frames as f64 / self.format.sample_rate.max(1) as f64)
    }

    /// Frames held.
    pub fn frames(&self) -> usize {
        self.pcm.len() / self.format.channels.max(1) as usize
    }
}

/// Decode the first `wanted` of `url` as fast as it will come, and measure its tempo.
///
/// `into` is the format to decode to — the output device's, so the result can be mixed against
/// whatever is already playing without a second conversion.
///
/// Stops at `wanted` rather than reading the whole track: the rest arrives through the normal
/// streaming path once the blend is over, and downloading entire songs to throw most away would
/// turn a quarter-second burst into a real second stream.
pub fn prefetch(url: &str, into: Format, wanted: Duration) -> Result<Prefetched> {
    let mut decoder = Decoder::open_at(url, Some(into))?;
    let format = decoder.format();
    let source = decoder.source();

    let channels = format.channels.max(1) as usize;
    let target_frames = (wanted.as_secs_f64() * format.sample_rate as f64) as usize;
    let target_samples = target_frames * channels;

    let mut pcm: Vec<i16> = Vec::with_capacity(target_samples);
    let mut tempo = TempoTracker::new(format.sample_rate, format.channels);

    while pcm.len() < target_samples {
        let Some(chunk) = decoder.next_chunk()? else {
            break;
        };
        let start = pcm.len();
        pcm.extend(
            chunk
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]])),
        );
        // Measure as it arrives rather than in a second pass over the buffer: the tracker is a
        // streaming one, and feeding it twice would cost another walk of six megabytes for
        // exactly the same answer.
        tempo.push(&pcm[start..]);
    }

    // Never leave a partial frame on the end. The mixer reads whole frames and would refuse the
    // last one anyway, but a buffer whose length is not a multiple of the channel count is the
    // sort of thing that later grows an off-by-one somewhere else.
    let whole = pcm.len() - (pcm.len() % channels);
    pcm.truncate(whole);

    Ok(Prefetched {
        pcm,
        format,
        source,
        tempo: tempo.tempo(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stock Windows sound, so the check costs the repo no fixture. Being a ringtone it repeats,
    /// which means it does have a measurable pulse — so it exercises the tempo path too, rather
    /// than only the shape of the result.
    const SOUND: &str = r"C:\Windows\Media\Ring05.wav";

    fn stereo_48k() -> Format {
        Format {
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 16,
        }
    }

    #[test]
    fn decodes_the_start_into_memory() {
        if !std::path::Path::new(SOUND).exists() {
            eprintln!("skipping: {SOUND} not present");
            return;
        }
        let got = prefetch(SOUND, stereo_48k(), Duration::from_secs(30)).expect("prefetch");

        assert_eq!(
            got.format.sample_rate, 48_000,
            "asked for the device's rate"
        );
        assert_eq!(got.format.channels, 2);
        assert!(!got.pcm.is_empty(), "nothing was decoded");
        assert_eq!(got.pcm.len() % 2, 0, "a partial frame was left on the end");
        assert!(
            got.duration() > Duration::from_millis(500),
            "{:?}",
            got.duration()
        );
    }

    /// The cap is the point: a blend needs the first few seconds, not the whole song, and
    /// pulling entire tracks to discard most of them would turn a burst into a second stream.
    #[test]
    fn stops_at_the_length_asked_for() {
        if !std::path::Path::new(SOUND).exists() {
            eprintln!("skipping: {SOUND} not present");
            return;
        }
        let brief = prefetch(SOUND, stereo_48k(), Duration::from_millis(300)).expect("prefetch");
        // Decoders emit whole chunks, so it overshoots a little — what matters is that it stopped
        // rather than reading to the end.
        assert!(
            brief.duration() < Duration::from_secs(2),
            "asked for 0.3s and got {:?}",
            brief.duration()
        );
    }

    /// Measuring as the audio arrives must give the same answer as measuring the finished
    /// buffer. The tracker is fed one decoded chunk at a time, from an offset into a `Vec` that
    /// is growing underneath it — an off-by-one there would feed it a sample twice or skip one,
    /// and the tempo would come out subtly wrong rather than obviously broken.
    ///
    /// (A repeating ringtone turns out to have a perfectly good pulse, which is why this asserts
    /// equivalence rather than absence — an earlier version of this test assumed a chime was
    /// unmusical and was simply wrong about that.)
    ///
    /// The two are compared loosely, and deliberately. `TempoTracker` analyses on a cadence and
    /// keeps its most confident window, so chunking changes *which* windows it ever looks at and
    /// the answers are close rather than identical — measured at 104.014 against 103.999, which
    /// is 0.015%. That is the cadence, not an error. A feed that duplicated or skipped samples
    /// would not be a fraction of a percent out; it would report a different tempo entirely, and
    /// a 1% band catches that while leaving the cadence alone.
    #[test]
    fn measuring_as_it_arrives_matches_measuring_the_whole_buffer() {
        if !std::path::Path::new(SOUND).exists() {
            eprintln!("skipping: {SOUND} not present");
            return;
        }
        let got = prefetch(SOUND, stereo_48k(), Duration::from_secs(30)).expect("prefetch");

        let mut whole = TempoTracker::new(got.format.sample_rate, got.format.channels);
        whole.push(&got.pcm);

        match (got.tempo, whole.tempo()) {
            (None, None) => {}
            (Some(streamed), Some(batch)) => assert!(
                (streamed.bpm / batch.bpm - 1.0).abs() < 0.01,
                "streamed {streamed:?} against batched {batch:?}"
            ),
            (a, b) => panic!("streamed gave {a:?} but the whole buffer gave {b:?}"),
        }
    }

    #[test]
    fn a_missing_source_fails_rather_than_panicking() {
        let err = prefetch("Z:\\nope.mp3", stereo_48k(), Duration::from_secs(5));
        assert!(err.is_err());
    }
}
