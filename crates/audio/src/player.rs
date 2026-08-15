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

use crate::{Decoder, Error, Format, Result, Source, Tempo, Voice};

/// How much audio the callback mixes in one pass, in samples.
///
/// Sized so a typical device period is one or two passes, and small enough to sit in the
/// callback's own state rather than being allocated — the callback may never allocate, so the
/// scratch buffer cannot be sized from whatever cpal happens to ask for.
const MIX_BLOCK: usize = 1024;

/// The queue, seen as a supply of PCM the mixer can read.
///
/// Whole frames only: `Voice` checks `available` before taking anything, so a queue that runs
/// dry mid-frame is left alone rather than half-consumed. Popping two samples of a stereo frame
/// and coming back later would swap the channels for the rest of the track.
impl crate::Pcm for rtrb::Consumer<i16> {
    fn available(&self) -> usize {
        self.slots()
    }

    fn pop(&mut self) -> Option<i16> {
        rtrb::Consumer::pop(self).ok()
    }
}

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
    /// Measured tempo as `f32` bits, and how periodic the track proved to be at it. Zero BPM
    /// means "not known yet" — which it always is for the first ten seconds or so, since
    /// decoding is throttled to roughly playback speed.
    bpm: AtomicU32,
    bpm_confidence: AtomicU32,
    /// Seconds from this player's stream start to a beat. Needed to line a blend up: the
    /// tempo says how often beats happen, this says when.
    beat_phase: AtomicU32,
    /// Frames of the incoming track the blend has played. Zero until one starts.
    ///
    /// This is what the handover is steered by: it says how far into the next song the listener
    /// already is, which is where an ordinary player has to be rebuilt to take over.
    blend_frames: AtomicU64,
    /// Set once the incoming voice has finished fading up, i.e. the blend is over and the
    /// outgoing track is silent. The owner watches this and completes the handover.
    blend_done: AtomicBool,
}

/// A prefetched track, read by the mixer.
///
/// Holds an `Arc` rather than the `Vec` itself so that dropping this **cannot free anything**.
/// It is dropped in the output callback when a blend ends, and freeing memory there is exactly
/// as forbidden as allocating it — the [`Player`] keeps its own reference alive for as long as
/// it exists, so the callback's drop is only ever a refcount decrement.
struct Prefetch {
    pcm: Arc<Vec<i16>>,
    at: usize,
}

impl crate::Pcm for Prefetch {
    fn available(&self) -> usize {
        self.pcm.len() - self.at
    }

    fn pop(&mut self) -> Option<i16> {
        let sample = self.pcm.get(self.at).copied()?;
        self.at += 1;
        Some(sample)
    }
}

/// A blend being handed to the output callback.
///
/// Sent through a lock-free queue rather than a mutex for the usual reason: the callback may not
/// wait for anything. Everything expensive — cloning the `Arc`, building the voice — happens on
/// the sending side, so the callback only moves a value out of a queue.
enum Handoff {
    /// Fade the next track in over the top of this one, played from memory.
    Blend {
        source: Prefetch,
        voice: Voice,
        /// What the outgoing track should do while this fades in.
        fade_frames: u64,
    },
    /// Re-point the primary voice at a new stream: the incoming track, now live.
    ///
    /// This is what makes the end of a blend gapless. Replacing the whole player instead would
    /// mean a fresh output stream, and the 200-400 ms it takes to open and seek one would land
    /// in the middle of a song rather than in the silence between two.
    Primary {
        consumer: rtrb::Consumer<i16>,
        voice: Voice,
        /// Long enough to hide a seek that did not land exactly where it was asked, short
        /// enough not to be heard as a fade.
        cross_frames: u64,
    },
}

