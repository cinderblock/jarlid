//! The last fraction of a second of what is playing, for a visualizer to look at.
//!
//! Written from the output callback, so writing never allocates, locks or waits: the ring is a
//! fixed array of atomics, and a write is a handful of relaxed stores. Reading is just as
//! unsynchronised, and may happen on any thread at any moment. A reader racing the writer can
//! at worst see a sample from one lap mixed with the next at the far end of the ring, which is
//! [`SCOPE_LEN`] − the read length frames away from where it reads — unreachable in the time a
//! copy takes, and harmless to a picture if it ever happened.
//!
//! Mono, before the volume gain: a visualizer shows the music, not where the volume knob is.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Frames kept. A power of two so the write position wraps with a mask, and comfortably more
/// than any analysis window plus the device's buffer, which reads lag behind.
pub const SCOPE_LEN: usize = 1 << 15;
const MASK: u64 = SCOPE_LEN as u64 - 1;

pub struct Scope {
    /// Mono samples as `f32` bits, in −1..1.
    samples: Box<[AtomicU32]>,
    /// Frames ever written. Never wraps in practice: at 192 kHz a `u64` lasts three million years.
    written: AtomicU64,
    /// Frames written but still in the device's own buffer, not yet out of the speaker. Reads
    /// end this far back, so the picture keeps time with what is heard.
    latency: AtomicU64,
    sample_rate: u32,
}

impl Scope {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            samples: (0..SCOPE_LEN).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            latency: AtomicU64::new(0),
            sample_rate,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Frames ever written. Unchanged between two reads means nothing played in between:
    /// paused, stopped, or between tracks.
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Append interleaved frames, in the mixer's 16-bit scale, as mono. A trailing partial
    /// frame is ignored. Allocation-free; safe in the output callback.
    pub fn push(&self, interleaved: &[f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let mut at = self.written.load(Ordering::Relaxed);
        let scale = 1.0 / (32768.0 * channels as f32);
        for frame in interleaved.chunks_exact(channels) {
            let mono = frame.iter().sum::<f32>() * scale;
            self.samples[(at & MASK) as usize].store(mono.to_bits(), Ordering::Relaxed);
            at += 1;
        }
        self.written.store(at, Ordering::Release);
    }

    /// How many frames have been written but not yet heard. Set from the output callback.
    pub fn set_latency(&self, frames: u64) {
        self.latency.store(frames, Ordering::Relaxed);
    }

    /// Fill `out` with the most recent audio the listener has heard, oldest first. Returns the
    /// write count it read at, for telling a fresh read from a repeat. Where less than `out` has
    /// ever been written, the front is silence.
    pub fn read_heard(&self, out: &mut [f32]) -> u64 {
        let written = self.written();
        let latency = self.latency.load(Ordering::Relaxed);
        // Never reach back further than the ring holds, whatever the device claims.
        let room = (SCOPE_LEN - out.len().min(SCOPE_LEN)) as u64;
        let end = written.saturating_sub(latency.min(room));
        let n = out.len() as u64;
        for (i, slot) in out.iter_mut().enumerate() {
            // `end - n + i`, as signed: before the first frame ever written is silence.
            let at = end as i128 - n as i128 + i as i128;
            *slot = if at < 0 {
                0.0
            } else {
                f32::from_bits(self.samples[(at as u64 & MASK) as usize].load(Ordering::Relaxed))
            };
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(frames: &[(f32, f32)]) -> Vec<f32> {
        frames.iter().flat_map(|&(l, r)| [l, r]).collect()
    }

    #[test]
    fn downmixes_and_normalises() {
        let scope = Scope::new(48_000);
        scope.push(&stereo(&[(32768.0, 0.0), (-16384.0, -16384.0)]), 2);
        let mut out = [0.0f32; 2];
        scope.read_heard(&mut out);
        assert_eq!(out, [0.5, -0.5]);
    }

    #[test]
    fn reads_the_newest_frames_in_order() {
        let scope = Scope::new(48_000);
        let frames: Vec<f32> = (0..10).map(|i| i as f32 * 512.0).collect();
        scope.push(&frames, 1);
        let mut out = [0.0f32; 3];
        assert_eq!(scope.read_heard(&mut out), 10);
        assert_eq!(out, [7.0 / 64.0, 8.0 / 64.0, 9.0 / 64.0]);
    }

    #[test]
    fn what_is_still_in_the_device_is_not_read() {
        let scope = Scope::new(48_000);
        let frames: Vec<f32> = (0..10).map(|i| i as f32 * 512.0).collect();
        scope.push(&frames, 1);
        scope.set_latency(4);
        let mut out = [0.0f32; 2];
        scope.read_heard(&mut out);
        assert_eq!(out, [4.0 / 64.0, 5.0 / 64.0]);
    }

    #[test]
    fn before_the_start_is_silence() {
        let scope = Scope::new(48_000);
        scope.push(&[32768.0, 32768.0], 1);
        let mut out = [9.0f32; 4];
        scope.read_heard(&mut out);
        assert_eq!(out, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn wraps_without_losing_order() {
        let scope = Scope::new(48_000);
        let lap: Vec<f32> = vec![0.0; SCOPE_LEN - 1];
        scope.push(&lap, 1);
        scope.push(&[3276.8, 6553.6, 9830.4], 1);
        let mut out = [0.0f32; 3];
        scope.read_heard(&mut out);
        for (got, want) in out.iter().zip([0.1, 0.2, 0.3]) {
            assert!((got - want).abs() < 1e-6, "{out:?}");
        }
    }

    #[test]
    fn a_partial_frame_is_ignored() {
        let scope = Scope::new(48_000);
        scope.push(&[32768.0, 32768.0, 32768.0], 2);
        assert_eq!(scope.written(), 1);
    }
}
