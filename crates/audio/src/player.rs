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

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use windows::core::w;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, GetCurrentThread,
    SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
};

use crate::{Decoder, Error, Format, Result};

/// Tells Windows this thread feeds an audio stream, for as long as the guard lives.
///
/// cpal already runs the device callback at `THREAD_PRIORITY_TIME_CRITICAL`, so the callback is
/// never the thread that loses a CPU race. The **decode** thread is the one that matters here: it
/// fills the ring buffer the callback drains, and at ordinary priority a busy machine — a big
/// compile, a render, a game — can deschedule it long enough for the buffer to run dry. The
/// callback then dutifully emits silence, which is heard as a dropout even though it never missed
/// a deadline of its own. Raising the *callback* would not have helped at all.
///
/// MMCSS is the sanctioned mechanism: the scheduler guarantees a registered thread a share of each
/// period rather than leaving it to compete on priority alone. The `Audio` task is the right one —
/// `Pro Audio` is for low-latency render threads, which this is not.
///
/// Deregistration must happen on the same thread, hence the guard rather than a bare call.
struct AudioPriority(Option<HANDLE>);

impl AudioPriority {
    fn raise() -> Self {
        unsafe {
            let mut index = 0u32;
            if let Ok(handle) = AvSetMmThreadCharacteristicsW(w!("Audio"), &mut index) {
                if !handle.is_invalid() {
                    return Self(Some(handle));
                }
            }
            // MMCSS can be disabled by policy, and the service can fail to start. A plain priority
            // bump is weaker — it buys no scheduling guarantee — but it beats running at the same
            // priority as the workload that is causing the trouble.
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
            Self(None)
        }
    }
}

impl Drop for AudioPriority {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            unsafe {
                let _ = AvRevertMmThreadCharacteristics(handle);
            }
        }
    }
}

/// How much audio to keep buffered: enough to ride out a network stall, short enough that
/// pause and skip feel immediate.
const TARGET_BUFFER: Duration = Duration::from_secs(5);

/// Which endpoint to play on.
///
/// `Default` is not resolved once and remembered — the owner re-checks it while playing and
/// rebuilds when Windows' default moves, because nothing else would notice. cpal binds an
/// endpoint when the stream is opened and a stream on a still-present device never errors, so
/// changing the default output mid-song leaves the music playing happily to the old speakers.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Output {
    /// Whatever Windows currently calls the default output, and keep following it.
    #[default]
    Default,
    /// One specific endpoint, by the name [`output_devices`] reports.
    Named(String),
}

/// Every output endpoint currently present, by name.
///
/// Names are the identity used throughout: cpal exposes no stable device id, and the name is
/// what a person picked from a list anyway. A renamed or absent device therefore reads as a
/// different one — which is why choosing a device that has gone away falls back rather than
/// failing (see [`resolve_device`]).
pub fn output_devices() -> Vec<String> {
    cpal::default_host()
        .output_devices()
        .map(|ds| ds.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// The name Windows currently gives the default output, if there is one.
pub fn default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| d.name().ok())
}

/// Find the endpoint an [`Output`] asks for.
///
/// A named device that is not present right now falls back to the default instead of
/// erroring. Unplugging the chosen DAC should cost you the *choice*, not the music — and the
/// setting is deliberately left pointing at the absent device so plugging it back in restores
/// it rather than silently rewriting what was asked for.
fn resolve_device(output: &Output) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Output::Named(want) = output {
        if let Ok(mut devices) = host.output_devices() {
            if let Some(found) =
                devices.find(|d| d.name().map(|have| &have == want).unwrap_or(false))
            {
                return Ok(found);
            }
        }
        eprintln!("output device {want:?} is not available; falling back to the system default");
    }
    host.default_output_device()
        .ok_or_else(|| Error::Unsupported("no audio output device".into()))
}

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
    /// Samples of silence the callback had to invent because the queue was dry. This is the
    /// dropout, measured at the only place it is unambiguous — nowhere else can distinguish
    /// "audio was late" from "audio was quiet".
    starved: AtomicU64,
    /// Set once the callback has filled a whole buffer, i.e. playback reached steady state. Before
    /// that an empty queue is just the pipeline priming, not a dropout.
    delivered: AtomicBool,
    /// Output gain as `f32` bits (1.0 = the decoded signal untouched).
    ///
    /// Stored as bits because there is no `AtomicF32`. It is the *target*, not what is
    /// currently being applied — the callback ramps towards it; see `callback!`.
    volume: AtomicU32,
}

