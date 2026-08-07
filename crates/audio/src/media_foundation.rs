//! Media Foundation source reader configured to emit PCM.
//!
//! MF's `IMFSourceReader` does demux + decode in one object: point it at a container, ask for
//! `MFAudioFormat_PCM` output, and it inserts the AAC decoder (SBR and all) itself.

use std::sync::Once;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::{Error, Format, Result};

/// MF must be initialised once per process, and deliberately never shut down — `MFShutdown` while
/// another decoder is alive would invalidate it.
static INIT: Once = Once::new();

fn ensure_initialized() -> Result<()> {
    let mut error = None;
    INIT.call_once(|| unsafe {
        // A failure here usually means COM is already initialised in a different mode, which is
        // fine for our purposes — only MFStartup failing is fatal.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
            error = Some(e);
        }
    });
    match error {
        Some(e) => Err(Error::Windows(e)),
        None => Ok(()),
    }
}

/// Reads a container and yields decoded PCM.
pub struct Decoder {
    reader: IMFSourceReader,
    format: Format,
    finished: bool,
}

/// `MF_SOURCE_READER_FIRST_AUDIO_STREAM`, which windows-rs exposes as an enum value.
const FIRST_AUDIO_STREAM: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

impl Decoder {
    /// Open a local file (or any URL Media Foundation's scheme handlers accept).
    pub fn open(path: &str) -> Result<Self> {
        ensure_initialized()?;

        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let reader = unsafe { MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None)? };

        unsafe {
            // Decode audio only — selecting nothing else avoids spinning up video pipelines for
            // containers that carry cover art as a video stream.
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(FIRST_AUDIO_STREAM, true)?;

            // Asking for uncompressed PCM makes MF insert the right decoder for us.
            let target = MFCreateMediaType()?;
            target.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            target.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            reader
                .SetCurrentMediaType(FIRST_AUDIO_STREAM, None, &target)
                .map_err(|_| Error::NoAudioStream)?;
        }

        // Read back what MF actually negotiated — this is where SBR shows up, as an output rate
        // of 44100 rather than the 22050 of the AAC core.
        let format = unsafe {
            let actual = reader.GetCurrentMediaType(FIRST_AUDIO_STREAM)?;
            Format {
                sample_rate: actual.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?,
                channels: actual.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)? as u16,
                bits_per_sample: actual.GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE)? as u16,
            }
        };

        Ok(Self {
            reader,
            format,
            finished: false,
        })
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// Decode the next chunk of PCM. Returns `None` at end of stream.
    ///
    /// Chunk sizes are whatever MF hands back (typically a few thousand frames); callers should
    /// treat this as a stream, not fixed-size blocks.
    pub fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.finished {
            return Ok(None);
        }

        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;

            unsafe {
                self.reader.ReadSample(
                    FIRST_AUDIO_STREAM,
                    0,
                    None,
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )?;
            }

            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                self.finished = true;
                return Ok(None);
            }

            // A format change mid-stream would invalidate our reported format; surface it rather
            // than silently emitting mismatched PCM.
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                return Err(Error::Unsupported("stream changed format mid-playback".into()));
            }

            // MF can return no sample without ending the stream (e.g. a gap); keep reading.
            let Some(sample) = sample else { continue };

            let data = unsafe {
                let buffer = sample.ConvertToContiguousBuffer()?;
                let mut pointer = std::ptr::null_mut();
                let mut length = 0u32;
                buffer.Lock(&mut pointer, None, Some(&mut length))?;
                let data = std::slice::from_raw_parts(pointer, length as usize).to_vec();
                buffer.Unlock()?;
                data
            };

            if !data.is_empty() {
                return Ok(Some(data));
            }
        }
    }

    /// Decode everything. Convenience for probes and tests — real playback should stream.
    pub fn decode_all(&mut self) -> Result<Vec<u8>> {
        let mut pcm = Vec::new();
        while let Some(chunk) = self.next_chunk()? {
            pcm.extend_from_slice(&chunk);
        }
        Ok(pcm)
    }
}

// The source reader is safe to move between threads; MF guards its own internals.
unsafe impl Send for Decoder {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening a nonexistent file must fail cleanly rather than panicking or hanging.
    #[test]
    fn missing_file_errors() {
        assert!(Decoder::open("Z:\\definitely\\not\\here.m4a").is_err());
    }

    #[test]
    fn format_arithmetic() {
        let format = Format {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        assert_eq!(format.frame_size(), 4);
        // One second of 44.1 kHz stereo 16-bit is 176400 bytes.
        assert_eq!(format.duration_of(176_400).as_secs(), 1);
    }
}
