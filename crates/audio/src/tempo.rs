//! Tempo (BPM) estimation from streaming PCM.
//!
//! Pandora sends no tempo information — the tuner's track model has no BPM, key or any other
//! musicological field — so the only way to know how fast a song is, is to listen to it. This
//! measures it from the same decoded samples that reach the speakers.
//!
//! The design is deliberately FFT-free. Beat detection does not need spectral detail: it needs
//! to know *when* energy arrives, not at exactly which frequency. Three bands split by two
//! one-pole filters separate a kick from a hi-hat well enough to find a pulse, and everything
//! after that is autocorrelation over a ~200 Hz envelope. That keeps the whole feature at zero
//! dependencies, which is worth more here than the last few percent of accuracy.
//!
//! Deliberately not `#[cfg(windows)]`-gated, unlike the rest of this crate: it touches no
//! platform API, and its tests are the only thing standing between a plausible-looking number
//! and a wrong one.

/// A tempo estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    pub bpm: f32,
    /// How periodic the onset envelope actually is at `bpm`, from 0 to 1.
    ///
    /// This is the normalised autocorrelation at the winning lag — a real correlation
    /// coefficient rather than an invented score, so it can be compared across tracks. Steady
    /// electronic music lands around 0.4-0.7; rubato piano may never clear [`MIN_CONFIDENCE`],
    /// which is the honest answer for music that has no single tempo.
    pub confidence: f32,
}

/// Envelope samples per second to aim for. The hop is derived from this and the real sample
/// rate, so the envelope rate is near this but never assumed to be exactly it.
const ENVELOPE_RATE: f64 = 200.0;

/// The tempo range considered. Anything outside it is almost certainly a half- or double-time
/// reading of something inside it, and the harmonic summing below exists to fold those back in.
const MIN_BPM: f64 = 60.0;
const MAX_BPM: f64 = 200.0;

/// Band edges. Low is where a kick lives, high is where a hi-hat does; the mid is everything
/// else. Two one-pole filters give three bands.
const LOW_HZ: f64 = 200.0;
const HIGH_HZ: f64 = 4000.0;
const BANDS: usize = 3;

/// Log compression applied to per-band RMS before differencing.
///
/// Onsets are *relative* events: a snare in a quiet passage matters as much as one in a loud
/// chorus. Differencing raw energy would weight the chorus tenfold; differencing `ln(1 + C·rms)`
/// weights them alike, which is the whole reason beat trackers work on a log scale.
const COMPRESSION: f64 = 1000.0;

/// Half-width of the moving mean subtracted from the envelope before correlating. Wide enough
/// to span a beat or two, narrow enough to follow a song getting louder.
const ADAPT_HALF_SECONDS: f64 = 0.5;

/// Envelope needed before reporting anything. Three seconds of it goes on the longest lag the
/// harmonic sum reaches, so this is not as generous as it looks.
const MIN_SECONDS: f64 = 10.0;

/// How much recent envelope the estimate is drawn from. Longer is steadier but slower to
/// follow a genuine tempo change, and costs correlation time on the decode thread.
const WINDOW_SECONDS: f64 = 20.0;

/// Seconds of new envelope between analyses.
const ANALYSE_EVERY: f64 = 1.0;

/// Below this the pulse is too weak to call, and the tracker reports nothing rather than
/// offering a number that is really just the loudest bit of noise in the range.
const MIN_CONFIDENCE: f32 = 0.10;

/// Width of the log-normal tempo prior, in octaves.
///
/// Deliberately gentle: it breaks ties between a tempo and its double, and does nothing else.
/// A narrow prior would simply report 120 BPM for everything.
const PRIOR_OCTAVES: f64 = 0.9;

/// Estimates the tempo of a stream of PCM as it arrives.
///
/// Feed every decoded sample to [`TempoTracker::push`]; ask [`TempoTracker::tempo`] whenever
/// convenient. One tracker per continuously-decoded stream — it assumes what it is fed is
/// contiguous, so a seek or a re-opened connection wants a fresh one.
pub struct TempoTracker {
    channels: usize,
    /// Frames per envelope sample.
    hop: usize,
    /// Envelope samples per second, as it actually worked out.
    envelope_rate: f64,

