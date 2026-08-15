//! Mixing several tracks into one output stream, each with its own gain and playback rate.
//!
//! This exists so two songs can overlap. Playing them as two separate output streams would be
//! easier, but two streams are not sample-synchronised — separate callbacks, separate clocks —
//! and the phase relationship between them drifts. That is tolerable for a dumb crossfade and
//! fatal for a beat-matched one, which is the whole point of the exercise.
//!
//! **Rate is here rather than in the decoder on purpose.** Media Foundation can resample, but only
//! to a ratio fixed when the stream is opened: `next_chunk` errors outright on a mid-stream format
//! change. So MF can offer one fixed pitch offset and never a glide. Reading the buffer at a
//! fractional, movable rate is what allows a tempo to be *moved* while it plays, which is what
//! matching two tracks needs.
//!
//! Deliberately free of `cpal`, `rtrb` and Media Foundation: everything here is arithmetic, it runs
//! in the output callback where a mistake is audible, and it should be testable without a sound
//! card. Sources arrive through the [`Pcm`] trait so the tests can feed it a slice.

/// Most channels we will mix. Windows endpoints are almost always stereo; 8 covers 7.1 without
/// making the per-frame arrays big enough to care about.
pub const MAX_CHANNELS: usize = 8;

/// A supply of interleaved 16-bit PCM.
///
/// `available` exists so a voice can refuse to consume a *partial* frame. Popping two of three
/// channels and coming back later would shift the interleaving permanently — the same class of
/// bug that once swapped left and right for the rest of a track.
pub trait Pcm {
    /// Samples that can be popped right now.
    fn available(&self) -> usize;
    fn pop(&mut self) -> Option<i16>;
}

/// How a gain ramp gets from one level to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    /// Straight line. Right for a volume change, wrong for a crossfade: two linear ramps
    /// crossing sum to a dip in the middle, heard as the mix losing energy just as both tracks
    /// are audible.
    Linear,
    /// Quarter-sine. Two of these in opposition hold constant *power* through the crossover,
    /// because `sin² + cos² = 1`, so the blend stays at an even level throughout.
    EqualPower,
}

impl Curve {
    /// The gain at linear progress `t` in 0..=1, going from `from` to `to`.
    ///
    /// Equal power interpolates the *square* rather than the amplitude, which is what makes it
    /// direction-agnostic. Shaping `t` and interpolating amplitude does not work: the classic
    /// `sin`/`cos` pair only holds its level if the fade-out gets the cosine, and a curve applied
    /// to `from + (to - from) * s` receives no direction to choose by. Doing it this way, a
    /// downward ramp is `√(1-t)` and an upward one `√t` automatically, and their squares sum to
    /// one at every point — which is the whole definition.
    fn gain(self, from: f32, to: f32, t: f32) -> f32 {
        match self {
            Curve::Linear => from + (to - from) * t,
            Curve::EqualPower => {
                let power = from * from + (to * to - from * from) * t;
                power.max(0.0).sqrt()
            }
        }
    }
}

/// One playing track, with a gain and a playback rate that can both be moved smoothly.
///
/// A voice owns only the *reading* — the decoding and buffering stay where they were. It is
/// driven from the output callback, so nothing here allocates, locks or blocks.
pub struct Voice {
    channels: usize,

    // --- gain ---
    gain: f32,
    gain_from: f32,
    gain_to: f32,
    /// Progress along the current ramp, 0..=1. At 1 the ramp is done and `gain == gain_to`.
    gain_t: f32,
    gain_step: f32,
    gain_curve: Curve,

    // --- rate ---
    /// Input frames consumed per output frame. 1.0 is native speed; 1.04 plays 4% fast and
    /// therefore 4% sharp, exactly as a turntable would.
    rate: f64,
    rate_from: f64,
    rate_to: f64,
    rate_t: f64,
    rate_step: f64,

    // --- fractional read position ---
    /// Where we are between `current` and `next`, in 0..1.
    frac: f64,
    current: [f32; MAX_CHANNELS],
    next: [f32; MAX_CHANNELS],
    primed: bool,

