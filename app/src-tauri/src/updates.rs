//! Updates that land in the gap between songs.
//!
//! The old flow downloaded and installed the moment you clicked, which cut whatever was
//! playing in half. Here the download happens invisibly in the background and the
//! *install* — which exits the process and relaunches it — is held until a track ends.
//!
//! The rule this implements: interrupting a running song is acceptable if we truly have
//! to, but always prefer waiting a couple of minutes for it to end. So the track boundary
//! is the normal path, and [`MAX_WAIT`] is a backstop for when no boundary ever arrives.
//!
//! Two properties of `tauri-plugin-updater` make this work, both read from its source
//! rather than assumed:
//!
//! - `Update::download` and `Update::install` are separate calls, and the **signature is
//!   verified inside `download`** — so staged bytes are already trusted and the trigger
//!   cannot fail verification at the worst possible moment.
//! - `Update` is `Clone` and a Tauri `Resource` (hence `Send + Sync`), so the handle can
//!   be held alongside the bytes. That matters: it means firing the install needs **no
//!   network at all**, which is the difference between "instant" and "one more round trip
//!   while the listener sits in silence".

// The flow is release-only — `spawn` is compiled out in debug so a dev build can never
// download a release over itself — so in a debug build everything here is legitimately
// unused. The decision logic below is still compiled and tested in both.
#![cfg_attr(debug_assertions, allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager};
use tauri_plugin_updater::Update;

/// How long to wait for a track to end before installing anyway.
///
/// Longer than almost any song, so in practice this only fires when something is wrong —
/// a playhead that has stopped advancing, say — rather than as a routine timeout.
const MAX_WAIT: Duration = Duration::from_secs(6 * 60);

/// Backstop tick. Coarse on purpose; the boundary path is what normally fires.
const BACKSTOP_TICK: Duration = Duration::from_secs(20);

/// How often to look for a new release once nothing is staged.
///
/// Was 4 hours, from when updates required a click and a late notice cost nothing. Now
/// that they install themselves in a gap between songs, a release can sit unnoticed for
/// that whole window for no reason — and the check is a single conditional GET.
const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);

struct Staged {
    version: String,
    bytes: Vec<u8>,
    update: Update,
    armed_at: Instant,
}

#[derive(Default)]
pub struct UpdateCtl {
    staged: Mutex<Option<Staged>>,
    /// Guards the hand-off: `install` exits the process, so a second caller getting
    /// through would launch a second installer.
    firing: AtomicBool,
}

impl UpdateCtl {
    /// The staged version, if any — for the UI badge.
    pub fn pending(&self) -> Option<String> {
        self.staged.lock().ok()?.as_ref().map(|s| s.version.clone())
    }

    fn waited(&self) -> Option<Duration> {
        Some(self.staged.lock().ok()?.as_ref()?.armed_at.elapsed())
    }
}

/// Check for an update and download it in the background.
///
/// Downloading here rather than at install time is the whole point: it takes the slowest
/// step off the critical path, so the install is near-instant once a track ends.
pub async fn stage(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_updater::UpdaterExt;

    if let Some(v) = app.state::<UpdateCtl>().pending() {
        return Ok(Some(v)); // already holding one
    }

    // Build the updater ourselves rather than using `app.updater()`, purely to attach
    // `on_before_exit`. The hook lives on the builder and is copied into the resulting
    // `Update`, so it has to be set here — before the handle is staged — not at install
    // time. Without it the process is simply `exit(0)`'d with the audio device still open,
    // and whatever the next track had already buffered gets cut off rather than stopped.
    let handle = app.clone();
    let updater = app
        .updater_builder()
        .on_before_exit(move || {
            if let Some(engine) = handle.state::<crate::native::NativeEngine>().try_engine() {
                engine.stop_audio();
            }
        })
        .build()
        .map_err(|e| e.to_string())?;

    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };

    let version = update.version.clone();
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;

    eprintln!(
        "[updater] staged v{version} ({} bytes) — installs at the next track boundary",
        bytes.len()
    );
    *app.state::<UpdateCtl>().staged.lock().unwrap() = Some(Staged {
        version: version.clone(),
        bytes,
        update,
        armed_at: Instant::now(),
    });

    // A notice for the badge, not a prompt.
    let _ = app.emit("app://update-staged", version.clone());
    Ok(Some(version))
}

/// Why an install was held back. Every hold is worth being able to explain afterwards —
/// "why didn't it update?" is otherwise unanswerable.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Hold {
    Disabled,
    NothingStaged,
    Exporting,
    Remote,
    Paused,
}

impl Hold {
    fn reason(&self) -> &'static str {
        match self {
            Hold::Disabled => "automatic updates are off",
            Hold::NothingStaged => "nothing staged",
            Hold::Exporting => "an export is running",
            Hold::Remote => "a network player owns playback",
            Hold::Paused => "playback is paused",
        }
    }
}