    // One-pole lowpass coefficients and state, applied to the mono signal.
    low_coeff: f32,
    high_coeff: f32,
    low_pass: f32,
    high_pass: f32,

    /// Partial frame carried across `push` calls: a decoded chunk is an arbitrary number of
    /// bytes and can end mid-frame.
    frame_sum: f32,
    frame_have: usize,

    /// Frames accumulated into the hop currently being filled.
    hop_have: usize,
    /// Sum of squares per band for that hop.
    band_energy: [f64; BANDS],
    /// Previous hop's compressed level per band, for differencing.
    previous_level: [f64; BANDS],
    /// False until a first hop has closed and `previous_level` means something.
    primed: bool,

    /// The onset envelope, trimmed to [`WINDOW_SECONDS`].
    envelope: Vec<f32>,
    /// Envelope samples added since the last analysis.
    since_analysis: usize,

    result: Option<Tempo>,
}

impl TempoTracker {
    /// A tracker for interleaved PCM at `sample_rate` with `channels` channels.
    ///
    /// The rate is the *device's*, not the source's: Media Foundation resamples before we ever
    /// see a sample. Resampling preserves tempo, but the arithmetic below has to use the rate
    /// the samples actually arrive at or every reported BPM is wrong by the resampling ratio.
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let rate = sample_rate.max(1) as f64;
        let hop = (rate / ENVELOPE_RATE).round().max(1.0) as usize;

        // Standard one-pole coefficient: the fraction of the way to the input each sample
        // moves, for a given -3 dB corner.
        let coeff = |hz: f64| 1.0 - (-2.0 * std::f64::consts::PI * hz / rate).exp();