    /// Output frames that had to be invented because the source was dry.
    starved: u64,
    /// Input frames actually consumed. The honest clock for "how far into this track are we",
    /// since it counts what was read rather than what was asked for.
    frames_read: u64,
    /// Set once the source ran out and stayed out.
    drained: bool,
}

impl Voice {
    /// A voice at `gain`, playing at native rate.
    pub fn new(channels: usize, gain: f32) -> Self {
        Self {
            channels: channels.clamp(1, MAX_CHANNELS),
            gain,
            gain_from: gain,
            gain_to: gain,
            gain_t: 1.0,
            gain_step: 0.0,
            gain_curve: Curve::Linear,
            rate: 1.0,
            rate_from: 1.0,
            rate_to: 1.0,
            rate_t: 1.0,
            rate_step: 0.0,
            frac: 0.0,
            current: [0.0; MAX_CHANNELS],
            next: [0.0; MAX_CHANNELS],
            primed: false,
            starved: 0,
            frames_read: 0,
            drained: false,
        }
    }

    /// Move the gain to `to` over `frames`, following `curve`.
    ///
    /// `frames` of zero sets it immediately, which is what a mute wants.
    pub fn fade_to(&mut self, to: f32, frames: u64, curve: Curve) {
        self.gain_from = self.gain;
        self.gain_to = to;
        self.gain_curve = curve;
        if frames == 0 {
            self.gain = to;
            self.gain_t = 1.0;
            self.gain_step = 0.0;
        } else {
            self.gain_t = 0.0;
            self.gain_step = 1.0 / frames as f32;
        }
    }

