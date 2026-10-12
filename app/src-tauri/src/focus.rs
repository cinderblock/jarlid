//! Which source the transport controls drive: this computer, or the network renderer (WiiM).
//!
//! This is the one place that decides. The UI's screen and play button, the SMTC media keys
//! and the taskbar thumbnail buttons all follow it. It used to be decided twice — in the UI
//! and in a `remote://state` listener — and the listener only ran when the renderer's polled
//! state *changed*, which a paused speaker never does. Starting local playback then moved the
//! screen but left the media keys on the speaker.
//!
//! The sources feed it directly: the engine's playhead ticker and the renderer's poll loop.
//! Takeover is edge-triggered — whichever source *starts* playing takes focus (the renderer
//! only while local is not playing), the toggle chooses outright, and pausing never moves
//! it. A background source that keeps playing is shown in the UI's mini player instead of
//! grabbing the screen.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Emitter;

/// The local playhead counts as moving for this long after its position last changed. The
/// same threshold the UI and SMTC use for the play/pause icon.
const LOCAL_IDLE_AFTER: Duration = Duration::from_millis(1600);

struct Focus {
    remote: bool,
    /// Chosen while the renderer had no track (a cast that has not landed yet); applied as
    /// soon as it has one.
    pending_remote: bool,
    remote_track: bool,
    remote_playing: bool,
    local_moving: bool,
    /// NaN until the first playhead, which is a baseline rather than a move.
    local_pos: f64,
    local_moved_at: Option<Instant>,
}

impl Focus {
    const fn new() -> Self {
        Self {
            remote: false,
            pending_remote: false,
            remote_track: false,
            remote_playing: false,
            local_moving: false,
            local_pos: f64::NAN,
            local_moved_at: None,
        }
    }

    fn on_local_playhead(&mut self, position: f64, now: Instant) {
        if self.local_pos.is_nan() {
            self.local_pos = position;
            return;
        }
        if (position - self.local_pos).abs() > 0.05 {
            self.local_pos = position;
            self.local_moved_at = Some(now);
        }
        let moving = self
            .local_moved_at
            .is_some_and(|t| now.duration_since(t) < LOCAL_IDLE_AFTER);
        if moving && !self.local_moving {
            // Local playback started: that is a move back to this computer.
            self.remote = false;
            self.pending_remote = false;
        }
        self.local_moving = moving;
    }

    fn on_remote_state(&mut self, playing: bool, has_track: bool) {
        let started = playing && !self.remote_playing;
        self.remote_track = has_track;
        self.remote_playing = playing && has_track;
        if !has_track {
            self.remote = false;
            return;
        }
        if self.pending_remote || (started && !self.local_moving) {
            self.remote = true;
            self.pending_remote = false;
        }
    }

    fn choose(&mut self, remote: bool) {
        if remote && !self.remote_track {
            self.pending_remote = true;
            return;
        }
        self.remote = remote;
        self.pending_remote = false;
    }
}

static FOCUS: Mutex<Focus> = Mutex::new(Focus::new());

/// Does the network renderer have focus? Media keys and the taskbar buttons go to it if so.
pub fn is_remote() -> bool {
    FOCUS.lock().unwrap().remote
}

fn update(app: &tauri::AppHandle, f: impl FnOnce(&mut Focus)) {
    let (was, now) = {
        let mut focus = FOCUS.lock().unwrap();
        let was = focus.remote;
        f(&mut focus);
        (was, focus.remote)
    };
    if was != now {
        let _ = app.emit("remote://focus", serde_json::json!({ "remote": now }));
    }
}

/// Every engine playhead tick, paused or not.
pub fn local_playhead(app: &tauri::AppHandle, position: f64) {
    update(app, |f| f.on_local_playhead(position, Instant::now()));
}

/// Every renderer poll, and with `has_track: false` when it becomes unreachable.
pub fn remote_state(app: &tauri::AppHandle, playing: bool, has_track: bool) {
    update(app, |f| f.on_remote_state(playing, has_track));
}

