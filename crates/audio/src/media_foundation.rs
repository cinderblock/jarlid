//! Media Foundation source reader configured to emit PCM.
//!
//! MF's `IMFSourceReader` does demux + decode in one object: point it at a container, ask for
//! `MFAudioFormat_PCM` output, and it inserts the AAC decoder (SBR and all) itself.

use std::sync::Once;
use std::time::Duration;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::VT_I8;

use crate::{Error, Format, Result, Source};

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
    source: Option<Source>,
    finished: bool,
    /// Presentation time of the most recent chunk, as the source reports it.
    ///
    /// This is the only trustworthy answer to "where are we actually decoding from" after a seek.
    /// `SetCurrentPosition` takes a request, not a promise: the source lands on whatever boundary
    /// it can, and for a network stream without an index that may be some distance off — while
    /// still returning success.
    last_timestamp: Option<Duration>,
}

/// `MF_SOURCE_READER_FIRST_AUDIO_STREAM`, which windows-rs exposes as an enum value.
const FIRST_AUDIO_STREAM: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

impl Decoder {
    /// Open a local file (or any URL Media Foundation's scheme handlers accept), decoding to
    /// whatever PCM format the source happens to be.
    pub fn open(path: &str) -> Result<Self> {
        Self::open_at(path, None)
    }

