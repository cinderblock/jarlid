//! Streaming audio playback.
//!
//! A decode thread pulls PCM from [`Decoder`] into a lock-free ring buffer; the cpal output
//! callback drains it. The buffer decouples network jitter from the audio device, which must never
//! be left waiting — a starved callback emits silence, heard as a click.
//!
//! **The queue is deliberately lock-free.** An earlier version shared a `Mutex<VecDeque<i16>>` and
//! used `try_lock` in the callback; whenever the decode thread held the lock the callback gave up
//! and emitted a whole period of silence, producing continuous scratchiness. An audio callback
//! must never contend for a lock, so producer and consumer now share an SPSC queue and two
//! atomics.
//!
//! Playback position comes from **frames actually delivered to the device**, not from how much has
//! been decoded. Decoding runs seconds ahead of what the listener hears, so tracking decode
//! progress would run synced lyrics early.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::{Decoder, Error, Format, Result};

/// How much audio to keep buffered: enough to ride out a network stall, short enough that
/// pause and skip feel immediate.
const TARGET_BUFFER: Duration = Duration::from_secs(5);

struct Shared {
    frames_played: AtomicU64,
    /// Every sample ever handed to the queue. Exists as a correctness invariant:
    /// `decoded == position + buffered`. If decoding outruns that sum, samples are being lost —
    /// which is precisely the bug that once made a 169 s track play out in 15 s.
    total_decoded: AtomicU64,
    /// Samples currently in the queue. Maintained by both sides so the UI can show buffer health
    /// without touching the queue itself.
    queued: AtomicU64,
    decoder_finished: AtomicBool,
    /// Set when decoding stopped for a reason that is *not* the end of the track — a dropped
    /// connection, an expired URL. Kept apart from `decoder_finished` so the owner can resume the
    /// same song rather than skipping it.
    decode_error: AtomicBool,
    paused: AtomicBool,
    stopped: AtomicBool,
    /// Set when cpal reports the output stream has failed — typically the device being removed or
    /// the default endpoint changing under us. The callback stops running at that point, so
    /// nothing else would ever notice; the owner watches this and rebuilds.
    device_error: AtomicBool,
    /// Fixed-point volume (1024 = unity), keeping the callback free of float state.
    volume: AtomicU64,
}

/// Plays one track. Create a [`Player`] per track; the caller sequences them.
pub struct Player {
    shared: Arc<Shared>,
    format: Format,
    /// Where in the track this player started; zero unless it was built to resume a stalled one.
    started_at: Duration,
    // Dropping the stream stops the device, so it must outlive playback.
    _stream: cpal::Stream,
}

impl Player {
    /// Open `url` and begin playing immediately.
    pub fn play(url: &str) -> Result<Self> {
        Self::play_at(url, Duration::ZERO)
    }