/// The UI's source toggle, and a cast to the speaker.
pub fn choose(app: &tauri::AppHandle, remote: bool) {
    update(app, |f| f.choose(remote));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives a `Focus` through time the way the 250 ms playhead ticker would.
    struct Sim {
        f: Focus,
        now: Instant,
        pos: f64,
    }

    impl Sim {
        fn new() -> Self {
            let mut s = Sim {
                f: Focus::new(),
                now: Instant::now(),
                pos: 10.0,
            };
            s.f.on_local_playhead(s.pos, s.now); // baseline
            s
        }
        /// Local playing for `secs`.
        fn play_local(&mut self, secs: f64) {
            self.ticks(secs, 0.25);
        }
        /// Local paused for `secs`.
        fn idle_local(&mut self, secs: f64) {
            self.ticks(secs, 0.0);
        }
        fn ticks(&mut self, secs: f64, step: f64) {
            for _ in 0..(secs / 0.25) as usize {
                self.now += Duration::from_millis(250);
                self.pos += step;
                self.f.on_local_playhead(self.pos, self.now);
            }
        }
        fn remote(&mut self, playing: bool) {
            self.f.on_remote_state(playing, true);
        }
    }

    #[test]
    fn first_playhead_is_a_baseline_not_a_start() {
        let mut f = Focus::new();
        f.on_remote_state(true, true);
        assert!(f.remote);
        f.on_local_playhead(42.0, Instant::now());
        assert!(f.remote, "the first sample must not read as local starting");
    }

    #[test]
    fn speaker_already_playing_at_launch_takes_focus() {
        let mut s = Sim::new();
        s.idle_local(1.0);
        s.remote(true);
        assert!(s.f.remote);
    }

    /// The reported bug: a paused speaker kept the media keys after local playback started,
    /// because nothing re-evaluated while the speaker's state sat unchanged.
    #[test]
    fn local_starting_takes_focus_from_a_paused_speaker() {
        let mut s = Sim::new();
        s.remote(true);
        s.remote(false); // paused: stays on the speaker
        s.idle_local(5.0);
        assert!(s.f.remote, "pausing the speaker keeps it focused");
        s.play_local(1.0); // no renderer poll arrives — none would, nothing changed
        assert!(!s.f.remote);
    }

    #[test]
    fn pausing_local_never_hands_focus_to_the_speaker() {
        let mut s = Sim::new();
        s.remote(false); // paused speaker with a track
        s.play_local(5.0);
        s.idle_local(10.0);
        s.remote(false);
        assert!(!s.f.remote, "paused speaker");

        // A speaker that started while local was playing doesn't take over on local's pause.
        s.play_local(2.0);
        s.remote(true);
        assert!(!s.f.remote, "local was playing when the speaker started");
        s.idle_local(10.0);
        s.remote(true);
        assert!(!s.f.remote, "still playing is not starting");
    }

    #[test]
    fn speaker_starting_while_local_is_idle_takes_focus() {
        let mut s = Sim::new();
        s.play_local(5.0);
        s.idle_local(2.0);
        s.remote(true);
        assert!(s.f.remote);
        s.remote(false);
        assert!(s.f.remote, "a pause is not a move back to local");
    }

    #[test]
    fn toggle_back_to_local_holds_while_the_speaker_plays_on() {
        let mut s = Sim::new();
        s.remote(true);
        s.f.choose(false);
        s.remote(true);
        s.idle_local(10.0);
        assert!(!s.f.remote);
        // Paused and resumed from the WiiM app: that is a start, and local is idle.
        s.remote(false);
        s.remote(true);
        assert!(s.f.remote);
    }

    #[test]
    fn toggle_to_speaker_holds_while_local_plays_on() {
        let mut s = Sim::new();
        s.remote(false);
        s.play_local(2.0);
        s.f.choose(true);
        s.play_local(5.0);
        assert!(s.f.remote, "local still playing is not local starting");
    }

    #[test]
    fn speaker_losing_its_track_returns_focus() {
        let mut s = Sim::new();
        s.remote(true);
        s.f.on_remote_state(false, false);
        assert!(!s.f.remote);
        s.f.choose(true);
        assert!(!s.f.remote, "nothing to show");
    }

    #[test]
    fn a_cast_chosen_before_the_track_lands_applies_when_it_does() {
        let mut s = Sim::new();
        s.play_local(2.0);
        s.f.choose(true); // cast: the speaker has nothing yet
        assert!(!s.f.remote);
        s.f.on_remote_state(false, true); // loading, not yet "play"
        assert!(s.f.remote);
    }

    #[test]
    fn local_starting_cancels_a_pending_cast_choice() {
        let mut s = Sim::new();
        s.f.choose(true);
        s.idle_local(2.0);
        s.play_local(1.0);
        s.f.on_remote_state(false, true);
        assert!(!s.f.remote);
    }
}
