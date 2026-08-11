//! Updates that land in the gap between songs.
//!
//! The download happens invisibly in the background; the *install* — which exits the
//! process and relaunches it — is held until a track ends. The rule: interrupting a
//! running song is acceptable if we truly have to, but always prefer waiting a couple of
//! minutes for it to end. So a track boundary is the normal trigger and [`MAX_WAIT`] is a
//! backstop for when no boundary ever arrives.
//!
//! Two properties of `tauri-plugin-updater` make this work, both read from its source
//! rather than assumed:
//!
//! - `Update::download` and `Update::install` are separate calls, and the **signature is
//!   verified inside `download`** — so staged bytes are already trusted and the trigger
//!   cannot fail verification at the worst possible moment.
//! - `Update` is `Clone` and a Tauri `Resource` (hence `Send + Sync`), so the handle can
//!   be held alongside the bytes. Firing then needs **no network at all**, which is the
//!   difference between "instant" and "one more round trip while the listener waits".
//!
//! # The three states
//!
//! *known* → *staged* → *armed*. [`Policy`] decides how far along that chain a new version
//! travels on its own; the badge walks whatever is left, one click per step.

// The loop is release-only — `spawn` is compiled out in debug so a dev build can never
// download a release over itself — so parts of this are unused in a debug build. The
// decision logic is compiled and tested in both.
#![cfg_attr(debug_assertions, allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{Emitter, Manager};
use tauri_plugin_updater::Update;

use crate::settings::{self, Policy};

/// How long to wait for a track to end before installing anyway.
///
/// Longer than almost any song, so it only fires when something is wrong — a playhead that
/// has stopped advancing, say — rather than as a routine timeout.
const MAX_WAIT: Duration = Duration::from_secs(6 * 60);

/// How often the loop wakes to re-evaluate. Coarse on purpose.
const TICK: Duration = Duration::from_secs(20);

struct Staged {
    version: String,
    bytes: Vec<u8>,
    update: Update,
    staged_at: Instant,
}

#[derive(Default)]
pub struct UpdateCtl {
    /// A newer version exists. Set even when we have not downloaded it (`NotifyOnly`).
    available: Mutex<Option<String>>,
    staged: Mutex<Option<Staged>>,
    /// Staged *and* cleared to install at the next opportunity. Separate from `staged`
    /// because `ManualInstall` downloads ahead of time but waits to be asked.
    armed: AtomicBool,
    /// Guards the hand-off: `install` exits the process, so a second caller getting
    /// through would launch a second installer.
    firing: AtomicBool,
}

/// What the badge needs to render itself, and what the next click will do.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub available: Option<String>,
    pub staged: bool,
    pub armed: bool,
    pub policy: Policy,
}

impl UpdateCtl {
    pub fn available(&self) -> Option<String> {
        self.available.lock().ok()?.clone()
    }
    pub fn is_staged(&self) -> bool {
        self.staged.lock().map(|g| g.is_some()).unwrap_or(false)
    }
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }
    fn waited(&self) -> Option<Duration> {
        Some(self.staged.lock().ok()?.as_ref()?.staged_at.elapsed())
    }
}

fn status(app: &tauri::AppHandle) -> Status {
    let ctl = app.state::<UpdateCtl>();
    Status {
        available: ctl.available(),
        staged: ctl.is_staged(),
        armed: ctl.is_armed(),
        policy: settings::get(app).update_policy,
    }
}

fn publish(app: &tauri::AppHandle) {
    let _ = app.emit("app://update-status", status(app));
}

/// Look for a newer version. Records it as available; downloads nothing.
pub async fn check(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    let updater = build_updater(app)?;
    let found = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .map(|u| u.version.clone());
    *app.state::<UpdateCtl>().available.lock().unwrap() = found.clone();
    publish(app);
    Ok(found)
}

/// Download and verify, so installing later needs no network.
///
/// Arms it too when the policy says to. `Instant` additionally installs right away, which
/// is the one path that deliberately cuts a song in half.
pub async fn stage(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    if app.state::<UpdateCtl>().is_staged() {
        return Ok(app.state::<UpdateCtl>().available());
    }

    let updater = build_updater(app)?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        *app.state::<UpdateCtl>().available.lock().unwrap() = None;
        publish(app);
        return Ok(None);
    };

    let version = update.version.clone();
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;

    let policy = settings::get(app).update_policy;
    eprintln!(
        "[updater] staged v{version} ({} bytes), policy {policy:?}",
        bytes.len()
    );
    {
        let ctl = app.state::<UpdateCtl>();
        *ctl.available.lock().unwrap() = Some(version.clone());
        *ctl.staged.lock().unwrap() = Some(Staged {
            version: version.clone(),
            bytes,
            update,
            staged_at: Instant::now(),
        });
        ctl.armed
            .store(policy.arms_automatically(), Ordering::SeqCst);
    }
    publish(app);

    // "Instant" means exactly that: do not wait for a boundary.
    if policy == Policy::Instant {
        try_install(app, true, "instant policy", true);
    }
    Ok(Some(version))
}