        Self {
            channels: channels.max(1) as usize,
            hop,
            envelope_rate: rate / hop as f64,
            low_coeff: coeff(LOW_HZ) as f32,
            high_coeff: coeff(HIGH_HZ) as f32,
            low_pass: 0.0,
            high_pass: 0.0,
            frame_sum: 0.0,
            frame_have: 0,
            hop_have: 0,
            band_energy: [0.0; BANDS],
            previous_level: [0.0; BANDS],
            primed: false,
            envelope: Vec::new(),
            since_analysis: 0,
            result: None,
        }
    }

    /// The best estimate so far, or `None` while the pulse is still unclear.
    ///
    /// Stays `None` for roughly the first [`MIN_SECONDS`] of a stream — decoding is throttled
    /// to about playback speed, so there is no way to have heard more than that yet.
    pub fn tempo(&self) -> Option<Tempo> {
        self.result
    }

    /// Feed interleaved samples. Must be every sample, in order.
    pub fn push(&mut self, samples: &[i16]) {
        for &sample in samples {
            self.frame_sum += sample as f32;
            self.frame_have += 1;
            if self.frame_have < self.channels {
                continue;
            }
            // Downmix by averaging, and scale to roughly -1..1 so the compression constant
            // means the same thing whatever the source level.
            let mono = self.frame_sum / (self.channels as f32 * 32768.0);
            self.frame_sum = 0.0;
            self.frame_have = 0;
            self.push_frame(mono);
        }

        let window = (WINDOW_SECONDS * self.envelope_rate) as usize;
        if self.envelope.len() > window {
            let excess = self.envelope.len() - window;
            self.envelope.drain(..excess);
        }

        let cadence = (ANALYSE_EVERY * self.envelope_rate) as usize;
        let minimum = (MIN_SECONDS * self.envelope_rate) as usize;
        if self.since_analysis >= cadence && self.envelope.len() >= minimum {
            self.since_analysis = 0;
            if let Some(tempo) = self.analyse() {
                // Keep the *most convincing* reading of the track so far, not the most recent.
                //
                // A sparse intro has no drums to correlate, and what pulse there is often sits
                // at two-thirds of the real tempo — the dotted note. Measured on a 128 BPM
                // track: 85.2 BPM at confidence 0.38 for the first two minutes, then 127.6 at
                // 0.58 the moment the beat arrives, and a brief slip back to 85 in a breakdown.
                // Confidence separates those cleanly, so preferring it stops the readout
                // flip-flopping every time a song thins out.
                //
                // The cost is that a genuine tempo change mid-track is not followed. For a
                // per-track readout on a radio stream that is the right trade; a tracker that
                // needed to follow one would want to age this out.
                if self
                    .result
                    .is_none_or(|best| tempo.confidence > best.confidence)
                {
                    self.result = Some(tempo);
                }
            }
        }
    }

    /// One mono frame through the filterbank and into the current hop.
    fn push_frame(&mut self, x: f32) {
        self.low_pass += self.low_coeff * (x - self.low_pass);
        self.high_pass += self.high_coeff * (x - self.high_pass);

        // Complementary split: low, what the 4 kHz filter passed minus the low, and the rest.
        let bands = [
            self.low_pass,
            self.high_pass - self.low_pass,
            x - self.high_pass,
        ];
        for (energy, band) in self.band_energy.iter_mut().zip(bands) {
            *energy += (band * band) as f64;
        }

        self.hop_have += 1;
        if self.hop_have < self.hop {
            return;
        }
        self.hop_have = 0;

        // Close the hop: half-wave-rectified rise in compressed level, summed over bands. Only
        // rises count — an onset is energy arriving, and counting its decay too would put a
        // second spurious peak after every beat.
        let mut flux = 0.0;
        for (band, energy) in self.band_energy.iter_mut().enumerate() {
            let rms = (*energy / self.hop as f64).sqrt();
            let level = (1.0 + COMPRESSION * rms).ln();
            if self.primed {
                flux += (level - self.previous_level[band]).max(0.0);
            }
            self.previous_level[band] = level;
            *energy = 0.0;
        }
        self.primed = true;

        self.envelope.push(flux as f32);
        self.since_analysis += 1;
    }

    /// Correlate the accumulated envelope against itself and pick a period.
    fn analyse(&self) -> Option<Tempo> {
        let n = self.envelope.len();
        let lag_min = (60.0 * self.envelope_rate / MAX_BPM).round().max(1.0) as usize;
        let lag_max = (60.0 * self.envelope_rate / MIN_BPM).round() as usize;
        if lag_max <= lag_min || n < 4 * lag_max {
            return None;
        }

        // Adaptive threshold. Subtracting a moving mean removes the slow swells — a song
        // getting louder is not a beat — and leaves the peaks that are. Half-wave rectified
        // because only what rises above the local average is an onset.
        let mut prefix = vec![0.0f64; n + 1];
        for (i, &value) in self.envelope.iter().enumerate() {
            prefix[i + 1] = prefix[i] + value as f64;
        }
        let half = (ADAPT_HALF_SECONDS * self.envelope_rate) as usize;
        let mut detail = vec![0.0f64; n];
        for (i, slot) in detail.iter_mut().enumerate() {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(n);
            let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
            *slot = (self.envelope[i] as f64 - mean).max(0.0);
        }

        // Centre it, so the autocorrelation below is a covariance and `acf[0]` a variance.
        // Without this every lag correlates strongly on the shared positive mean alone.
        let mean = detail.iter().sum::<f64>() / n as f64;
        for value in detail.iter_mut() {
            *value -= mean;
        }

        // Normalising each lag by its own overlap length matters: the raw sum has fewer terms
        // at longer lags, which tapers it and would bias every estimate fast.
        let max_lag = (4 * lag_max).min(n / 2);
        let mut acf = vec![0.0f64; max_lag + 1];
        for (lag, slot) in acf.iter_mut().enumerate() {
            let mut sum = 0.0;
            for i in 0..(n - lag) {
                sum += detail[i] * detail[i + lag];
            }
            *slot = sum / (n - lag) as f64;
        }
        if acf[0] <= 0.0 {
            return None;
        }

        let mut scores = vec![f64::NEG_INFINITY; lag_max + 1];
        for (lag, slot) in scores
            .iter_mut()
            .enumerate()
            .take(lag_max + 1)
            .skip(lag_min)
        {
            // A comb filter over the autocorrelation. Each harmonic contributes the height of
            // the peak at k·L *minus* the value halfway to the next one, and that subtraction
            // is the whole trick.
            //
            // Summing the peaks alone cannot tell a tempo from half of it: if L is the true
            // period then 2L, 3L … are peaks, but every multiple of 2L is also a multiple of L,
            // so both candidates score identically and the answer comes down to noise. (It did:
            // a 174 BPM click track read as exactly 87.) The midpoints break the tie. At the
            // true period, L/2 falls between beats and is a trough, so peak − midpoint is
            // large; at twice the true period the "midpoint" 1.5L lands on a real beat, so the
            // difference collapses to nothing.
            let mut score = 0.0;
            for (harmonic, weight) in [(1.0, 1.0), (2.0, 0.5), (3.0, 0.25)] {
                let peak = (harmonic * lag as f64).round() as usize;
                let trough = ((harmonic + 0.5) * lag as f64).round() as usize;
                // Both terms or neither — keeping a peak whose midpoint fell off the end would
                // quietly reinstate the bias this exists to remove.
                if trough <= max_lag {
                    score += weight * (acf[peak] - acf[trough]);
                }
            }

            // A gentle nudge toward tempos people actually count, to break what ties are left.
            // Wide enough that a real 75 or 170 BPM track still wins.
            let bpm = 60.0 * self.envelope_rate / lag as f64;
            let octaves = (bpm / 120.0).log2();
            *slot = score * (-0.5 * (octaves / PRIOR_OCTAVES).powi(2)).exp();
        }

        let best = (lag_min..=lag_max).max_by(|&a, &b| scores[a].total_cmp(&scores[b]))?;

        // The envelope is a ~200 Hz grid, so neighbouring lags are whole BPM apart up here.
        // Fitting a parabola through the peak and its neighbours recovers the fraction between
        // them, which is the difference between reporting 128 and reporting "about 127".
        let refined = if best > lag_min && best < lag_max {
            let (left, peak, right) = (scores[best - 1], scores[best], scores[best + 1]);
            let curvature = left - 2.0 * peak + right;
            if curvature.abs() > f64::EPSILON {
                best as f64 + (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
            } else {
                best as f64
            }
        } else {
            best as f64
        };

        let confidence = (acf[best] / acf[0]).clamp(0.0, 1.0) as f32;
        if confidence < MIN_CONFIDENCE {
            return None;
        }
        Some(Tempo {
            bpm: (60.0 * self.envelope_rate / refined) as f32,
            confidence,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic noise. `rand` is not a dependency and this does not need to be good — it
    /// only has to be broadband and the same on every run.
    struct Lcg(u32);

    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1 << 23) as f32 - 1.0
        }
    }

    /// A click track: a short decaying noise burst on every beat, over a quiet tone so the
    /// signal is never digital silence.
    ///
    /// `accents` scales alternate beats — 1.0 is an even pulse, 0.3 is the loud/soft pattern
    /// that fools a plain autocorrelation into reporting half the tempo.
    fn click_track(bpm: f64, seconds: f64, rate: u32, channels: u16, accents: f32) -> Vec<i16> {
        let mut noise = Lcg(0x2545_F491);
        let frames = (seconds * rate as f64) as usize;
        let period = 60.0 / bpm * rate as f64;
        let decay = 0.03 * rate as f64;

        let mut pcm = Vec::with_capacity(frames * channels as usize);
        for frame in 0..frames {
            let beat = frame as f64 / period;
            let since = (beat.fract() * period) as f32;
            let quiet = beat as usize % 2 == 1;

            let level = (-since / decay as f32).exp() * if quiet { accents } else { 1.0 };
            let tone = 0.05 * (frame as f32 * 0.01).sin();
            let sample = (noise.next() * level * 0.6 + tone).clamp(-1.0, 1.0);

            let quantised = (sample * 30_000.0) as i16;
            for _ in 0..channels {
                pcm.push(quantised);
            }
        }
        pcm
    }

    fn measure(bpm: f64, rate: u32, channels: u16, accents: f32) -> Tempo {
        let pcm = click_track(bpm, 25.0, rate, channels, accents);
        let mut tracker = TempoTracker::new(rate, channels);
        // In chunks, at a size that is not a multiple of the channel count or the hop, because
        // Media Foundation hands back whatever it feels like and a chunk boundary landing
        // mid-frame must not shift the interleaving.
        for chunk in pcm.chunks(4093) {
            tracker.push(chunk);
        }
        tracker.tempo().expect("a tempo from a literal click track")
    }

    /// The base case: if this cannot find the beat in a metronome, nothing else matters.
    #[test]
    fn finds_an_even_pulse() {
        for bpm in [70.0, 100.0, 128.0, 174.0] {
            let found = measure(bpm, 48_000, 2, 1.0);
            let error = (found.bpm as f64 - bpm).abs();
            assert!(
                error < bpm * 0.02,
                "{bpm} BPM click track read as {:.1} BPM",
                found.bpm
            );
        }
    }

    /// The reason the harmonic sum is there. Alternating loud and soft beats make the raw
    /// autocorrelation peak at *two* beats, so a detector without it confidently halves the
    /// tempo of a large fraction of real music.
    #[test]
    fn accented_beats_do_not_halve_the_tempo() {
        let found = measure(128.0, 48_000, 2, 0.3);
        assert!(
            (found.bpm - 128.0).abs() < 4.0,
            "loud/soft beats at 128 BPM read as {:.1} BPM — half-tempo error is at 64",
            found.bpm
        );
    }

    /// Media Foundation resamples to the output device's rate, which is 48 kHz far more often
    /// than it is Pandora's 44.1 kHz. A tracker that assumed either would be ~8.8% wrong on
    /// the other, which is small enough to look plausible and be wrong all day.
    #[test]
    fn rate_and_channel_count_do_not_change_the_answer() {
        for (rate, channels) in [(44_100, 2), (48_000, 2), (48_000, 1), (44_100, 6)] {
            let found = measure(120.0, rate, channels, 1.0);
            assert!(
                (found.bpm - 120.0).abs() < 3.0,
                "120 BPM at {rate} Hz / {channels}ch read as {:.1} BPM",
                found.bpm
            );
        }
    }

    /// Silence has no tempo. Reporting one would put a confident number under every track that
    /// failed to decode.
    #[test]
    fn silence_reports_nothing() {
        let mut tracker = TempoTracker::new(48_000, 2);
        tracker.push(&vec![0i16; 48_000 * 2 * 25]);
        assert_eq!(tracker.tempo(), None);
    }

    /// Nothing may be reported before there is enough audio to have measured it. The decoder
    /// runs only ~5 s ahead of playback, so an early answer could only be a guess.
    #[test]
    fn says_nothing_until_it_has_listened() {
        let pcm = click_track(120.0, 4.0, 48_000, 2, 1.0);
        let mut tracker = TempoTracker::new(48_000, 2);
        tracker.push(&pcm);
        assert_eq!(tracker.tempo(), None, "four seconds is not enough to know");
    }

    /// A steady pulse should read as strongly periodic; the confidence is shown to the user as
    /// a hedge on the number, so it has to mean something.
    #[test]
    fn confidence_is_high_for_a_metronome() {
        let found = measure(120.0, 48_000, 2, 1.0);
        assert!(
            found.confidence > 0.3,
            "a metronome scored only {:.2}",
            found.confidence
        );
    }
}