    /// Open `url` and begin playing `offset` into it.
    ///
    /// This is the recovery path: when a stream stalls or the audio device disappears, the owner
    /// throws the old player away and builds a new one at the position the listener had reached,
    /// so a dead socket costs a rebuffer rather than the rest of the song.
    ///
    /// If the source refuses to seek, playback starts from the beginning and [`Player::started_at`]
    /// reports `0` — the position must describe what is actually being heard, or synced lyrics
    /// would run confidently wrong for the whole track.
    pub fn play_at(url: &str, offset: Duration) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| Error::Unsupported("no audio output device".into()))?;
        let config = device
            .default_output_config()
            .map_err(|e| Error::Unsupported(format!("no output config: {e}")))?;

        // Decode straight to the device's own format so Media Foundation does any resampling.
        // Pandora is 44.1 kHz and most Windows devices run at 48 kHz; converting here rather than
        // ignoring the difference is what keeps playback at the right pitch.
        let requested = Format {
            sample_rate: config.sample_rate().0,
            channels: config.channels(),
            bits_per_sample: 16,
        };

        let mut decoder = Decoder::open_at(url, Some(requested))?;
        let format = decoder.format();

        // A source that won't seek still plays — from the top. Say so rather than pretending.
        let started_at = if offset.is_zero() || decoder.seek(offset).is_err() {
            Duration::ZERO
        } else {
            offset
        };

        let capacity = format.sample_rate as usize
            * format.channels as usize
            * TARGET_BUFFER.as_secs() as usize;
        let (mut producer, mut consumer) = rtrb::RingBuffer::<i16>::new(capacity);

        // Seed both counters, not just the position: `drift()` is `decoded - position - buffered`,
        // so seeding the position alone would leave it saturated at zero and silently retire the
        // lost-audio detector for the rest of the track.
        let start_frames = (started_at.as_secs_f64() * format.sample_rate as f64) as u64;
        let start_samples = start_frames * format.channels as u64;

        let shared = Arc::new(Shared {
            frames_played: AtomicU64::new(start_frames),
            total_decoded: AtomicU64::new(start_samples),
            queued: AtomicU64::new(0),
            decoder_finished: AtomicBool::new(false),
            decode_error: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            device_error: AtomicBool::new(false),
            volume: AtomicU64::new(1024),
        });

        // Decode thread: keep the queue topped up, backing off when full so a track is streamed
        // rather than pulled wholly into memory.
        //
        // CRITICAL: decoded samples are never discarded. An earlier version dropped whatever did
        // not fit before fetching the next chunk, which both lost audio and — when the dropped
        // count was odd — permanently shifted left/right interleaving, garbling everything after
        // it. Anything that doesn't fit stays in `pending` until there is room.
        let decode_shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let mut pending: Vec<i16> = Vec::new();
            let mut cursor = 0usize;

            loop {
                if decode_shared.stopped.load(Ordering::Relaxed) {
                    return;
                }

                // Drain whatever is left over before decoding anything new.
                if cursor < pending.len() {
                    let mut pushed = 0u64;
                    while cursor < pending.len() {
                        if producer.push(pending[cursor]).is_err() {
                            break;
                        }
                        cursor += 1;
                        pushed += 1;
                    }
                    if pushed > 0 {
                        decode_shared.queued.fetch_add(pushed, Ordering::Relaxed);
                        decode_shared.total_decoded.fetch_add(pushed, Ordering::Relaxed);
                    }
                    if cursor < pending.len() {
                        // Still full: wait for the callback to consume, then resume mid-buffer.
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    continue;
                }

                pending.clear();
                cursor = 0;

                match decoder.next_chunk() {
                    Ok(Some(chunk)) => {
                        pending.extend(
                            chunk
                                .chunks_exact(2)
                                .map(|pair| i16::from_le_bytes([pair[0], pair[1]])),
                        );
                    }
                    // End of track: stop producing and let the queue drain so the ending isn't
                    // clipped.
                    Ok(None) => {
                        decode_shared.decoder_finished.store(true, Ordering::Relaxed);
                        return;
                    }
                    // A decode error is *not* an ending, and conflating the two silently ate the
                    // rest of the song: the queue drained, `is_finished()` went true, and the
                    // engine advanced as though the track had played out. Flagged separately so
                    // the owner can re-open and resume where the listener actually is.
                    Err(e) => {
                        eprintln!("decode stopped: {e}");
                        decode_shared.decode_error.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            }
        });

        let callback_shared = Arc::clone(&shared);
        let channels = config.channels() as u64;

        // A stream error means the callback stops being invoked — no more audio, and the queue
        // stops draining, so `is_finished()` would never fire either. Record it so the owner can
        // rebuild onto whatever device is current now; printing alone leaves playback dead.
        let error_shared = Arc::clone(&shared);
        let error_callback = move |e| {
            eprintln!("audio output error: {e}");
            error_shared.device_error.store(true, Ordering::Relaxed);
        };

        // Runs on the audio thread: no allocation, no locks, no blocking.
        macro_rules! callback {
            ($sample:ty, $convert:expr) => {
                move |output: &mut [$sample], _: &cpal::OutputCallbackInfo| {
                    let paused = callback_shared.paused.load(Ordering::Relaxed);
                    let volume = callback_shared.volume.load(Ordering::Relaxed) as i32;

                    let mut written = 0usize;
                    if !paused {
                        for slot in output.iter_mut() {
                            let Ok(sample) = consumer.pop() else { break };
                            let scaled = (sample as i32 * volume / 1024)
                                .clamp(i16::MIN as i32, i16::MAX as i32)
                                as i16;
                            *slot = $convert(scaled);
                            written += 1;
                        }
                    }

                    // Anything unfilled must be explicit silence, or the device replays stale
                    // memory as a buzz.
                    for slot in output.iter_mut().skip(written) {
                        *slot = $convert(0i16);
                    }

                    callback_shared
                        .queued
                        .fetch_sub(written as u64, Ordering::Relaxed);
                    callback_shared
                        .frames_played
                        .fetch_add(written as u64 / channels, Ordering::Relaxed);
                }
            };
        }

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config.config(),
                callback!(f32, |s: i16| s as f32 / 32768.0),
                error_callback,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config.config(),
                callback!(i16, |s: i16| s),
                error_callback,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                &config.config(),
                callback!(u16, |s: i16| (s as i32 + 32768) as u16),
                error_callback,
                None,
            ),
            other => {
                return Err(Error::Unsupported(format!(
                    "unsupported output sample format {other:?}"
                )))
            }
        }
        .map_err(|e| Error::Unsupported(format!("could not open output stream: {e}")))?;

        stream
            .play()
            .map_err(|e| Error::Unsupported(format!("could not start playback: {e}")))?;

        Ok(Self {
            shared,
            format,
            started_at,
            _stream: stream,
        })
    }

    /// Where in the track this player actually began — the requested offset, or zero if the
    /// source refused to seek.
    pub fn started_at(&self) -> Duration {
        self.started_at
    }

    /// True once the output device has failed. The player produces no more audio after this and
    /// cannot repair itself; build a new one at [`Player::position`].
    pub fn device_error(&self) -> bool {
        self.shared.device_error.load(Ordering::Relaxed)
    }

    /// How far into the track the listener actually is.
    ///
    /// Derived from frames delivered to the device, so it stays honest even though decoding runs
    /// well ahead — which is exactly what synced lyrics need.
    pub fn position(&self) -> Duration {
        let frames = self.shared.frames_played.load(Ordering::Relaxed);
        Duration::from_secs_f64(frames as f64 / self.format.sample_rate.max(1) as f64)
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// 0.0 to 1.0. Values above 1.0 are permitted but will clip.
    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store((volume.max(0.0) * 1024.0) as u64, Ordering::Relaxed);
    }

    /// True once the decoder finished *and* the queue has drained — i.e. the listener has actually
    /// heard the end, not merely that we stopped decoding.
    ///
    /// Deliberately false when decoding stopped because of an error; see [`Player::decode_error`].
    pub fn is_finished(&self) -> bool {
        self.shared.decoder_finished.load(Ordering::Relaxed)
            && self.shared.queued.load(Ordering::Relaxed) == 0
    }

    /// True if decoding stopped early — a dropped connection or an expired URL rather than the end
    /// of the song. Whatever was already queued still plays out; the owner should rebuild at
    /// [`Player::position`] to hear the rest.
    pub fn decode_error(&self) -> bool {
        self.shared.decode_error.load(Ordering::Relaxed)
    }

    /// True once the decoder has read the whole track, whether or not the listener has heard it
    /// all yet.
    ///
    /// The difference from [`Player::is_finished`] matters to a watchdog: a decoder that has
    /// legitimately reached the end also stops producing, and must not be mistaken for one that
    /// has hung.
    pub fn end_of_stream(&self) -> bool {
        self.shared.decoder_finished.load(Ordering::Relaxed)
    }

    /// Seconds of audio buffered ahead of the listener. Useful for spotting network stalls.
    pub fn buffered(&self) -> Duration {
        let samples = self.shared.queued.load(Ordering::Relaxed);
        let frames = samples / self.format.channels.max(1) as u64;
        Duration::from_secs_f64(frames as f64 / self.format.sample_rate.max(1) as f64)
    }

    /// Total audio handed to the queue since playback began.
    ///
    /// Invariant: `decoded() ~= position() + buffered()`. A growing gap means decoded samples are
    /// being dropped, which sounds like the track playing too fast and, if an odd number is lost,
    /// permanently swaps the stereo channels. See [`Player::drift`].
    pub fn decoded(&self) -> Duration {
        let samples = self.shared.total_decoded.load(Ordering::Relaxed);
        let frames = samples / self.format.channels.max(1) as u64;
        Duration::from_secs_f64(frames as f64 / self.format.sample_rate.max(1) as f64)
    }

    /// How far `decoded()` has run ahead of what has been played plus what is still queued.
    ///
    /// Should stay at essentially zero. Anything that grows over time is lost audio.
    pub fn drift(&self) -> Duration {
        self.decoded()
            .saturating_sub(self.position() + self.buffered())
    }

    pub fn format(&self) -> Format {
        self.format
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Signal the decode thread to exit; otherwise it keeps pulling from the network after the
        // caller has moved on to the next track.
        self.shared.stopped.store(true, Ordering::Relaxed);
    }
}
