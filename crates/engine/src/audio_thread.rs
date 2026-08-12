//! Owns the audio device on a dedicated thread.
//!
//! `cpal::Stream` is **not `Send` on Windows**, so a [`audio::Player`] cannot be moved between
//! threads or held across an `.await`. Rather than fight that, one thread owns the player for its
//! whole life and everything else talks to it by message. State comes back through atomics, so
//! callers can poll position without blocking the audio path.
//!
//! This thread is also the **watchdog**. A [`audio::Player`] is a one-shot: it cannot repair a
//! dropped connection or a vanished output device, and when either happens it simply stops
//! producing sound while everything upstream still believes it is playing. So the current track's
//! URL is kept here, and the player is treated as disposable — rebuilt at the position the
//! listener had reached whenever it stalls, errors, or is released across a long pause.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the thread refreshes its published state. Fast enough for smooth lyric sync,
/// slow enough to be free.
const POLL: Duration = Duration::from_millis(50);

/// Decoding that has produced nothing new for this long has hung.
///
/// **Watch the decoder, not the listener.** Media Foundation reads synchronously with no timeout,
/// so a half-open socket blocks in `ReadSample` indefinitely — but the ring buffer keeps feeding
/// the device for seconds afterwards, so the *position* carries on moving as though nothing were
/// wrong and only freezes once the buffer is dry, i.e. once the silence has already started.
/// Decoded output stops the instant the read hangs, which buys the whole buffer's worth of time to
/// re-open before anyone hears a gap.
const DECODE_STALL: Duration = Duration::from_secs(3);

/// Buffer depth below which a hung decoder is worth acting on. Above this there is enough audio in
/// hand to ride out a slow read without doing anything drastic.
const LOW_BUFFER: Duration = Duration::from_secs(2);

/// Audio that is queued but not being consumed for this long means the device has stopped without
/// reporting an error.
const PLAYBACK_STALL: Duration = Duration::from_secs(4);

/// How long a pause may hold the network connection and the audio device open.
///
/// Short pauses keep the player so resuming is instant. Beyond this the connection is likely to
/// have been dropped by the far end anyway — and Pandora allows only one concurrent stream per
/// account, so idling on one for hours is rude as well as fragile.
const RELEASE_AFTER_PAUSE: Duration = Duration::from_secs(45);

/// Consecutive rebuild attempts before giving up on a track and letting the engine skip it. Each
/// attempt is one full re-open, so this bounds how long we can sit silent insisting.
const MAX_RECOVERIES: u32 = 3;

/// Playing for this long without further trouble clears the recovery count — otherwise three
/// stalls spread across an evening would retire a track that is simply on a flaky connection, and
/// a spell of heavy CPU load could cost a song rather than a few seconds of audio.
const RECOVERY_FORGIVENESS: Duration = Duration::from_secs(10);

enum Command {
    /// `paused: true` records the track without ever opening the device — see
    /// [`AudioThread::play_paused`].
    Play {
        url: String,
        paused: bool,
    },
    SetPaused(bool),
    SetVolume(f32),
    /// Move the playhead. Re-opens the stream at the new offset — see [`AudioThread::seek`].
    Seek(Duration),
    Stop,
    Shutdown,
}

#[derive(Default)]
struct Published {
    position_ms: AtomicU64,
    buffered_ms: AtomicU64,
    drift_ms: AtomicU64,
    /// Silence played because the decoder could not keep up. Accumulates across tracks, so it
    /// answers "is this machine dropping audio?" rather than "did this song".
    starved_ms: AtomicU64,
    /// Set when a track reaches its natural end, so the engine can advance. Cleared on the next
    /// `Play`; deliberately *not* set by `Stop`, which is a deliberate act rather than an ending.
    track_ended: AtomicBool,
    playing: AtomicBool,
    paused: AtomicBool,
    /// Set when a track could not be played and is not worth retrying, so the engine can skip it
    /// instead of stalling forever. Covers both "never opened" and "stalled and would not come
    /// back", which the engine treats the same way.
    failed: AtomicBool,
}

/// The track the thread is responsible for. Present whenever there is something to (re)build a
/// player from — so it outlives any individual player, and is cleared only when the track truly
/// ends, is stopped, or is given up on.
struct Current {
    url: String,
    /// Where a rebuilt player should pick up.
    resume_at: Duration,
}