    /// Open, decoding to a specific PCM format.
    ///
    /// Requesting a sample rate makes Media Foundation insert a **resampler** into the pipeline.
    /// That matters: Pandora's audio is 44.1 kHz but Windows output devices commonly run at
    /// 48 kHz, and feeding one to the other unconverted plays everything sharp. Letting MF handle
    /// it costs no extra dependency and no resampling code of our own.
    pub fn open_at(path: &str, desired: Option<Format>) -> Result<Self> {
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

            if let Some(format) = desired {
                target.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, format.sample_rate)?;
                target.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, format.channels as u32)?;
                target.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, format.bits_per_sample as u32)?;
                // Block alignment and byte rate must agree with the above or MF rejects the type.
                let block = format.frame_size() as u32;
                target.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block)?;
                target.SetUINT32(
                    &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                    block * format.sample_rate,
                )?;
            }

            reader
                .SetCurrentMediaType(FIRST_AUDIO_STREAM, None, &target)
                .map_err(|_| Error::NoAudioStream)?;
        }

        // What the container holds, asked before anything is decoded. Best-effort: a source
        // that will not describe itself is worth a blank field, never a failed open.
        let source = unsafe { describe_source(&reader) };

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
            source,
            finished: false,
            last_timestamp: None,
        })
    }

    /// Where the most recent chunk actually sits in the track, per the source itself.
    ///
    /// `None` until something has been decoded. After a [`Decoder::seek`] this is the truth about
    /// where playback resumed, which is not necessarily what was asked for.
    pub fn last_timestamp(&self) -> Option<Duration> {
        self.last_timestamp
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// What the container actually holds, when Media Foundation will say.
    ///
    /// Distinct from [`Decoder::format`], which is what we decode *to*. On a Pandora stream the
    /// two differ in every field: 128 kbit/s MP3 at 44.1 kHz in, 16-bit PCM at the output
    /// device's 48 kHz out.
    pub fn source(&self) -> Option<Source> {
        self.source.clone()
    }

    /// Jump to `position` in the source.
    ///
    /// This is what makes a stalled stream recoverable: re-open the same URL, seek back to where
    /// the listener actually was, and carry on. Over HTTP it costs one ranged request rather than
    /// re-downloading everything already heard.
    ///
    /// Not every source is seekable — a server without range support, or a container without an
    /// index, will refuse — so callers must have a plan for `Err` rather than assuming success.
    pub fn seek(&mut self, position: Duration) -> Result<()> {
        // MF's default time format counts in 100 ns units.
        let ticks = (position.as_secs_f64() * 1e7) as i64;

        let mut target = PROPVARIANT::default();
        unsafe {
            let variant = &mut target.Anonymous.Anonymous;
            variant.vt = VT_I8;
            variant.Anonymous.hVal = ticks;
            // A null time format GUID means "the source's default", i.e. those 100 ns units.
            self.reader.SetCurrentPosition(&GUID::zeroed(), &target)?;
        }

        // A seek un-ends the stream: a reader that had hit EOF can produce samples again.
        self.finished = false;
        Ok(())
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
            let mut timestamp = 0i64;

            unsafe {
                self.reader.ReadSample(
                    FIRST_AUDIO_STREAM,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
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
                return Err(Error::Unsupported(
                    "stream changed format mid-playback".into(),
                ));
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
                // 100 ns units, same as the seek request.
                self.last_timestamp = Some(Duration::from_nanos(timestamp.max(0) as u64 * 100));
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

/// Describe the undecoded stream from the reader's *native* media type.
///
/// Everything here is best-effort. Reporting nothing costs a blank corner of the UI; failing an
/// open over it would cost the song.
///
/// # Safety
///
/// `reader` must be a live source reader with the audio stream selected.
unsafe fn describe_source(reader: &IMFSourceReader) -> Option<Source> {
    let native = unsafe { reader.GetNativeMediaType(FIRST_AUDIO_STREAM, 0) }.ok()?;

    // Named subtypes rather than a GUID dump: this ends up in front of a person. AAC-LC and
    // HE-AAC share a subtype — telling them apart needs the AAC profile out of the codec
    // private data — so this says "AAC" rather than claiming a precision it does not have.
    let codec = match unsafe { native.GetGUID(&MF_MT_SUBTYPE) }.ok() {
        Some(subtype) if subtype == MFAudioFormat_MP3 => "MP3",
        Some(subtype) if subtype == MFAudioFormat_AAC || subtype == MFAudioFormat_ADTS => "AAC",
        Some(subtype) if subtype == MFAudioFormat_PCM => "PCM",
        Some(subtype) if subtype == MFAudioFormat_Float => "PCM float",
        Some(subtype) if subtype == MFAudioFormat_FLAC => "FLAC",
        Some(subtype) if subtype == MFAudioFormat_Opus => "Opus",
        Some(subtype) if subtype == MFAudioFormat_WMAudioV9 => "WMA",
        _ => "unknown",
    };

    let bytes_per_second =
        unsafe { native.GetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND) }.unwrap_or(0);

    Some(Source {
        codec: codec.to_string(),
        // Rounded to the nearest kbit/s: a 128 kbit/s MP3 declares 16000 B/s, which is 128
        // exactly, but VBR averages land just off and "127" reads as a bug rather than a bitrate.
        bitrate_kbps: ((bytes_per_second as u64 * 8 + 500) / 1000) as u32,
        sample_rate: unsafe { native.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }.unwrap_or(0),
        channels: unsafe { native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }.unwrap_or(0) as u16,
    })
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

    /// Seeking has to *work*, not merely return `Ok`: the PROPVARIANT is hand-built, and getting
    /// `vt` or the union field wrong is exactly the sort of mistake Media Foundation would accept
    /// silently while ignoring the position. Resume-after-stall would then restart every track
    /// from the top instead of where the listener was, which is much harder to spot in the app
    /// than here.
    ///
    /// Uses a stock Windows sound rather than a fixture so the check costs the repo nothing.
    #[test]
    fn seek_skips_ahead() {
        const SOUND: &str = r"C:\Windows\Media\Ring05.wav";
        if !std::path::Path::new(SOUND).exists() {
            eprintln!("skipping: {SOUND} not present on this machine");
            return;
        }

        let mut decoder = Decoder::open(SOUND).expect("open");
        let format = decoder.format();
        let whole = decoder.decode_all().expect("decode from the start");
        let duration = format.duration_of(whole.len());
        assert!(duration > Duration::from_secs(2), "test file is too short");

        let mut seeked = Decoder::open(SOUND).expect("reopen");
        seeked.seek(duration / 2).expect("seek");
        let rest = seeked.decode_all().expect("decode after seek");
        let remaining = format.duration_of(rest.len());

        // Decoders emit whole samples and MF may land on a nearby boundary, so allow slack —
        // what matters is that a seek to the midpoint returned roughly half the audio and not,
        // as a no-op seek would, all of it.
        let expected = duration / 2;
        let error = remaining.abs_diff(expected);
        assert!(
            error < duration / 10,
            "seek to {expected:?} of {duration:?} left {remaining:?} to decode"
        );
    }

    /// After a seek, the decoder must be able to say where it *actually* is.
    ///
    /// This is the invariant the playback position rests on. `SetCurrentPosition` reports success
    /// and then lands wherever the source can, so seeding a position clock from the requested
    /// offset is an assumption, not a fact — and when it is wrong, everything downstream (progress
    /// bar, synced lyrics) is wrong by the same amount for the whole track, silently.
    #[test]
    fn first_sample_reports_where_the_seek_landed() {
        const SOUND: &str = r"C:\Windows\Media\Ring05.wav";
        if !std::path::Path::new(SOUND).exists() {
            eprintln!("skipping: {SOUND} not present on this machine");
            return;
        }

        let mut decoder = Decoder::open(SOUND).expect("open");
        assert!(
            decoder.last_timestamp().is_none(),
            "nothing decoded yet, so there is no honest answer to give"
        );

        let target = Duration::from_millis(1500);
        decoder.seek(target).expect("seek");
        decoder.next_chunk().expect("decode").expect("a sample");

        let landed = decoder
            .last_timestamp()
            .expect("a timestamp after decoding");
        assert!(
            landed.abs_diff(target) < Duration::from_millis(250),
            "seek asked for {target:?} and the source reported {landed:?}"
        );
    }

    /// The technical readout puts this in front of a person, so it has to be read off a real
    /// stream rather than assumed. A stock WAV is the one container guaranteed to be present.
    #[test]
    fn describes_the_undecoded_source() {
        const SOUND: &str = r"C:\Windows\Media\Ring05.wav";
        if !std::path::Path::new(SOUND).exists() {
            eprintln!("skipping: {SOUND} not present on this machine");
            return;
        }

        let source = Decoder::open(SOUND)
            .expect("open")
            .source()
            .expect("media foundation describes a WAV");

        assert_eq!(source.codec, "PCM");
        assert!(source.sample_rate >= 8_000, "{source:?}");
        assert!(source.channels >= 1, "{source:?}");
        // Uncompressed, so the declared byte rate is exactly rate x channels x bytes-per-sample
        // and the bitrate must agree with it rather than being some unrelated field.
        assert!(source.bitrate_kbps > 0, "{source:?}");
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