/// What the world looks like when we consider installing.
#[derive(Debug, Clone, Copy)]
struct Conditions {
    /// The Settings checkbox. Off means never install on our own initiative — but an
    /// explicit click still works, which is why `force` overrides it.
    auto: bool,
    staged: bool,
    exporting: bool,
    remote: bool,
    playing: bool,
}

/// The whole decision, as a pure function.
///
/// Deliberately separated from the Tauri plumbing: the rest of this module cannot run
/// outside a release build (see [`spawn`]), so this is the only part that can be tested
/// at all — and it is the part where a mistake means restarting the app at a bad moment.
fn decide(c: Conditions) -> Result<(), Hold> {
    if !c.auto {
        return Err(Hold::Disabled);
    }
    if !c.staged {
        return Err(Hold::NothingStaged);
    }
    // Restarting mid-export would discard a deliberately slow walk over the collection.
    if c.exporting {
        return Err(Hold::Exporting);
    }
    // A renderer owns playback; there is no local track boundary to ride, and restarting
    // would only drop the display.
    if c.remote {
        return Err(Hold::Remote);
    }
    // Nothing is being interrupted while paused — but the app comes back *playing*, and
    // starting music at someone who deliberately stopped it is worse than waiting.
    if !c.playing {
        return Err(Hold::Paused);
    }
    Ok(())
}

fn conditions(app: &tauri::AppHandle, playing: bool) -> Conditions {
    Conditions {
        auto: crate::settings::get(app).auto_update,
        staged: app.state::<UpdateCtl>().pending().is_some(),
        exporting: app.state::<crate::export::ExportCtl>().is_running(),
        remote: crate::remote_active(),
        playing,
    }
}

/// A track just ended — the moment we have been waiting for.
pub fn on_track_boundary(app: &tauri::AppHandle) {
    try_install(app, true, "track boundary", false);
}

/// Install the staged update right now, because the user asked.
///
/// Returns false if nothing was staged, so the caller can fall back to the
/// download-then-install path. An explicit click overrides the paused and remote guards —
/// those exist to avoid surprising someone, and this is not a surprise — but **not** the
/// export guard, which protects work in progress rather than the listener's comfort.
///
/// On success this does not return: the installer is launched and the process exits.
pub fn install_staged(app: &tauri::AppHandle) -> bool {
    if app.state::<UpdateCtl>().pending().is_none() {
        return false;
    }
    if app.state::<crate::export::ExportCtl>().is_running() {
        return false;
    }
    try_install(app, true, "user asked", true);
    // Reached only when the install failed, in which case the bytes were put back.
    true
}

/// `force` is an explicit user request: it waives the guards that exist purely to avoid
/// surprising the listener (paused, and a renderer owning playback), because a deliberate
/// click is not a surprise. It does **not** waive the export guard, which protects work in
/// progress rather than anyone's comfort.
fn try_install(app: &tauri::AppHandle, playing: bool, why: &str, force: bool) {
    let ctl = app.state::<UpdateCtl>();
    let mut c = conditions(app, playing);
    if force {
        c.auto = true;
        c.remote = false;
        c.playing = true;
    }
    if let Err(h) = decide(c) {
        // Silent when there is simply nothing to install, or every track end would log.
        if !matches!(h, Hold::NothingStaged | Hold::Disabled) {
            eprintln!("[updater] holding install ({why}): {}", h.reason());
        }
        return;
    }
    if ctl.firing.swap(true, Ordering::SeqCst) {
        return;
    }

    let Some(staged) = ctl.staged.lock().unwrap().take() else {
        ctl.firing.store(false, Ordering::SeqCst);
        return;
    };

    eprintln!("[updater] installing v{} at {why}", staged.version);
    let _ = app.emit("app://update-installing", staged.version.clone());
    // Let the UI paint the notice before the process exits under it.
    std::thread::sleep(Duration::from_millis(250));

    // No network here: the bytes are downloaded and verified, and the handle is held.
    // On success this never returns — the plugin launches the installer and exits(0).
    if let Err(e) = staged.update.install(&staged.bytes) {
        eprintln!("[updater] install failed: {e}");
        let _ = app.emit("app://update-failed", staged.version.clone());
        // Put it back so a later boundary can retry rather than losing the download.
        *ctl.staged.lock().unwrap() = Some(staged);
        ctl.firing.store(false, Ordering::SeqCst);
    }
}

/// Background loop: keep an update staged, and run the backstop.
///
/// The backstop only counts while playing, so a paused app is never restarted out from
/// under the listener — it updates once playback resumes, or on the next launch.
pub fn spawn(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Settle before the first check, as the previous startup check did.
        tokio::time::sleep(Duration::from_secs(10)).await;
        let mut since_check = CHECK_EVERY; // check immediately on the first pass

        loop {
            let auto = crate::settings::get(&app).auto_update;
            if auto && since_check >= CHECK_EVERY && app.state::<UpdateCtl>().pending().is_none() {
                since_check = Duration::ZERO;
                if let Err(e) = stage(&app).await {
                    eprintln!("[updater] check failed: {e}");
                }
            }

            tokio::time::sleep(BACKSTOP_TICK).await;
            since_check += BACKSTOP_TICK;

            // Only while playing: see the module note on never restarting a paused app.
            if auto && playing(&app).await {
                if let Some(waited) = app.state::<UpdateCtl>().waited() {
                    if waited >= MAX_WAIT {
                        // No boundary in six minutes of playback — something is stuck, and
                        // interrupting beats never updating.
                        try_install(&app, true, "backstop (no track boundary)", false);
                    }
                }
            }
        }
    });
}