/// Handle to the audio thread.
pub struct AudioThread {
    commands: Sender<Command>,
    published: Arc<Published>,
}

impl AudioThread {
    pub fn spawn() -> Self {
        let (commands, rx) = mpsc::channel::<Command>();
        let published = Arc::new(Published::default());
        let thread_state = Arc::clone(&published);

        std::thread::spawn(move || {
            // The player lives entirely on this thread; it is never sent anywhere.
            let mut player: Option<audio::Player> = None;
            let mut current: Option<Current> = None;

            // Volume is remembered here rather than only in the player, so it survives a rebuild.
            let mut volume = 1.0f32;
            let mut paused = false;
            let mut paused_since: Option<Instant> = None;

            // Stall detection, on both halves of the pipeline: what the decoder produces and what
            // the device consumes. Armed from player creation, not from first motion — a track
            // that never produces a single frame is exactly the case worth catching.
            let mut last_position = Duration::ZERO;
            let mut last_moved = Instant::now();
            let mut last_decoded = Duration::ZERO;
            let mut last_decode = Instant::now();
            // Per-player, so it resets with each rebuild; the published total does not.
            let mut last_starved = Duration::ZERO;
            let mut recoveries = 0u32;
            let mut recovered_at = Duration::ZERO;

            loop {
                // Drain commands first so pause and skip feel immediate.
                loop {
                    match rx.try_recv() {
                        Ok(Command::Play {
                            url,
                            paused: start_paused,
                        }) => {
                            thread_state.track_ended.store(false, Ordering::Relaxed);
                            thread_state.failed.store(false, Ordering::Relaxed);
                            thread_state.position_ms.store(0, Ordering::Relaxed);
                            paused = start_paused;
                            // Dated now, so a track queued paused and left that way still hits
                            // the release-after-pause path rather than looking freshly paused
                            // forever.
                            paused_since = start_paused.then(Instant::now);
                            recoveries = 0;
                            recovered_at = Duration::ZERO;
                            last_position = Duration::ZERO;
                            last_moved = Instant::now();
                            last_decoded = Duration::ZERO;
                            last_decode = Instant::now();
                            thread_state.paused.store(start_paused, Ordering::Relaxed);
                            player = None; // stop the old device before opening a new one
                            current = Some(Current {
                                url,
                                resume_at: Duration::ZERO,
                            });
                        }
                        Ok(Command::SetPaused(want)) => {
                            paused = want;
                            thread_state.paused.store(want, Ordering::Relaxed);
                            if let Some(player) = &player {
                                player.set_paused(want);
                            }
                            if want {
                                paused_since = Some(Instant::now());
                            } else {
                                paused_since = None;
                                // A pause is not a stall; don't let its duration count as one.
                                // Both clocks, or resuming instantly looks like a hung decoder.
                                last_moved = Instant::now();
                                last_decode = Instant::now();
                            }
                            // If the player was released during a long pause, the build step below
                            // brings it back — which is what makes the play button able to start
                            // audio again rather than merely clearing a flag.
                        }
                        Ok(Command::Seek(to)) => {
                            // A seek is the recovery path pointed somewhere deliberate: park the
                            // target in `resume_at` and drop the player, and the build step below
                            // re-opens there. Nothing here has to know how to seek a live decoder.
                            if let Some(track) = &mut current {
                                track.resume_at = to;
                            }
                            player = None;
                            // Publish the destination now. The rebuild takes a moment, and a
                            // playhead that keeps reporting the old position for a beat would drag
                            // the lyric highlight backwards before it jumped.
                            thread_state
                                .position_ms
                                .store(millis(to), Ordering::Relaxed);
                            last_position = to;
                            last_moved = Instant::now();
                            last_decoded = Duration::ZERO;
                            last_decode = Instant::now();
                            // Asking for a seek is not evidence the track is failing; a pass of
                            // timing work would otherwise spend the track's recovery budget.
                            recoveries = 0;
                            recovered_at = to;
                        }
                        Ok(Command::SetVolume(v)) => {
                            volume = v;
                            if let Some(player) = &player {
                                player.set_volume(v);
                            }
                        }
                        Ok(Command::Stop) => {
                            player = None; // dropping stops the device and the decode thread
                            current = None;
                            thread_state.playing.store(false, Ordering::Relaxed);
                        }
                        Ok(Command::Shutdown) => return,
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }

                // Build (or rebuild) whenever there is a track to play and nothing playing it.
                // Every recovery path funnels through here by clearing `player` and leaving
                // `current` in place.
                if !paused && player.is_none() {
                    if let Some(track) = &current {
                        match audio::Player::play_at(&track.url, track.resume_at) {
                            Ok(new_player) => {
                                new_player.set_volume(volume);
                                last_position = new_player.started_at();
                                last_moved = Instant::now();
                                last_decoded = new_player.decoded();
                                last_decode = Instant::now();
                                last_starved = Duration::ZERO;
                                recovered_at = new_player.started_at();
                                thread_state
                                    .position_ms
                                    .store(millis(new_player.started_at()), Ordering::Relaxed);
                                thread_state.playing.store(true, Ordering::Relaxed);
                                player = Some(new_player);
                            }
                            Err(e) => {
                                // Re-opening failed, which for a Pandora URL usually means the
                                // signed link has expired. Nothing to resume; let the engine move
                                // on rather than sit silent.
                                eprintln!("could not play track: {e}");
                                thread_state.failed.store(true, Ordering::Relaxed);
                                thread_state.playing.store(false, Ordering::Relaxed);
                                current = None;
                            }
                        }
                    }
                }

                if let Some(active) = &player {
                    let position = active.position();
                    thread_state
                        .position_ms
                        .store(millis(position), Ordering::Relaxed);
                    thread_state
                        .buffered_ms
                        .store(millis(active.buffered()), Ordering::Relaxed);
                    thread_state
                        .drift_ms
                        .store(millis(active.drift()), Ordering::Relaxed);

                    // Accumulated as a delta rather than folded in when a player is dropped, so
                    // the total survives every rebuild path without each one having to remember.
                    let starved = active.starved();
                    let fresh = starved.saturating_sub(last_starved);
                    if !fresh.is_zero() {
                        thread_state
                            .starved_ms
                            .fetch_add(millis(fresh), Ordering::Relaxed);
                    }
                    last_starved = starved;

                    if position != last_position {
                        last_position = position;
                        last_moved = Instant::now();
                        // Enough trouble-free playback to trust the stream again.
                        if position.saturating_sub(recovered_at) > RECOVERY_FORGIVENESS {
                            recoveries = 0;
                        }
                    }

                    let decoded = active.decoded();
                    if decoded != last_decoded {
                        last_decoded = decoded;
                        last_decode = Instant::now();
                    }

                    // Four ways a player dies without saying so out loud, all recoverable the same
                    // way: rebuild at the position actually reached. Only judged while playing — a
                    // paused player is *supposed* to look motionless, and its buffer is supposed to
                    // stay full.
                    let reason = if paused {
                        None
                    } else if active.device_error() {
                        Some("audio device failed")
                    } else if active.decode_error() && active.buffered().is_zero() {
                        // Whatever was already decoded has now been heard; the rest of the song
                        // is still out there.
                        Some("stream ended early")
                    } else if !active.end_of_stream()
                        && active.buffered() < LOW_BUFFER
                        && last_decode.elapsed() > DECODE_STALL
                    {
                        // Caught while there is still audio in the buffer, so re-opening usually
                        // costs the listener nothing at all. `end_of_stream` matters here: a
                        // decoder that has legitimately finished the track also stops producing
                        // and drains, and must not be mistaken for a hung one.
                        Some("decoding stalled")
                    } else if !active.buffered().is_zero() && last_moved.elapsed() > PLAYBACK_STALL {
                        // Audio queued and nobody consuming it: the device stopped without saying
                        // so. Distinct from the case above, where the *supply* is what dried up.
                        Some("playback stalled")
                    } else {
                        None
                    };

                    if active.is_finished() {
                        thread_state.track_ended.store(true, Ordering::Relaxed);
                        thread_state.playing.store(false, Ordering::Relaxed);
                        player = None;
                        current = None;
                    } else if let Some(reason) = reason {
                        recoveries += 1;
                        if recoveries > MAX_RECOVERIES {
                            eprintln!("{reason}; giving up on this track after {recoveries} tries");
                            thread_state.failed.store(true, Ordering::Relaxed);
                            thread_state.playing.store(false, Ordering::Relaxed);
                            current = None;
                        } else {
                            eprintln!(
                                "{reason} at {:.1}s; reopening (attempt {recoveries})",
                                position.as_secs_f64()
                            );
                            if let Some(track) = &mut current {
                                track.resume_at = position;
                            }
                        }
                        player = None; // rebuilt on the next pass, or dropped for good
                    } else if paused
                        && (active.device_error()
                            || paused_since
                                .map(|since| since.elapsed() > RELEASE_AFTER_PAUSE)
                                .unwrap_or(false))
                    {
                        // Let go of the socket and the device; resume from here on unpause. This
                        // is the ordinary long-pause path, not a fault, so it costs no recovery
                        // attempt — pausing all afternoon is allowed.
                        if let Some(track) = &mut current {
                            track.resume_at = position;
                        }
                        player = None;
                    }
                }

                std::thread::sleep(POLL);
            }
        });

        Self {
            commands,
            published,
        }
    }

    pub fn play(&self, url: &str) {
        let _ = self.commands.send(Command::Play {
            url: url.to_string(),
            paused: false,
        });
    }

    /// Load a track but do not start it — the device is never opened, so not one frame is
    /// emitted. Pressing play afterwards runs the same rebuild path a long pause does.
    ///
    /// This has to be part of the `Play` message rather than `play()` followed by
    /// `set_paused(true)`: those are two messages, and the thread's build step runs between
    /// drains, so a split would open the device and play a burst of audio before the pause
    /// landed. The listener would hear exactly the interruption this exists to avoid.
    pub fn play_paused(&self, url: &str) {
        let _ = self.commands.send(Command::Play {
            url: url.to_string(),
            paused: true,
        });
    }

    pub fn set_paused(&self, paused: bool) {
        let _ = self.commands.send(Command::SetPaused(paused));
    }

    pub fn set_volume(&self, volume: f32) {
        let _ = self.commands.send(Command::SetVolume(volume));
    }

    /// Move the playhead to `to`.
    ///
    /// There is no cheap way back: the ring buffer only holds audio *ahead* of the listener, so
    /// going backwards means re-opening the source and re-buffering — the same cost as recovering
    /// from a stall, and the same code path. Callers should treat it as a deliberate, occasional
    /// act rather than something to do on every frame of a drag.
    ///
    /// Seeking while paused is honoured: the target is remembered and resuming starts there.
    pub fn seek(&self, to: Duration) {
        let _ = self.commands.send(Command::Seek(to));
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    pub fn position(&self) -> Duration {
        Duration::from_millis(self.published.position_ms.load(Ordering::Relaxed))
    }

    pub fn buffered(&self) -> Duration {
        Duration::from_millis(self.published.buffered_ms.load(Ordering::Relaxed))
    }

    /// Lost-audio detector; see `audio::Player::drift`. Should stay at zero.
    pub fn drift(&self) -> Duration {
        Duration::from_millis(self.published.drift_ms.load(Ordering::Relaxed))
    }

    /// Total silence played because the decoder could not keep up, across every track this
    /// session. Distinct from `drift`: drift is audio we *lost*, this is audio that arrived too
    /// late to play. Non-zero means the decode thread is losing CPU to something.
    pub fn starved(&self) -> Duration {
        Duration::from_millis(self.published.starved_ms.load(Ordering::Relaxed))
    }

    pub fn is_paused(&self) -> bool {
        self.published.paused.load(Ordering::Relaxed)
    }

    /// True once the current track has played to its end.
    pub fn track_ended(&self) -> bool {
        self.published.track_ended.load(Ordering::Relaxed)
    }

    /// True if the last track could not be played and recovery was exhausted — the engine should
    /// skip rather than wait.
    pub fn failed(&self) -> bool {
        self.published.failed.load(Ordering::Relaxed)
    }
}

fn millis(d: Duration) -> u64 {
    d.as_millis() as u64
}

impl Drop for AudioThread {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}
