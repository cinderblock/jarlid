//! Owns the audio device on a dedicated thread.
//!
//! `cpal::Stream` is **not `Send` on Windows**, so a [`audio::Player`] cannot be moved between
//! threads or held across an `.await`. Rather than fight that, one thread owns the player for its
//! whole life and everything else talks to it by message. State comes back through atomics, so
//! callers can poll position without blocking the audio path.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::Duration;

/// How often the thread refreshes its published state. Fast enough for smooth lyric sync,
/// slow enough to be free.
const POLL: Duration = Duration::from_millis(50);

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
    /// Set when a track failed to open, so the engine can skip it instead of stalling forever.
    failed: AtomicBool,
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

            loop {
                // Drain commands first so pause and skip feel immediate.
                loop {
                    match rx.try_recv() {
                        Ok(Command::Play(url)) => {
                            thread_state.track_ended.store(false, Ordering::Relaxed);
                            thread_state.failed.store(false, Ordering::Relaxed);
                            thread_state.position_ms.store(0, Ordering::Relaxed);
                            match audio::Player::play(&url) {
                                Ok(new_player) => {
                                    thread_state.playing.store(true, Ordering::Relaxed);
                                    thread_state.paused.store(false, Ordering::Relaxed);
                                    player = Some(new_player);
                                }
                                Err(e) => {
                                    eprintln!("could not play track: {e}");
                                    thread_state.failed.store(true, Ordering::Relaxed);
                                    thread_state.playing.store(false, Ordering::Relaxed);
                                    player = None;
                                }
                            }
                        }
                        Ok(Command::SetPaused(paused)) => {
                            if let Some(player) = &player {
                                player.set_paused(paused);
                                thread_state.paused.store(paused, Ordering::Relaxed);
                            }
                        }
                        Ok(Command::SetVolume(volume)) => {
                            if let Some(player) = &player {
                                player.set_volume(volume);
                            }
                        }
                        Ok(Command::Stop) => {
                            player = None; // dropping stops the device and the decode thread
                            thread_state.playing.store(false, Ordering::Relaxed);
                        }
                        Ok(Command::Shutdown) => return,
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }

                if let Some(current) = &player {
                    let millis = |d: Duration| d.as_millis() as u64;
                    thread_state
                        .position_ms
                        .store(millis(current.position()), Ordering::Relaxed);
                    thread_state
                        .buffered_ms
                        .store(millis(current.buffered()), Ordering::Relaxed);
                    thread_state
                        .drift_ms
                        .store(millis(current.drift()), Ordering::Relaxed);

                    if current.is_finished() {
                        thread_state.track_ended.store(true, Ordering::Relaxed);
                        thread_state.playing.store(false, Ordering::Relaxed);
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

    pub fn is_playing(&self) -> bool {
        self.published.playing.load(Ordering::Relaxed)
    }

    pub fn is_paused(&self) -> bool {
        self.published.paused.load(Ordering::Relaxed)
    }

    /// True once the current track has played to its end.
    pub fn track_ended(&self) -> bool {
        self.published.track_ended.load(Ordering::Relaxed)
    }

    /// True if the last track could not be opened — the engine should skip rather than wait.
    pub fn failed(&self) -> bool {
        self.published.failed.load(Ordering::Relaxed)
    }
}

impl Drop for AudioThread {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}
