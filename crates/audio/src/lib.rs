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

#[cfg(windows)]
pub use media_foundation::Decoder;
#[cfg(windows)]
pub use player::{default_output_name, output_devices, Output, Player};

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
