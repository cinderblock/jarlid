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

/// Playback that hasn't advanced by this much, while unpaused and not finished, is stalled.
///
/// Generous because it must never fire on an ordinary rebuffer: Media Foundation reads
/// synchronously with no timeout, so a half-open socket blocks in `ReadSample` indefinitely and
/// this timer is the only thing that will ever notice.
const STALL_AFTER: Duration = Duration::from_secs(8);

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
/// stalls spread across an evening would retire a track that is simply on a flaky connection.
const RECOVERY_FORGIVENESS: Duration = Duration::from_secs(30);

enum Command {
    Play(String),
    SetPaused(bool),
    SetVolume(f32),
    Stop,
    Shutdown,
}

#[derive(Default)]
struct Published {
    position_ms: AtomicU64,
    buffered_ms: AtomicU64,
    drift_ms: AtomicU64,
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

            // Stall detection. Armed from player creation, not from first motion — a track that
            // never produces a single frame is exactly the case worth catching.
            let mut last_position = Duration::ZERO;
            let mut last_moved = Instant::now();
            let mut recoveries = 0u32;
            let mut recovered_at = Duration::ZERO;

            loop {
                // Drain commands first so pause and skip feel immediate.
                loop {
                    match rx.try_recv() {
                        Ok(Command::Play(url)) => {
                            thread_state.track_ended.store(false, Ordering::Relaxed);
                            thread_state.failed.store(false, Ordering::Relaxed);
                            thread_state.position_ms.store(0, Ordering::Relaxed);
                            paused = false;
                            paused_since = None;
                            recoveries = 0;
                            recovered_at = Duration::ZERO;
                            last_position = Duration::ZERO;
                            last_moved = Instant::now();
                            thread_state.paused.store(false, Ordering::Relaxed);
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
                                last_moved = Instant::now();
                            }
                            // If the player was released during a long pause, the build step below
                            // brings it back — which is what makes the play button able to start
                            // audio again rather than merely clearing a flag.
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

                    if position != last_position {
                        last_position = position;
                        last_moved = Instant::now();
                        // Enough trouble-free playback to trust the stream again.
                        if position.saturating_sub(recovered_at) > RECOVERY_FORGIVENESS {
                            recoveries = 0;
                        }
                    }

                    // Three ways a player dies without saying so out loud, all recoverable the
                    // same way: rebuild at the position actually reached. Only judged while
                    // playing — a paused player is *supposed* to look motionless, and its buffer
                    // is supposed to stay full.
                    let reason = if paused {
                        None
                    } else if active.device_error() {
                        Some("audio device failed")
                    } else if active.decode_error() && active.buffered().is_zero() {
                        // Whatever was already decoded has now been heard; the rest of the song
                        // is still out there.
                        Some("stream ended early")
                    } else if last_moved.elapsed() > STALL_AFTER {
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
        let _ = self.commands.send(Command::Play(url.to_string()));
    }

    pub fn set_paused(&self, paused: bool) {
        let _ = self.commands.send(Command::SetPaused(paused));
    }

    pub fn set_volume(&self, volume: f32) {
        let _ = self.commands.send(Command::SetVolume(volume));
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