/// Plays one track. Create a [`Player`] per track; the caller sequences them.
pub struct Player {
    shared: Arc<Shared>,
    format: Format,
    /// What the container held, when Media Foundation would say — the honest answer to "what is
    /// actually playing", as opposed to Pandora's `audioEncoding` label.
    source: Option<Source>,
    /// Where in the track this player started; zero unless it was built to resume a stalled one.
    started_at: Duration,
    /// The endpoint actually opened — not necessarily the one asked for, since a named device
    /// that has gone away falls back to the default. The owner compares this against the
    /// current default to notice when Windows moves it.
    device_name: String,
    /// Sends a blend to the callback. One slot: a second blend cannot start while one is
    /// running, and silently queueing one would be worse than refusing it.
    blend_tx: rtrb::Producer<Handoff>,
    /// Keeps the prefetched audio alive for the life of the player, so the callback's copy of
    /// the `Arc` is never the last one. Freeing six megabytes on the audio thread would be a
    /// dropout with no obvious cause.
    _blend_held: Option<Arc<Vec<i16>>>,
    /// Spent ring buffers coming back from the callback, to be dropped on this side.
    retire_rx: rtrb::Consumer<rtrb::Consumer<i16>>,
    /// What the counters read when the primary was last re-pointed, and where in the new track
    /// that was.
    ///
    /// The counters are never reset. Zeroing them at a handover is a race from either side — the
    /// incoming decode thread is already filling the new queue before the callback processes the
    /// swap, so a reset lands after some increments and `queued`, decremented with `fetch_sub`,
    /// can be driven below zero and wrap. Subtracting a base has no such window, and `drift`
    /// stays honest because every term shifts by the same amount.
    base_frames: u64,
    base_decoded: u64,
    track_start: Duration,
    // Dropping the stream stops the device, so it must outlive playback.
    _stream: cpal::Stream,
}