/// Build an updater carrying the `on_before_exit` hook.
///
/// The hook lives on the builder and is copied into the resulting `Update`, so it has to
/// be attached here rather than at install time. Without it the process is `exit(0)`'d
/// with the audio device still open, cutting off whatever the next track had buffered
/// instead of stopping it.
fn build_updater(app: &tauri::AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    use tauri_plugin_updater::UpdaterExt;
    let handle = app.clone();
    app.updater_builder()
        .on_before_exit(move || {
            if let Some(engine) = handle.state::<crate::native::NativeEngine>().try_engine() {
                engine.stop_audio();
            }
        })
        .build()
        .map_err(|e| e.to_string())
}

/// Why an install was held back. Every hold is worth being able to explain afterwards —
/// "why didn't it update?" is otherwise unanswerable.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Hold {
    NothingStaged,
    NotArmed,
    Exporting,
    Remote,
    Paused,
}

impl Hold {
    fn reason(&self) -> &'static str {
        match self {
            Hold::NothingStaged => "nothing staged",
            Hold::NotArmed => "waiting to be asked",
            Hold::Exporting => "an export is running",
            Hold::Remote => "a network player owns playback",
            Hold::Paused => "playback is paused",
        }
    }
}

/// What the world looks like when we consider installing.
#[derive(Debug, Clone, Copy)]
struct Conditions {
    staged: bool,
    /// Cleared to install. Set automatically by `Instant`/`AfterSong`, by hand otherwise.
    armed: bool,
    exporting: bool,
    remote: bool,
    playing: bool,
}