    /// Move the playback rate to `to` over `frames`.
    ///
    /// Always worth spreading over a real span. A step change in rate is a step change in pitch,
    /// which is heard as a click or a lurch; a few seconds makes even a 6% correction
    /// imperceptible — under a cent per second.
    pub fn glide_rate_to(&mut self, to: f64, frames: u64) {
        self.rate_from = self.rate;
        self.rate_to = to.max(0.01);
        if frames == 0 {
            self.rate = self.rate_to;
            self.rate_t = 1.0;
            self.rate_step = 0.0;
        } else {
            self.rate_t = 0.0;
            self.rate_step = 1.0 / frames as f64;
        }
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// True once the gain ramp has arrived — how a crossfade knows a voice is finished with.
    pub fn faded(&self) -> bool {
        self.gain_t >= 1.0
    }

    /// Input frames consumed so far.
    pub fn frames_read(&self) -> u64 {
        self.frames_read
    }

    /// Output frames of silence invented because the source was dry.
    pub fn starved(&self) -> u64 {
        self.starved
    }

    /// True once the source stopped supplying frames.
    pub fn drained(&self) -> bool {
        self.drained
    }

    /// Render `out` frames from `source`, **adding** into `out` so voices sum.
    ///
    /// `out` is interleaved at this voice's channel count. Whatever the source cannot supply is
    /// left as it was rather than written as silence — a voice that has run dry must not erase
    /// another voice that is still playing.
    pub fn mix_into(&mut self, source: &mut impl Pcm, out: &mut [f32]) {
        let channels = self.channels;
        for frame in out.chunks_mut(channels) {
            if frame.len() < channels {
                break;
            }
            if !self.fill(source) {
                // Dry. Count it and stop; the frames left in `out` belong to whoever else is
                // playing, and inventing silence over them would be worse than doing nothing.
                self.starved += 1;
                continue;
            }

            let gain = self.advance_gain();
            let frac = self.frac as f32;
            for (channel, slot) in frame.iter_mut().enumerate() {
                let a = self.current[channel];
                let b = self.next[channel];
                *slot += (a + (b - a) * frac) * gain;
            }

            self.frac += self.rate;
            self.advance_rate();
        }
    }

    /// Make sure `current` and `next` straddle the read position. False if the source ran dry.
    fn fill(&mut self, source: &mut impl Pcm) -> bool {
        if !self.primed {
            if !self.pop_frame(source, true) || !self.pop_frame(source, false) {
                return false;
            }
            self.primed = true;
            self.frac = 0.0;
        }
        // A rate above 1.0 can step past more than one input frame per output frame.
        while self.frac >= 1.0 {
            self.current = self.next;
            if !self.pop_frame(source, false) {
                // Put the position back so no frame is skipped when audio arrives again.
                self.drained = true;
                return false;
            }
            self.frac -= 1.0;
        }
        true
    }

    /// Pop one whole frame into `current` or `next`. Never consumes a partial frame.
    fn pop_frame(&mut self, source: &mut impl Pcm, into_current: bool) -> bool {
        if source.available() < self.channels {
            return false;
        }
        for channel in 0..self.channels {
            let Some(sample) = source.pop() else {
                // `available` said otherwise; treat it as dry rather than shifting interleaving.
                return false;
            };
            let value = sample as f32;
            if into_current {
                self.current[channel] = value;
            } else {
                self.next[channel] = value;
            }
        }
        self.frames_read += 1;
        self.drained = false;
        true
    }

    fn advance_gain(&mut self) -> f32 {
        if self.gain_t < 1.0 {
            self.gain_t = (self.gain_t + self.gain_step).min(1.0);
            self.gain = self
                .gain_curve
                .gain(self.gain_from, self.gain_to, self.gain_t);
        }
        self.gain
    }

    fn advance_rate(&mut self) {
        if self.rate_t < 1.0 {
            self.rate_t = (self.rate_t + self.rate_step).min(1.0);
            self.rate = self.rate_from + (self.rate_to - self.rate_from) * self.rate_t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slice pretending to be a ring buffer.
    struct Slice {
        data: Vec<i16>,
        at: usize,
    }

    impl Slice {
        fn new(data: Vec<i16>) -> Self {
            Self { data, at: 0 }
        }
    }

    impl Pcm for Slice {
        fn available(&self) -> usize {
            self.data.len() - self.at
        }
        fn pop(&mut self) -> Option<i16> {
            let v = self.data.get(self.at).copied()?;
            self.at += 1;
            Some(v)
        }
    }

    /// A ramp, so any resampling error shows up as a deviation from a straight line.
    fn ramp(frames: usize, channels: usize) -> Vec<i16> {
        (0..frames)
            .flat_map(|f| (0..channels).map(move |_| f as i16))
            .collect()
    }

    /// The one that matters most. At native rate with unity gain the mixer must be a wire —
    /// every frame out equals the frame in, in order, with no interpolation smear. If this is
    /// wrong then ordinary single-track playback, which is 99.9% of listening, is degraded to
    /// buy a feature that only runs for eight seconds between songs.
    #[test]
    fn native_rate_is_a_wire() {
        let mut source = Slice::new(ramp(64, 2));
        let mut voice = Voice::new(2, 1.0);
        let mut out = vec![0.0f32; 40 * 2];
        voice.mix_into(&mut source, &mut out);

        for (frame, chunk) in out.chunks(2).enumerate() {
            assert_eq!(chunk[0], frame as f32, "frame {frame} left");
            assert_eq!(chunk[1], frame as f32, "frame {frame} right");
        }
    }

    /// Half rate consumes half as much input, which is what "play it slower" means.
    #[test]
    fn rate_scales_consumption() {
        for (rate, frames_out, expected_in) in
            [(0.5, 100u64, 50u64), (1.0, 100, 100), (2.0, 50, 100)]
        {
            let mut source = Slice::new(ramp(400, 2));
            let mut voice = Voice::new(2, 1.0);
            voice.glide_rate_to(rate, 0);
            let mut out = vec![0.0f32; frames_out as usize * 2];
            voice.mix_into(&mut source, &mut out);

            // Two frames are read to prime before any output, hence the slack.
            let read = voice.frames_read();
            assert!(
                read.abs_diff(expected_in) <= 3,
                "rate {rate}: read {read} input frames for {frames_out} output, wanted ~{expected_in}"
            );
        }
    }

    /// Interpolation has to actually interpolate: at half rate, a straight-line input must stay
    /// a straight line at half the slope, not a staircase.
    #[test]
    fn half_rate_interpolates_rather_than_repeating() {
        let mut source = Slice::new(ramp(200, 1));
        let mut voice = Voice::new(1, 1.0);
        voice.glide_rate_to(0.5, 0);
        let mut out = vec![0.0f32; 100];
        voice.mix_into(&mut source, &mut out);

        for (i, &value) in out.iter().enumerate().take(90) {
            let expected = i as f32 * 0.5;
            assert!(
                (value - expected).abs() < 0.01,
                "sample {i}: {value} should be {expected} on a half-rate ramp"
            );
        }
    }

    /// Two voices in opposition must hold their level through the crossover. Linear ramps dip in
    /// the middle to 0.5+0.5 of *amplitude* but only 0.707 of power, which is audible as the mix
    /// sagging exactly when both songs are playing.
    #[test]
    fn equal_power_crossfade_holds_its_level() {
        const FRAMES: usize = 1000;
        // Correlated full-scale input on both voices is the worst case for a dip.
        let mut a_src = Slice::new(vec![1000i16; FRAMES * 2]);
        let mut b_src = Slice::new(vec![1000i16; FRAMES * 2]);

        let mut a = Voice::new(1, 1.0);
        let mut b = Voice::new(1, 0.0);
        a.fade_to(0.0, FRAMES as u64, Curve::EqualPower);
        b.fade_to(1.0, FRAMES as u64, Curve::EqualPower);

        // Equal-power means the *powers* sum, so measure that rather than the amplitudes: two
        // correlated signals summing at cos/sin would otherwise read as a bulge, not a dip.
        for frame in 0..FRAMES {
            let mut one = [0.0f32; 1];
            let mut two = [0.0f32; 1];
            a.mix_into(&mut a_src, &mut one);
            b.mix_into(&mut b_src, &mut two);
            let power = one[0] * one[0] + two[0] * two[0];
            let want = 1000.0 * 1000.0;
            assert!(
                (power / want - 1.0).abs() < 0.01,
                "frame {frame}: power {power} drifted from {want}"
            );
        }
    }

    /// A dry voice must leave the buffer alone. If it wrote silence it would mute whichever
    /// track is still playing — a stall on the outgoing song would cut the incoming one too.
    #[test]
    fn a_dry_voice_does_not_erase_the_other() {
        let mut empty = Slice::new(Vec::new());
        let mut voice = Voice::new(2, 1.0);
        let mut out = vec![0.5f32; 16];
        voice.mix_into(&mut empty, &mut out);

        assert!(
            out.iter().all(|&s| s == 0.5),
            "a dry voice overwrote the mix"
        );
        assert_eq!(
            voice.starved(),
            8,
            "eight frames of starvation went uncounted"
        );
    }

    /// A source that stops mid-buffer must not shift the channel interleaving of what it did
    /// deliver — half a frame consumed would swap left and right for the rest of the track.
    #[test]
    fn a_partial_frame_is_never_consumed() {
        // Five samples of a stereo stream: two whole frames and one orphan.
        let mut source = Slice::new(vec![1, 2, 3, 4, 5]);
        let mut voice = Voice::new(2, 1.0);
        let mut out = vec![0.0f32; 20];
        voice.mix_into(&mut source, &mut out);

        assert_eq!(source.available(), 1, "the orphaned sample was consumed");
    }

    /// A rate glide must be gradual — that is the entire reason it exists rather than a setter.
    #[test]
    fn a_rate_glide_is_gradual() {
        let mut voice = Voice::new(1, 1.0);
        voice.glide_rate_to(1.06, 1000);
        let mut source = Slice::new(vec![0i16; 4000]);

        let mut previous = voice.rate();
        for _ in 0..1000 {
            let mut out = [0.0f32; 1];
            voice.mix_into(&mut source, &mut out);
            let step = (voice.rate() - previous).abs();
            assert!(step < 1e-3, "rate jumped by {step} in one frame");
            previous = voice.rate();
        }
        assert!((voice.rate() - 1.06).abs() < 1e-6, "glide never arrived");
    }

    /// Fading with zero frames is an immediate set, which is what muting wants.
    #[test]
    fn a_zero_length_fade_is_immediate() {
        let mut voice = Voice::new(2, 1.0);
        voice.fade_to(0.0, 0, Curve::Linear);
        assert_eq!(voice.gain(), 0.0);
        assert!(voice.faded());
    }
}