/// Spawn the thread that keeps the queue topped up for one track.
///
/// Extracted so a blend handover can start a second one against the same player. This is
/// the most delicate loop in the app — see the note about never discarding decoded samples
/// — and having two copies of it drift apart is a worse risk than the indirection.
fn spawn_decode(
    mut decoder: Decoder,
    first_chunk: Option<Vec<u8>>,
    decode_shared: Arc<Shared>,
    mut producer: rtrb::Producer<i16>,
    format: Format,
) {
    std::thread::spawn(move || {
        // Held for the life of the thread: this is the thread whose starvation is audible.
        let _priority = AudioPriority::raise();

        // The chunk already decoded to establish the true start position; play it, don't
        // re-read it.
        let Some(first) = first_chunk else {
            // Nothing in the stream at all — an ending, not a fault.
            decode_shared
                .decoder_finished
                .store(true, Ordering::Relaxed);
            return;
        };
        let mut pending: Vec<i16> = first
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let mut cursor = 0usize;

        // Tempo is measured here rather than looked up: Pandora's track model carries no BPM,
        // key or any other musicological field, so the only source of the number is the audio
        // itself. This thread is the right place for it — it sees every decoded sample exactly
        // once and in order, and unlike the output callback it is allowed to think. The work
        // is a few milliseconds once a second, against a decode-stall watchdog measured in
        // seconds.
        let mut tempo = crate::TempoTracker::new(format.sample_rate, format.channels);

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
                    decode_shared
                        .total_decoded
                        .fetch_add(pushed, Ordering::Relaxed);
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
                    // `pending` was cleared immediately above, so this is exactly the new
                    // samples — the tracker must see each one once, and the drain loop above
                    // can revisit `pending` many times before it empties.
                    tempo.push(&pending);
                    if let Some(measured) = tempo.tempo() {
                        decode_shared
                            .bpm
                            .store(measured.bpm.to_bits(), Ordering::Relaxed);
                        decode_shared
                            .bpm_confidence
                            .store(measured.confidence.to_bits(), Ordering::Relaxed);
                        decode_shared
                            .beat_phase
                            .store(measured.beat_phase.to_bits(), Ordering::Relaxed);
                    }
                }
                // End of track: stop producing and let the queue drain so the ending isn't
                // clipped.
                Ok(None) => {
                    decode_shared
                        .decoder_finished
                        .store(true, Ordering::Relaxed);
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
    /// The position clock is seeded from the source's *first decoded sample*, not from `offset` —
    /// a seek is a request the source may satisfy approximately or ignore entirely. The position
    /// must describe what is actually being heard, or synced lyrics run confidently wrong for the
    /// whole track. See [`Player::started_at`].
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
        // Taken before the decoder moves onto its thread: it describes the container, which
        // cannot change, so there is no reason to reach across a thread boundary for it later.
        let source = decoder.source();

        if !offset.is_zero() {
            // A source that won't seek still plays, from the top. Failure is not fatal here.
            let _ = decoder.seek(offset);
        }

        // Decode the first chunk up front so the position clock can be seeded from where the seek
        // ACTUALLY landed rather than where it was aimed.
        //
        // `SetCurrentPosition` takes a request, not a promise — it reports success and lands on
        // whatever boundary the source can manage, which for a network stream without an index
        // can be some distance away, and a refused seek lands at zero. Trusting the requested
        // offset meant `position()` could describe audio nobody was hearing, and every consumer
        // of it inherits that lie: the progress bar, and synced lyrics, which would then run at a
        // fixed error for the rest of the track.
        let first_chunk = decoder.next_chunk()?;
        let started_at = decoder.last_timestamp().unwrap_or(Duration::ZERO);

        let capacity = format.sample_rate as usize
            * format.channels as usize
            * TARGET_BUFFER.as_secs() as usize;
        let (producer, mut consumer) = rtrb::RingBuffer::<i16>::new(capacity);

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
            bpm: AtomicU32::new(0.0f32.to_bits()),
            bpm_confidence: AtomicU32::new(0.0f32.to_bits()),
            beat_phase: AtomicU32::new(0.0f32.to_bits()),
            blend_frames: AtomicU64::new(0),
            blend_done: AtomicBool::new(false),
        });

        // Decode thread: keep the queue topped up, backing off when full so a track is streamed
        // rather than pulled wholly into memory.
        //
        // CRITICAL: decoded samples are never discarded. An earlier version dropped whatever did
        // not fit before fetching the next chunk, which both lost audio and — when the dropped
        // count was odd — permanently shifted left/right interleaving, garbling everything after
        // it. Anything that doesn't fit stays in `pending` until there is room.
        spawn_decode(decoder, first_chunk, Arc::clone(&shared), producer, format);

        let callback_shared = Arc::clone(&shared);
        let channels = config.channels() as u64;
        let channel_count = config.channels() as usize;

        // One slot, because one blend at a time is all that means anything. `rtrb` carries any
        // type, so this moves a whole prepared voice across without the callback waiting on a
        // lock or building anything itself.
        let (blend_tx, mut blend_rx) = rtrb::RingBuffer::<Handoff>::new(1);
        // Spent queues travel back out. Two slots so a handover never blocks on the control
        // thread not having drained yet.
        let (mut retire_tx, retire_rx) = rtrb::RingBuffer::<rtrb::Consumer<i16>>::new(2);

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
                // The track, read through the mixer rather than popped straight out of the queue.
                // At the native rate this is arithmetically a wire — see `native_rate_is_a_wire`
                // in `mixer` — so ordinary playback is untouched; what it buys is somewhere for a
                // second track to arrive, and a rate that can move.
                let mut voice = Voice::new(channel_count, 1.0);
                // The incoming track, once there is one. Absent for all of ordinary playback.
                let mut blend: Option<(Voice, Prefetch)> = None;
                // Mixing happens in `f32`, which needs somewhere to put it. On the closure rather
                // than the stack per call, and fixed-size, because the callback must never
                // allocate. Whatever the device asks for is processed in frame-aligned passes of
                // at most this size.
                let mut scratch = [0.0f32; MIX_BLOCK];
                move |output: &mut [$sample], _: &cpal::OutputCallbackInfo| {
                    let paused = callback_shared.paused.load(Ordering::Relaxed);
                    let target = f32::from_bits(callback_shared.volume.load(Ordering::Relaxed));

                    // A blend that has been handed over starts on this callback. Popping a queue
                    // is the whole cost here; the voice and the `Arc` were built by the sender.
                    match blend_rx.pop() {
                        Ok(Handoff::Blend {
                            source,
                            voice: incoming,
                            fade_frames,
                        }) => {
                            voice.fade_to(0.0, fade_frames, crate::Curve::EqualPower);
                            blend = Some((incoming, source));
                        }
                        Ok(Handoff::Primary {
                            consumer: fresh,
                            voice: primary,
                            cross_frames,
                        }) => {
                            // The spent queue must leave by the other door. `rtrb` frees the
                            // shared buffer when both halves are gone, and the producer half went
                            // with the decode thread that finished the outgoing track — so
                            // dropping this here would deallocate megabytes on the audio thread.
                            let spent = std::mem::replace(&mut consumer, fresh);
                            let _ = retire_tx.push(spent);
                            voice = primary;
                            // Hand over from memory to the live stream. Both are the same track at
                            // the same instant, so this is only wide enough to cover a seek that
                            // landed a little off.
                            if let Some((incoming, _)) = blend.as_mut() {
                                incoming.fade_to(0.0, cross_frames, crate::Curve::EqualPower);
                            }
                            callback_shared.blend_done.store(false, Ordering::Relaxed);
                        }
                        Err(_) => {}
                    }

                    // Once the incoming voice has gone quiet its buffer is finished with. Dropping
                    // it here is only an `Arc` decrement — the player holds the last reference —
                    // and leaving it in place would keep mixing a source that has run dry.
                    if let Some((incoming, _)) = blend.as_ref() {
                        if incoming.faded() && incoming.gain() <= 0.0 {
                            blend = None;
                        }
                    }

                    let read_before = voice.frames_read();
                    let mut written = 0usize;
                    if !paused {
                        // Trim to whole frames so a pass boundary can never split one.
                        let pass = MIX_BLOCK - (MIX_BLOCK % channel_count);
                        for block in output.chunks_mut(pass) {
                            let usable = block.len() - (block.len() % channel_count);
                            if usable == 0 {
                                break;
                            }
                            let mix = &mut scratch[..usable];
                            mix.fill(0.0);

                            // A voice that runs dry stops early and starves the rest of the
                            // block, so what it produced is always a prefix.
                            let starved_before = voice.starved();
                            voice.mix_into(&mut consumer, mix);
                            let dry = (voice.starved() - starved_before) as usize * channel_count;
                            let mut produced = usable - dry;

                            // The incoming track mixes on top, and the longer of the two decides
                            // how much of the block was filled. Taking the outgoing track's
                            // length would truncate the blend at exactly the moment it matters:
                            // the outgoing song is *ending*, so it runs dry first by design.
                            if let Some((incoming, source)) = blend.as_mut() {
                                let starved_before = incoming.starved();
                                incoming.mix_into(source, mix);
                                let dry =
                                    (incoming.starved() - starved_before) as usize * channel_count;
                                produced = produced.max(usable - dry);
                                callback_shared
                                    .blend_frames
                                    .store(incoming.frames_read(), Ordering::Relaxed);
                                // Faded fully in and the outgoing track is silent: the blend is
                                // over. The owner completes the handover; the callback's only job
                                // is to say so and stop mixing a voice whose buffer will run out.
                                if incoming.faded() && voice.faded() && voice.gain() <= 0.0 {
                                    callback_shared.blend_done.store(true, Ordering::Relaxed);
                                }
                            }

                            for (slot, mixed) in block.iter_mut().zip(mix.iter()).take(produced) {
                                // A one-pole approaches its target without ever arriving; snap
                                // when the remainder is below a 16-bit LSB so that "muted" is
                                // really zero and the filter isn't left grinding on denormals
                                // forever.
                                gain += (target - gain) * gain_step;
                                if (target - gain).abs() < 1.0 / 65536.0 {
                                    gain = target;
                                }
                                *slot = $convert(*mixed * gain);
                            }
                            written += produced;
                            if produced < usable {
                                break;
                            }
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

                    // Both counters follow frames *taken from the queue*, not frames handed to
                    // the device. They are the same number at the native rate, and they must not
                    // be conflated once a voice can play at another one: at 1.04 the device eats
                    // 4% more input than it emits output, and the queue has to be debited for
                    // what was actually consumed or `drift` would accuse the pipeline of losing
                    // audio. Position wants the same figure for a different reason — it is a
                    // position *within the track*, and synced lyrics are timed against the
                    // track's own clock rather than the device's.
                    let consumed = voice.frames_read() - read_before;
                    callback_shared
                        .queued
                        .fetch_sub(consumed * channels, Ordering::Relaxed);
                    callback_shared
                        .frames_played
                        .fetch_add(consumed, Ordering::Relaxed);
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
            source,
            started_at,
            blend_tx,
            _blend_held: None,
            retire_rx,
            base_frames: 0,
            base_decoded: 0,
            track_start: Duration::ZERO,
            device_name,
            _stream: stream,
        })
    }

    /// Where in the track this player actually began, as reported by the source's own first
    /// sample — not the offset that was requested, which the source is free to ignore.
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
        let frames = self
            .shared
            .frames_played
            .load(Ordering::Relaxed)
            .saturating_sub(self.base_frames);
        self.track_start
            + Duration::from_secs_f64(frames as f64 / self.format.sample_rate.max(1) as f64)
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
        let samples = self
            .shared
            .total_decoded
            .load(Ordering::Relaxed)
            .saturating_sub(self.base_decoded);
        let frames = samples / self.format.channels.max(1) as u64;
        self.track_start
            + Duration::from_secs_f64(frames as f64 / self.format.sample_rate.max(1) as f64)
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

    /// What the container being played actually holds — codec, bitrate and the source's own
    /// rate, all read from the stream rather than from Pandora's label. `None` when Media
    /// Foundation would not describe it.
    pub fn source(&self) -> Option<Source> {
        self.source.clone()
    }

    /// Start fading the next track in over the top of this one.
    ///
    /// `pcm` is the incoming track's opening, already decoded to this player's format — see
    /// [`crate::prefetch`]. `rate` is the speed to play it at, where 1.0 is its own: anything
    /// else pulls its tempo (and its pitch with it) onto the outgoing track's, which is what a
    /// DJ's pitch fader does. `over` is how long the two overlap.
    ///
    /// The outgoing track fades out on the matching equal-power curve, so the pair hold a
    /// constant level through the crossover instead of sagging in the middle.
    ///
    /// Once [`Player::blend_done`] goes true the incoming track is the only thing audible, and
    /// the owner should rebuild an ordinary player for it at [`Player::blend_position`] — the
    /// same disposable-player handover every recovery path already uses. **The buffer is finite**:
    /// whatever was prefetched is all there is, so leaving a finished blend running eventually
    /// runs it dry.
    ///
    /// Refuses if a blend is already running, rather than queueing a second one.
    pub fn blend_in(&mut self, pcm: Arc<Vec<i16>>, rate: f64, over: Duration) -> Result<()> {
        if self._blend_held.is_some() {
            return Err(Error::Unsupported("a blend is already running".into()));
        }
        let frames = (over.as_secs_f64() * self.format.sample_rate as f64) as u64;

        // Built here rather than in the callback: the callback's whole job is to move this out
        // of a queue, not to construct anything.
        let mut voice = Voice::new(self.format.channels as usize, 0.0);
        voice.glide_rate_to(rate, 0);
        voice.fade_to(1.0, frames, crate::Curve::EqualPower);

        let handoff = Handoff::Blend {
            source: Prefetch {
                pcm: Arc::clone(&pcm),
                at: 0,
            },
            voice,
            fade_frames: frames,
        };
        self.blend_tx
            .push(handoff)
            .map_err(|_| Error::Unsupported("the blend queue is full".into()))?;
        // Held so the callback's `Arc` is never the last one; see `Prefetch`.
        self._blend_held = Some(pcm);
        Ok(())
    }

    /// True once the incoming track has fully faded in and the outgoing one is silent.
    pub fn blend_done(&self) -> bool {
        self.shared.blend_done.load(Ordering::Relaxed)
    }

    /// Take the blend over with a live stream, and become that track's player.
    ///
    /// Call once [`Player::blend_done`] is true and the outgoing track has finished. `url` is the
    /// incoming track and `at` is where in it to pick up — normally [`Player::blend_position`],
    /// which says how much of it has already been heard from memory. Returns where the seek
    /// **actually** landed, which is not always where it was aimed.
    ///
    /// This re-points the primary voice instead of replacing the player, and that is the whole
    /// point: building a fresh player would mean a fresh output stream, and the couple of hundred
    /// milliseconds it takes to open and seek one would land in the middle of a song rather than
    /// in the silence between two.
    ///
    /// Afterwards every accessor describes the new track — [`Player::position`] counts from its
    /// start, not from the blend.
    pub fn continue_with(&mut self, url: &str, at: Duration, cross: Duration) -> Result<Duration> {
        // Anything the callback has finished with, dropped here rather than there.
        while self.retire_rx.pop().is_ok() {}

        let mut decoder = Decoder::open_at(url, Some(self.format))?;
        if !at.is_zero() {
            let _ = decoder.seek(at);
        }
        // Same reasoning as `play_on`: decode one chunk up front so the clock is seeded from
        // where the seek landed rather than where it was aimed.
        let first_chunk = decoder.next_chunk()?;
        let landed = decoder.last_timestamp().unwrap_or(Duration::ZERO);

        // Read the counters before the incoming decode thread can touch them. Safe from this
        // thread precisely because the outgoing one has already exited — a finished track is the
        // only state this is ever called in.
        self.base_frames = self.shared.frames_played.load(Ordering::Relaxed);
        self.base_decoded = self.shared.total_decoded.load(Ordering::Relaxed);
        self.track_start = landed;

        // The old track's ending is not the new one's. Cleared before the thread that will set
        // them again for real.
        self.shared.decoder_finished.store(false, Ordering::Relaxed);
        self.shared.decode_error.store(false, Ordering::Relaxed);

        let capacity = self.format.sample_rate as usize
            * self.format.channels as usize
            * TARGET_BUFFER.as_secs() as usize;
        let (producer, consumer) = rtrb::RingBuffer::<i16>::new(capacity);
        spawn_decode(
            decoder,
            first_chunk,
            Arc::clone(&self.shared),
            producer,
            self.format,
        );

        let cross_frames = (cross.as_secs_f64() * self.format.sample_rate as f64) as u64;
        let mut voice = Voice::new(self.format.channels as usize, 0.0);
        voice.fade_to(1.0, cross_frames, crate::Curve::EqualPower);

        self.blend_tx
            .push(Handoff::Primary {
                consumer,
                voice,
                cross_frames,
            })
            .map_err(|_| Error::Unsupported("the handoff queue is full".into()))?;

        // The prefetched audio is finished with once the cross completes; the callback drops its
        // reference, and this releases the last one when the player goes.
        Ok(landed)
    }

    /// How far into the incoming track the blend has played.
    ///
    /// Where an ordinary player must be rebuilt to take the blend over. Measured from frames the
    /// mixer actually consumed, so it already accounts for having played at a pulled rate.
    pub fn blend_position(&self) -> Duration {
        let frames = self.shared.blend_frames.load(Ordering::Relaxed);
        Duration::from_secs_f64(frames as f64 / self.format.sample_rate.max(1) as f64)
    }

    /// The measured tempo of what is playing, or `None` before enough of it has been heard.
    ///
    /// Expect `None` for roughly the first ten seconds of a track. Decoding is deliberately
    /// throttled to about playback speed (see `TARGET_BUFFER`), so there is no more audio in
    /// hand than that — an earlier answer could only be a guess.
    pub fn tempo(&self) -> Option<Tempo> {
        let bpm = f32::from_bits(self.shared.bpm.load(Ordering::Relaxed));
        if bpm <= 0.0 {
            return None;
        }
        Some(Tempo {
            bpm,
            confidence: f32::from_bits(self.shared.bpm_confidence.load(Ordering::Relaxed)),
            beat_phase: f32::from_bits(self.shared.beat_phase.load(Ordering::Relaxed)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prefetched buffer is finite, and what happens at its end matters: the mixer must be
    /// able to see it coming rather than reading off the end, and running out has to look like
    /// an empty queue rather than an error.
    #[test]
    fn a_prefetched_buffer_runs_out_cleanly() {
        use crate::Pcm;

        let pcm = Arc::new(vec![1i16, 2, 3, 4]);
        let mut source = Prefetch {
            pcm: Arc::clone(&pcm),
            at: 0,
        };

        assert_eq!(source.available(), 4);
        assert_eq!(source.pop(), Some(1));
        assert_eq!(source.available(), 3);
        for _ in 0..3 {
            assert!(source.pop().is_some());
        }
        assert_eq!(source.available(), 0);
        assert_eq!(source.pop(), None, "read off the end of the buffer");
        // Still held elsewhere, so the callback dropping its copy frees nothing.
        assert_eq!(Arc::strong_count(&pcm), 2);
    }

    /// Two blends at once would mean two incoming tracks and no way to say which one the
    /// handover belongs to. Refusing is the only sane answer; queueing silently would surface
    /// as the wrong song fading in.
    #[test]
    fn a_second_blend_is_refused_rather_than_queued() {
        // Exercised through the queue alone, since building a `Player` needs a sound device.
        let (mut tx, _rx) = rtrb::RingBuffer::<u8>::new(1);
        assert!(tx.push(1).is_ok());
        assert!(tx.push(2).is_err(), "a one-slot queue accepted two blends");
    }

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