/// Plays one track. Create a [`Player`] per track; the caller sequences them.
pub struct Player {
    shared: Arc<Shared>,
    format: Format,
    /// Where in the track this player started; zero unless it was built to resume a stalled one.
    started_at: Duration,
    /// The endpoint actually opened — not necessarily the one asked for, since a named device
    /// that has gone away falls back to the default. The owner compares this against the
    /// current default to notice when Windows moves it.
    device_name: String,
    // Dropping the stream stops the device, so it must outlive playback.
    _stream: cpal::Stream,
}

impl Player {
    /// Open `url` and begin playing immediately on the default output.
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
        Self::play_on(url, offset, &Output::Default)
    }

    /// As [`Player::play_at`], but on a chosen endpoint.
    ///
    /// The choice is honoured once, here. A [`Player`] never migrates between devices — the
    /// owner throws it away and builds another, which is the same disposable-player pattern
    /// every other recovery path uses.
    pub fn play_on(url: &str, offset: Duration, output: &Output) -> Result<Self> {
        let device = resolve_device(output)?;
        let device_name = device.name().unwrap_or_else(|_| "unknown".into());
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
            starved: AtomicU64::new(0),
            delivered: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            device_error: AtomicBool::new(false),
            volume: AtomicU32::new(1.0f32.to_bits()),
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
            // Held for the life of the thread: this is the thread whose starvation is audible.
            let _priority = AudioPriority::raise();

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

        // A volume change arrives as a step: the callback reads a new target between one buffer
        // and the next, and moving a waveform's amplitude discontinuously is heard as a click.
        // Ramping to the target with a one-pole filter over roughly 15 ms makes any change —
        // including a slider dragged as fast as the UI can send events — inaudible as anything
        // but a change in level. Derived from the device's own rate so the ramp is 15 ms of
        // sound rather than 15 ms at some assumed 48 kHz. The ramp advances once per *sample*
        // and the stream is interleaved, so the rate it is derived from has to include the
        // channel count — using the frame rate would make it 7.5 ms on stereo.
        const VOLUME_RAMP: f32 = 0.015;
        let samples_per_second = config.sample_rate().0 as f32 * config.channels() as f32;
        let gain_step = 1.0 - (-1.0 / (VOLUME_RAMP * samples_per_second)).exp();

        // Runs on the audio thread: no allocation, no locks, no blocking.
        macro_rules! callback {
            ($sample:ty, $convert:expr) => {{
                // The ramp's current value, owned by the callback and carried between calls.
                // Seeded at the target so opening a stream — including the rebuild after a stall
                // — starts at the right level instead of fading up from silence.
                let mut gain = f32::from_bits(callback_shared.volume.load(Ordering::Relaxed));
                move |output: &mut [$sample], _: &cpal::OutputCallbackInfo| {
                    let paused = callback_shared.paused.load(Ordering::Relaxed);
                    let target = f32::from_bits(callback_shared.volume.load(Ordering::Relaxed));

                    let mut written = 0usize;
                    if !paused {
                        for slot in output.iter_mut() {
                            let Ok(sample) = consumer.pop() else { break };
                            // A one-pole approaches its target without ever arriving; snap when
                            // the remainder is below a 16-bit LSB so that "muted" is really zero
                            // and the filter isn't left grinding on denormals forever.
                            gain += (target - gain) * gain_step;
                            if (target - gain).abs() < 1.0 / 65536.0 {
                                gain = target;
                            }
                            *slot = $convert(sample as f32 * gain);
                            written += 1;
                        }
                    }

                    // Anything unfilled must be explicit silence, or the device replays stale
                    // memory as a buzz.
                    for slot in output.iter_mut().skip(written) {
                        *slot = $convert(0.0f32);
                    }

                    // Silence we were forced to invent because the queue ran dry: the dropout,
                    // counted. Two exclusions, both cases where an empty queue is expected rather
                    // than a fault — the end of a track, and the priming gap before the first
                    // sample of one (which also covers the gap after a stall rebuild).
                    // A completely filled buffer is the definition of keeping up; from the first
                    // one onwards, any shortfall is a real dropout. Partial fills are the common
                    // case of one, so the two branches must not be exclusive on `written`.
                    let short = output.len() - written;
                    if short == 0 && written > 0 {
                        callback_shared.delivered.store(true, Ordering::Relaxed);
                    }
                    if short > 0
                        && !paused
                        && callback_shared.delivered.load(Ordering::Relaxed)
                        && !callback_shared.decoder_finished.load(Ordering::Relaxed)
                    {
                        callback_shared
                            .starved
                            .fetch_add(short as u64, Ordering::Relaxed);
                    }

                    callback_shared
                        .queued
                        .fetch_sub(written as u64, Ordering::Relaxed);
                    callback_shared
                        .frames_played
                        .fetch_add(written as u64 / channels, Ordering::Relaxed);
                }
            }};
        }

        // Attenuation happens in `f32`, before the sample is quantised to whatever the device
        // takes. On the usual Windows path — WASAPI shared mode, which is `f32` — that means
        // turning the volume down costs no resolution at all. Doing the multiply in the old
        // 16-bit fixed point instead threw away roughly a bit per halving, so a comfortable
        // listening level would have been played back as 11- or 12-bit audio.
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config.config(),
                callback!(f32, |s: f32| (s / 32768.0).clamp(-1.0, 1.0)),
                error_callback,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config.config(),
                callback!(i16, |s: f32| s.round().clamp(-32768.0, 32767.0) as i16),
                error_callback,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                &config.config(),
                callback!(
                    u16,
                    |s: f32| (s.round().clamp(-32768.0, 32767.0) as i32 + 32768) as u16
                ),
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
            device_name,
            _stream: stream,
        })
    }

    /// Where in the track this player actually began — the requested offset, or zero if the
    /// source refused to seek.
    pub fn started_at(&self) -> Duration {
        self.started_at
    }

    /// The endpoint this player is actually playing to.
    ///
    /// Worth asking rather than assuming: a named device that was missing at open time fell
    /// back to the default, and a player built as [`Output::Default`] is pinned to whatever
    /// was default *then*, which is exactly the thing that goes stale.
    pub fn device_name(&self) -> &str {
        &self.device_name
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

    /// 0.0 to 1.0, where 1.0 is the decoded signal untouched. Values above 1.0 are
    /// permitted but will clip.
    ///
    /// The change is a target, not an instruction to jump: the callback ramps to it over a
    /// few milliseconds, so this can be called as fast as a slider drag produces events
    /// without any of them being heard as a click. NaN reads as 0.0.
    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// The gain being aimed at, which is what was last set — not the ramp's current value.
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
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

    /// How much silence has been played because the decoder could not keep up.
    ///
    /// The honest measure of a dropout: counted in the callback, where "the queue was empty and I
    /// had to invent samples" is the literal definition. Should be zero. Anything non-zero on a
    /// healthy network means the decode thread is losing CPU — which is what the MMCSS
    /// registration on that thread exists to prevent.
    pub fn starved(&self) -> Duration {
        let samples = self.shared.starved.load(Ordering::Relaxed);
        let frames = samples / self.format.channels.max(1) as u64;
        Duration::from_secs_f64(frames as f64 / self.format.sample_rate.max(1) as f64)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The registration has to actually succeed, and it fails silently: a wrong task name, a
    /// missing `Win32_System_Threading` feature or a disabled MMCSS service all just return an
    /// error that the fallback swallows, leaving the decode thread at the priority it was already
    /// dropping audio at. Only a runtime check tells them apart.
    #[test]
    fn mmcss_registration_succeeds() {
        // On its own thread, exactly as the decode thread uses it — MMCSS is per-thread state.
        let registered = std::thread::spawn(|| AudioPriority::raise().0.is_some())
            .join()
            .expect("thread");
        assert!(
            registered,
            "MMCSS registration failed; the decode thread would silently fall back to a plain \
             priority bump and keep losing CPU under load"
        );
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Signal the decode thread to exit; otherwise it keeps pulling from the network after the
        // caller has moved on to the next track.
        self.shared.stopped.store(true, Ordering::Relaxed);
    }
}