/// The whole decision, as a pure function.
///
/// Deliberately separated from the Tauri plumbing: the rest of this module cannot run
/// outside a release build, so this is the only part that can be tested at all — and it is
/// the part where a mistake means restarting the app at a bad moment.
fn decide(c: Conditions) -> Result<(), Hold> {
    if !c.staged {
        return Err(Hold::NothingStaged);
    }
    if !c.armed {
        return Err(Hold::NotArmed);
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
    let ctl = app.state::<UpdateCtl>();
    Conditions {
        staged: ctl.is_staged(),
        armed: ctl.is_armed(),
        exporting: app.state::<crate::export::ExportCtl>().is_running(),
        remote: crate::remote_active(),
        playing,
    }
}

/// A track just ended — the moment we have been waiting for.
pub fn on_track_boundary(app: &tauri::AppHandle) {
    try_install(app, true, "track boundary", false);
}

/// `force` is an explicit request: it waives the guards that exist purely to avoid
/// surprising the listener (not armed, paused, a renderer owning playback), because a
/// deliberate request is not a surprise. It does **not** waive the export guard, which
/// protects work in progress rather than anyone's comfort.
fn try_install(app: &tauri::AppHandle, playing: bool, why: &str, force: bool) {
    let ctl = app.state::<UpdateCtl>();
    let mut c = conditions(app, playing);
    if force {
        c.armed = true;
        c.remote = false;
        c.playing = true;
    }
    if let Err(h) = decide(c) {
        // These two are the steady state at every track boundary; logging them would be
        // pure noise.
        if !matches!(h, Hold::NothingStaged | Hold::NotArmed) {
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
        publish(app);
    }
}

/// One click on the version badge, which walks the *known → staged → armed → now* ladder a
/// step at a time.
///
/// The same escalation covers every policy, which is why there is one command rather than
/// four: under `NotifyOnly` the first click downloads; under `ManualInstall` it schedules
/// for the end of this song and a second means now; under `AfterSong` the update is
/// already armed, so a click can only mean "don't wait".
#[tauri::command]
pub async fn update_action(app: tauri::AppHandle) -> Result<Status, String> {
    let staged = app.state::<UpdateCtl>().is_staged();
    let armed = app.state::<UpdateCtl>().is_armed();

    if staged && armed {
        // Already going to happen on its own — so this means now.
        try_install(&app, true, "user asked", true);
        return Ok(status(&app)); // only reached when the install failed
    }
    if staged {
        // Downloaded and waiting to be asked: schedule it for the end of this song.
        app.state::<UpdateCtl>().armed.store(true, Ordering::SeqCst);
        publish(&app);
        return Ok(status(&app));
    }
    if app.state::<UpdateCtl>().available().is_some() {
        stage(&app).await?;
        return Ok(status(&app));
    }

    check(&app).await?;
    // A check that finds something shouldn't need a second click to start a download the
    // policy would have done anyway.
    if app.state::<UpdateCtl>().available().is_some()
        && settings::get(&app).update_policy.downloads_automatically()
    {
        stage(&app).await?;
    }
    Ok(status(&app))
}

#[tauri::command]
pub fn update_status(app: tauri::AppHandle) -> Status {
    status(&app)
}

/// Local wall clock as `(hour, minute)`, for the daily schedule.
///
/// `now_local` refuses when it cannot determine the offset safely; UTC is a poor but
/// harmless fallback — the check simply happens at a different hour than asked.
fn local_hm() -> (u32, u32) {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    (now.hour() as u32, now.minute() as u32)
}

/// Background loop: check on the configured schedule, and run the install backstop.
pub fn spawn(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Settle before the first check.
        tokio::time::sleep(Duration::from_secs(10)).await;
        let mut next_check = Some(Instant::now());

        loop {
            let cfg = settings::get(&app);

            if next_check.is_some_and(|due| Instant::now() >= due) {
                let outcome = if cfg.update_policy.downloads_automatically() {
                    stage(&app).await.map(|v| v.is_some())
                } else {
                    check(&app).await.map(|v| v.is_some())
                };
                if let Err(e) = outcome {
                    eprintln!("[updater] check failed: {e}");
                }
                // Recomputed below; scheduling from *now* rather than from the due time
                // keeps a slow check from immediately being due again.
                next_check = None;
            }

            // Recomputed every pass so a settings change takes effect without a restart.
            if next_check.is_none() {
                let (h, m) = local_hm();
                next_check = cfg
                    .check_schedule
                    .minutes_until_next(h, m)
                    .map(|mins| Instant::now() + Duration::from_secs(mins as u64 * 60));
            }

            tokio::time::sleep(TICK).await;

            // Backstop: only while playing, so a paused app is never restarted out from
            // under the listener.
            if playing(&app).await {
                let ctl = app.state::<UpdateCtl>();
                if ctl.is_armed() && ctl.waited().is_some_and(|w| w >= MAX_WAIT) {
                    try_install(&app, true, "backstop (no track boundary)", false);
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

    /// Everything lined up for an install.
    fn ready() -> Conditions {
        Conditions {
            staged: true,
            armed: true,
            exporting: false,
            remote: false,
            playing: true,
        }
    }

    #[test]
    fn installs_when_a_track_ends_with_an_armed_update() {
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

    /// `ManualInstall` and `NotifyOnly` leave a staged update unarmed. It must sit there
    /// indefinitely rather than installing itself at the next boundary.
    #[test]
    fn a_staged_but_unarmed_update_waits_to_be_asked() {
        assert_eq!(
            decide(Conditions {
                armed: false,
                ..ready()
            }),
            Err(Hold::NotArmed)
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
    /// someone who deliberately stopped it.
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

    /// Guard precedence: report the one a user would most want explained.
    #[test]
    fn reports_the_most_important_hold_first() {
        let c = Conditions {
            staged: true,
            armed: true,
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

    /// An explicit request waives the courtesy guards — including "not armed", which is
    /// what makes a second click on a `ManualInstall` update mean "now". It never waives
    /// the export guard. Mirrors what `try_install(force: true)` builds.
    #[test]
    fn an_explicit_request_waives_only_the_courtesy_guards() {
        let forced = |c: Conditions| Conditions {
            armed: true,
            remote: false,
            playing: true,
            ..c
        };

        // The worst case: unarmed, paused, and a renderer owns playback.
        let held = Conditions {
            staged: true,
            armed: false,
            exporting: false,
            remote: true,
            playing: false,
        };
        assert_eq!(decide(held), Err(Hold::NotArmed), "held automatically");
        assert_eq!(decide(forced(held)), Ok(()), "but a request goes through");

        assert_eq!(
            decide(forced(Conditions {
                exporting: true,
                ..held
            })),
            Err(Hold::Exporting),
            "an export is never waived, even on request"
        );

        // Force cannot conjure an update that was never downloaded.
        assert_eq!(
            decide(forced(Conditions {
                staged: false,
                ..held
            })),
            Err(Hold::NothingStaged)
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
        assert!(TICK < MAX_WAIT);
    }
}