/// Is local playback actually running? The engine owns the answer, so there is no need to
/// infer it from playhead motion the way the DOM-scraping era had to.
async fn playing(app: &tauri::AppHandle) -> bool {
    match app.state::<crate::native::NativeEngine>().engine().await {
        Ok(engine) => !engine.is_paused(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything lined up for an install: staged, playing, nothing else going on.
    fn ready() -> Conditions {
        Conditions {
            auto: true,
            staged: true,
            exporting: false,
            remote: false,
            playing: true,
        }
    }

    /// With the Settings checkbox off, nothing installs on its own — not even with an
    /// update already downloaded and a track ending.
    #[test]
    fn does_nothing_when_automatic_updates_are_off() {
        assert_eq!(
            decide(Conditions {
                auto: false,
                ..ready()
            }),
            Err(Hold::Disabled)
        );
    }

    #[test]
    fn installs_when_a_track_ends_with_an_update_staged() {
        assert_eq!(decide(ready()), Ok(()));
    }

    #[test]
    fn does_nothing_without_a_staged_update() {
        // The common case: this runs at every single track boundary.
        assert_eq!(
            decide(Conditions {
                staged: false,
                ..ready()
            }),
            Err(Hold::NothingStaged)
        );
    }

    /// Restarting partway through an export throws away a walk that is deliberately slow —
    /// potentially many minutes of it — and leaves no file behind.
    #[test]
    fn never_interrupts_an_export() {
        assert_eq!(
            decide(Conditions {
                exporting: true,
                ..ready()
            }),
            Err(Hold::Exporting)
        );
    }

    /// A WiiM owns playback: the local "track ended" is meaningless, and restarting would
    /// just drop the display the listener is watching.
    #[test]
    fn never_restarts_while_a_network_player_owns_playback() {
        assert_eq!(
            decide(Conditions {
                remote: true,
                ..ready()
            }),
            Err(Hold::Remote)
        );
    }

    /// The app always comes back playing, so restarting a paused app starts music at
    /// someone who deliberately stopped it. Accepted trade-off: an app left paused
    /// indefinitely updates on its next launch instead.
    #[test]
    fn never_restarts_a_paused_app() {
        assert_eq!(
            decide(Conditions {
                playing: false,
                ..ready()
            }),
            Err(Hold::Paused)
        );
    }

    /// Guard precedence: a paused, exporting, remote app reports the export, because that
    /// is the one a user would most want explained.
    #[test]
    fn reports_the_most_important_hold_first() {
        let c = Conditions {
            auto: true,
            staged: true,
            exporting: true,
            remote: true,
            playing: false,
        };
        assert_eq!(decide(c), Err(Hold::Exporting));
        assert_eq!(
            decide(Conditions {
                exporting: false,
                ..c
            }),
            Err(Hold::Remote)
        );
    }

    /// An explicit click waives paused/remote but never the export guard. Mirrors what
    /// `try_install(force: true)` builds, so the promise in its doc comment is checked.
    #[test]
    fn an_explicit_request_waives_only_the_courtesy_guards() {
        let forced = |c: Conditions| Conditions {
            auto: true,
            remote: false,
            playing: true,
            ..c
        };

        // Deliberately the worst case: the setting is off AND a renderer owns playback
        // AND it is paused. Automatically this does nothing; asked for, it goes.
        let paused_remote = Conditions {
            auto: false,
            staged: true,
            exporting: false,
            remote: true,
            playing: false,
        };
        assert_eq!(
            decide(paused_remote),
            Err(Hold::Disabled),
            "held automatically"
        );
        assert_eq!(
            decide(forced(paused_remote)),
            Ok(()),
            "but a click goes through"
        );

        let exporting = Conditions {
            exporting: true,
            ..paused_remote
        };
        assert_eq!(
            decide(forced(exporting)),
            Err(Hold::Exporting),
            "an export is never waived, even on request"
        );
    }

    /// "Wait a couple of minutes" — the backstop must be longer than a song, so it only
    /// fires when a boundary genuinely is not coming.
    #[test]
    fn backstop_is_longer_than_a_song() {
        assert!(
            MAX_WAIT >= Duration::from_secs(5 * 60),
            "would cut normal tracks short"
        );
        assert!(
            MAX_WAIT <= Duration::from_secs(10 * 60),
            "too long to be a backstop"
        );
        assert!(BACKSTOP_TICK < MAX_WAIT);
    }
}
