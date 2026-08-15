//! HE-AAC decoding via Windows Media Foundation.
//!
//! Pandora serves HE-AAC (AAC-LC + SBR) at a 22.05 kHz core that SBR reconstructs to 44.1 kHz.
//! Symphonia, the obvious pure-Rust choice, implements neither SBR nor PS, so it would silently
//! decode the core layer alone — half the sample rate and no high band. Media Foundation handles
//! SBR natively, ships with Windows, and adds nothing to the installer.
//!
//! See `plans/pandora-native-client.md` for the measurements behind that decision.

#[cfg(windows)]
mod media_foundation;
#[cfg(windows)]
mod player;
// Pure DSP with no platform surface, so it is not gated: its tests are what stand between a
// plausible-looking BPM and a wrong one, and they should run wherever `cargo test` does.
mod tempo;
// Also ungated: it is arithmetic that runs in the output callback, where a mistake is audible
// and a sound card is the worst possible place to discover one.
mod mixer;

#[cfg(windows)]
pub use media_foundation::Decoder;
pub use mixer::{Curve, Pcm, Voice, MAX_CHANNELS};
#[cfg(windows)]
pub use player::{default_output_name, output_devices, Output, Player};
pub use tempo::{Tempo, TempoTracker};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[cfg(windows)]
    #[error("media foundation error: {0}")]
    Windows(#[from] windows::core::Error),

    #[error("no audio stream in the container")]
    NoAudioStream,

    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The PCM format a [`Decoder`] produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

/// What the container actually holds, before any of our decoding.
///
/// Read from the stream rather than from Pandora's `audioEncoding` label, which describes the
/// default `audioUrlMap` stream and is stale whenever a better spec was granted — we ask for
/// `HTTP_192_MP3,HTTP_128_MP3,HTTP_64_AACPLUS_ADTS` and usually get the 128 kbit/s MP3 while the
/// label still says `aacplus`. Measuring is the only honest way to say what is playing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Codec name from the stream's own subtype, e.g. `"MP3"` or `"AAC"`.
    pub codec: String,
    /// Nominal bitrate. Zero when the container declares none; an average for VBR.
    pub bitrate_kbps: u32,
    /// The source's own rate, which is not necessarily what we decode to — [`Format`] carries
    /// that. HE-AAC reports its 22.05 kHz core here while SBR reconstructs 44.1 kHz.
    pub sample_rate: u32,
    pub channels: u16,
}

impl Format {
    /// Bytes per frame (one sample across all channels).
    pub fn frame_size(&self) -> usize {
        (self.channels as usize) * (self.bits_per_sample as usize / 8)
    }

    /// How long `bytes` of this format will play for.
    pub fn duration_of(&self, bytes: usize) -> std::time::Duration {
        let frames = bytes / self.frame_size().max(1);
        std::time::Duration::from_secs_f64(frames as f64 / self.sample_rate.max(1) as f64)
    }
}
