//! Updates that land when nothing is playing.
//!
//! The download happens invisibly in the background; the *install* — which exits the
//! process and relaunches it — waits for a moment that costs the listener nothing. There
//! are two such moments, in order of preference:
//!
//! - **Paused.** The best one: nothing is being cut off, and the app is told to come back
//!   paused (see [`arm_resume_paused`]) so the restart is inaudible. [`PAUSE_SETTLE`] keeps
//!   a five-second pause from restarting the app.
//! - **A track boundary.** Interrupting a running song is acceptable if we truly have to,
//!   but always prefer waiting a couple of minutes for it to end. [`MAX_WAIT`] is a backstop
//!   for when no boundary ever arrives.
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

/// How much *playing* time to allow without a track boundary before installing anyway.
///
/// Longer than almost any song, so it only fires when something is wrong — a playhead that
/// has stopped advancing, say — rather than as a routine timeout. Deliberately not wall
/// time: see [`Waiting`].
const MAX_WAIT: Duration = Duration::from_secs(6 * 60);

/// How long playback must have been continuously paused before an update installs itself.
///
/// Long enough that pausing to answer a question does not restart the app under you, short
/// enough that walking away for a minute is sufficient. Combined with [`TICK`] the real
/// latency is up to about fifty seconds, which nobody is timing.
///
/// Sampled at [`TICK`] granularity rather than measured, so this is really "paused at every
/// sample across 30 s" — a brief resume that falls between two samples goes unseen. The
/// last-moment re-check in [`try_install`] is what covers the case that actually matters,
/// which is playback running at the instant of the install.
const PAUSE_SETTLE: Duration = Duration::from_secs(30);

/// How often the loop wakes to re-evaluate. Coarse on purpose.
const TICK: Duration = Duration::from_secs(20);

/// Marker read once at launch: come back paused rather than playing.
///
/// A file rather than a process argument because the NSIS installer relaunches us with the
/// *old* process's arguments (`/UPDATE /ARGS …`), so there is nothing to append to. It lives
/// next to `last-station.json` in the config directory, which the installer does not touch.
///
/// It holds the version it was written for, which is what makes it self-validating rather
/// than merely time-limited. `install()` exits this process the moment NSIS is launched, so
/// a successful hand-off is not a successful *install* — the listener can still cancel the
/// UAC prompt, or the installer can fail. Then they relaunch by hand, which is the natural
/// reaction, and an unconditional marker would greet them with a silent app. The relaunched
/// binary is only the new version if the install truly happened, so requiring the marker to
/// name the running version answers exactly that question.
const RESUME_PAUSED: &str = "resume-paused";

/// How long the [`RESUME_PAUSED`] marker stays meaningful.
///
/// Belt and braces behind the version check: the marker is deleted on read and on a failed
/// install, and a stale one is already inert against a different version. This only covers
/// being killed between writing it and installing, where the version would still match.
const RESUME_PAUSED_TTL: Duration = Duration::from_secs(10 * 60);

struct Staged {
    version: String,
    bytes: Vec<u8>,
    update: Update,
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
}

/// How long an update has been waiting for a moment, split by what the listener was doing.
///
/// Neither clock is wall time, and that distinction is the whole point. The backstop asks
/// "how long have we been *playing* without a track boundary"; the paused install asks "how
/// long has playback been *stopped*, without interruption".
///
/// Conflating them was a real bug: the backstop used to measure wall time while its gate
/// only opened once playback resumed, so an update staged during a pause longer than
/// [`MAX_WAIT`] arrived at the moment the listener pressed play with its six minutes already
/// spent — and cut the song they had just resumed in half, every time.
///
/// `ready` is the same lesson generalised. Neither clock may run while the install is held
/// for a reason that has nothing to do with timing — not armed, an export running, a network
/// player owning playback. Otherwise the countdown is spent behind a shut gate and fires the
/// instant the gate opens: clicking "install after this song" on an update that had been
/// sitting unarmed all evening would restart the app mid-song, having promised the opposite.
#[derive(Default, Debug, PartialEq, Eq, Clone, Copy)]
struct Waiting {
    /// Playing time while nothing but a track boundary was missing. Survives pauses: a pause
    /// does not make an absent boundary any less absent.
    playing: Duration,
    /// The *current* pause, reset the moment playback resumes.
    paused: Duration,
}

impl Waiting {
    /// `ready` means every condition except the moment itself is satisfied.
    fn tick(&mut self, ready: bool, playback: Playback, dt: Duration) {
        match (ready, playback) {
            (false, _) => *self = Self::default(),
            (true, Playback::Playing) => {
                self.playing += dt;
                self.paused = Duration::ZERO;
            }
            (true, Playback::Paused) => self.paused += dt,
            // Neither clock may advance on a guess. Holding rather than resetting means a
            // blind spot — a sign-out, a moment mid-login — does not quietly discard a
            // legitimate wait that was already under way.
            (true, Playback::Unknown) => {}
        }
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
        });
        ctl.armed
            .store(policy.arms_automatically(), Ordering::SeqCst);
    }
    publish(app);

    // "Instant" means exactly that: do not wait for a boundary. It still comes back the way
    // it was left, so "instant" while paused is silent rather than merely fast.
    if policy == Policy::Instant {
        try_install(app, playback(app).await, true, "instant policy", true);
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
    MidSong,
    Unknown,
}

impl Hold {
    fn reason(&self) -> &'static str {
        match self {
            Hold::NothingStaged => "nothing staged",
            Hold::NotArmed => "waiting to be asked",
            Hold::Exporting => "an export is running",
            Hold::Remote => "a network player owns playback",
            Hold::MidSong => "a song is playing",
            Hold::Unknown => "there is no engine to ask what playback is doing",
        }
    }
}

/// What the app should do when it comes back up.
///
/// Derived from what playback was doing at the instant of the decision, and nothing else:
/// restarting must not change whether music is coming out of the speakers.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Resume {
    Playing,
    Paused,
}

/// What local playback is doing — including "we cannot tell".
///
/// The third case is the whole point. There is no engine to ask before sign-in finishes at
/// launch, or at any time while signed out, and `engine()` reports that as an error. Folding
/// that into "paused" was a real bug: the loop counted [`PAUSE_SETTLE`] against a listener
/// who had never touched the play button, installed, and armed the silent restart — so a
/// network hiccup at launch meant the *next* successful launch came up silent for no reason.
/// It compounded, too, because an app that comes back paused really is paused, so every
/// later update legitimately came back paused as well.
///
/// Only [`Playback::Paused`] — a genuine, observed pause — may arm a silent restart.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Playback {
    Playing,
    Paused,
    /// No engine to ask: signed out, or still logging in. Deliberately *not* paused.
    Unknown,
}

/// What the world looks like when we consider installing.
#[derive(Debug, Clone, Copy)]
struct Conditions {
    staged: bool,
    /// Cleared to install. Set automatically by `Instant`/`AfterSong`, by hand otherwise.
    armed: bool,
    exporting: bool,
    remote: bool,
    /// What playback is doing. Polled, not assumed — the one caller that still asserts it is
    /// the track boundary, where a track having just ended is proof.
    playback: Playback,
    /// The caller has a mandate to cut off a running song: a track just ended, the backstop
    /// expired, or the user asked for it. Without one, playing audio is left alone.
    may_interrupt: bool,
}

/// The whole decision, as a pure function.
///
/// Deliberately separated from the Tauri plumbing: the rest of this module cannot run
/// outside a release build, so this is the only part that can be tested at all — and it is
/// the part where a mistake means restarting the app at a bad moment.
fn decide(c: Conditions) -> Result<Resume, Hold> {
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
    match c.playback {
        // Needs no mandate at all: nothing is being cut off, and `Resume::Paused` means the
        // app comes back exactly as it was left.
        Playback::Paused => Ok(Resume::Paused),
        // Audible playback is the case that has to justify itself.
        Playback::Playing if c.may_interrupt => Ok(Resume::Playing),
        Playback::Playing => Err(Hold::MidSong),
        // Nothing audible, but nothing *known* either, so this is not the silent moment the
        // paused path is allowed to use. An explicit request still goes through — and comes
        // back playing, because being signed out is not somebody asking for silence.
        Playback::Unknown if c.may_interrupt => Ok(Resume::Playing),
        Playback::Unknown => Err(Hold::Unknown),
    }
}

fn conditions(app: &tauri::AppHandle, playback: Playback, may_interrupt: bool) -> Conditions {
    let ctl = app.state::<UpdateCtl>();
    Conditions {
        staged: ctl.is_staged(),
        armed: ctl.is_armed(),
        exporting: app.state::<crate::export::ExportCtl>().is_running(),
        remote: crate::remote_active(),
        playback,
        may_interrupt,
    }
}

/// A track just ended — a moment we are allowed to use.
///
/// The only place playback is asserted rather than polled, and it is earned: the engine has
/// already started the next track by the time this event arrives, so there genuinely is
/// audio running.
pub fn on_track_boundary(app: &tauri::AppHandle) {
    try_install(app, Playback::Playing, true, "track boundary", false);
}

/// `force` is an explicit request: it waives the guards that exist purely to avoid
/// surprising the listener (not armed, a renderer owning playback, and the requirement of a
/// mandate to interrupt), because a deliberate request is not a surprise. It does **not**
/// waive the export guard, which protects work in progress rather than anyone's comfort —
/// nor does it override `playback`, because "install now" is not a request to start music —
/// nor, when we cannot tell what playback is doing, a request for silence.
fn try_install(
    app: &tauri::AppHandle,
    playback: Playback,
    may_interrupt: bool,
    why: &str,
    force: bool,
) {
    let ctl = app.state::<UpdateCtl>();
    let mut c = conditions(app, playback, may_interrupt);
    if force {
        c.armed = true;
        c.remote = false;
        c.may_interrupt = true;
    }
    let resume = match decide(c) {
        Ok(resume) => resume,
        Err(h) => {
            // These two are the steady state at every track boundary; logging them would be
            // pure noise.
            if !matches!(h, Hold::NothingStaged | Hold::NotArmed) {
                eprintln!("[updater] holding install ({why}): {}", h.reason());
            }
            return;
        }
    };
    if ctl.firing.swap(true, Ordering::SeqCst) {
        return;
    }

    let Some(staged) = ctl.staged.lock().unwrap().take() else {
        ctl.firing.store(false, Ordering::SeqCst);
        return;
    };

    // Written before the install rather than after, because `install` never returns.
    if resume == Resume::Paused {
        arm_resume_paused(app, &staged.version);
    }

    eprintln!(
        "[updater] installing v{} at {why} (coming back {})",
        staged.version,
        match resume {
            Resume::Playing => "playing",
            Resume::Paused => "paused",
        }
    );
    let _ = app.emit("app://update-installing", staged.version.clone());
    // Let the UI paint the notice before the process exits under it.
    std::thread::sleep(Duration::from_millis(250));

    // The listener can hit play — or pause — inside that window. Going ahead anyway would
    // restart the app into the state they just left, which is precisely the interruption
    // all of this exists to avoid. Standing down costs nothing: the next tick or boundary
    // picks it up again, with the state re-read.
    if paused_now(app).is_some_and(|p| p != (resume == Resume::Paused)) {
        eprintln!("[updater] stood down ({why}): playback changed while arming");
        disarm_resume_paused(app);
        *ctl.staged.lock().unwrap() = Some(staged);
        ctl.firing.store(false, Ordering::SeqCst);
        let _ = app.emit("app://update-stood-down", ());
        publish(app);
        return;
    }

    // No network here: the bytes are downloaded and verified, and the handle is held.
    // On success this never returns — the plugin launches the installer and exits(0).
    if let Err(e) = staged.update.install(&staged.bytes) {
        eprintln!("[updater] install failed: {e}");
        disarm_resume_paused(app);
        let _ = app.emit("app://update-failed", staged.version.clone());
        // Put it back so a later boundary can retry rather than losing the download.
        *ctl.staged.lock().unwrap() = Some(staged);
        ctl.firing.store(false, Ordering::SeqCst);
        publish(app);
    }
}

fn resume_paused_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(RESUME_PAUSED))
}

/// Tell the next launch to load a track without starting it — but only if that launch is
/// the version we are installing.
fn arm_resume_paused(app: &tauri::AppHandle, version: &str) {
    if let Some(path) = resume_paused_path(app) {
        if let Err(e) = std::fs::write(&path, version) {
            // Not fatal, but worth saying: the update still installs, it just comes back
            // playing at someone who had it paused.
            eprintln!("[updater] could not arm the paused restart: {e}");
        }
    }
}

fn disarm_resume_paused(app: &tauri::AppHandle) {
    if let Some(path) = resume_paused_path(app) {
        let _ = std::fs::remove_file(path);
    }
}

/// Should this launch come up paused? Consumes the marker, so it answers `true` exactly
/// once per restart that asked for it.
pub fn take_resume_paused(app: &tauri::AppHandle) -> bool {
    let Some(path) = resume_paused_path(app) else {
        return false;
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };

    let fresh = meta
        .modified()
        .map(|t| t.elapsed().unwrap_or_default() < RESUME_PAUSED_TTL)
        .unwrap_or(false);
    let wanted = std::fs::read_to_string(&path).unwrap_or_default();
    let running = app.package_info().version.to_string();
    let ours = wanted.trim() == running;

    // Removed whatever we decided: a marker we have chosen to ignore must not linger and
    // surprise the launch after this one. Existence came from the metadata above, not from
    // this call succeeding — if the file is momentarily locked, saying "it wasn't there"
    // would leave it to be consumed by an unrelated launch later.
    if let Err(e) = std::fs::remove_file(&path) {
        eprintln!("[updater] could not clear the paused-restart marker: {e}");
    }
    if !ours {
        // The overwhelmingly likely cause is an install that did not happen — a cancelled
        // UAC prompt, say — so the listener is looking at the old version and did not ask
        // for any of this.
        eprintln!("[updater] ignoring a paused-restart marker for v{wanted} (running v{running})");
    } else if !fresh {
        eprintln!("[updater] ignoring a stale paused-restart marker");
    }
    ours && fresh
}

/// Local paused state without awaiting, for the last look before an irreversible step.
///
/// `None` means "could not tell" — not signed in, or the engine mutex was momentarily
/// contended — and callers treat that as no reason to change course.
fn paused_now(app: &tauri::AppHandle) -> Option<bool> {
    Some(
        app.state::<crate::native::NativeEngine>()
            .try_engine()?
            .is_paused(),
    )
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
        try_install(&app, playback(&app).await, true, "user asked", true);
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
        let mut waiting = Waiting::default();

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

            let playback = playback(&app).await;
            // Everything except the moment: staged, armed, no export, no network player.
            // Asking `decide` with a mandate leaves exactly those guards standing, so this
            // cannot drift out of step with the real rules.
            let ready = decide(conditions(&app, playback, true)).is_ok();
            waiting.tick(ready, playback, TICK);

            match playback {
                // The preferred moment. Nothing is cut off and the app comes back paused, so
                // from the listener's side the update simply never happened.
                Playback::Paused if waiting.paused >= PAUSE_SETTLE => {
                    try_install(&app, playback, false, "paused", false);
                }
                // Backstop: playing this long with no track boundary means one is not
                // coming, so interrupting is the lesser evil.
                Playback::Playing if waiting.playing >= MAX_WAIT => {
                    try_install(&app, playback, true, "backstop (no track boundary)", false);
                }
                // Unknown never triggers an install on its own. Waiting costs nothing: the
                // boundary path picks it up as soon as anything is playing, and the paused
                // path as soon as playback is genuinely stopped.
                _ => {}
            }
        }
    });
}

/// What local playback is doing. The engine owns the answer, so there is no need to infer it
/// from playhead motion the way the DOM-scraping era had to.
///
/// The error case is [`Playback::Unknown`], never `Paused`: `engine()` fails when there is no
/// engine at all — signed out, or still logging in — which says nothing whatsoever about what
/// the listener wants.
async fn playback(app: &tauri::AppHandle) -> Playback {
    match app.state::<crate::native::NativeEngine>().engine().await {
        Ok(engine) if engine.is_paused() => Playback::Paused,
        Ok(_) => Playback::Playing,
        Err(_) => Playback::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything lined up for an install: playing, at a track boundary.
    fn ready() -> Conditions {
        Conditions {
            staged: true,
            armed: true,
            exporting: false,
            remote: false,
            playback: Playback::Playing,
            may_interrupt: true,
        }
    }

    #[test]
    fn installs_when_a_track_ends_with_an_armed_update() {
        assert_eq!(decide(ready()), Ok(Resume::Playing));
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

    /// The whole point: a paused app updates on its own, with no mandate to interrupt
    /// anything, because there is nothing to interrupt.
    #[test]
    fn a_paused_app_installs_without_needing_a_mandate() {
        assert_eq!(
            decide(Conditions {
                playback: Playback::Paused,
                may_interrupt: false,
                ..ready()
            }),
            Ok(Resume::Paused)
        );
    }

    /// …and it must come back the way it was left. Restarting a paused app into playing
    /// starts music at someone who deliberately stopped it, which is what made "never while
    /// paused" the rule before the app could come back paused.
    #[test]
    fn restarting_never_changes_whether_music_is_playing() {
        for may_interrupt in [true, false] {
            assert_eq!(
                decide(Conditions {
                    playback: Playback::Paused,
                    may_interrupt,
                    ..ready()
                }),
                Ok(Resume::Paused),
                "paused stays paused even with a mandate to interrupt"
            );
        }
        assert_eq!(decide(ready()), Ok(Resume::Playing));
    }

    /// The v1.4.1 regression.
    ///
    /// Before the third state existed, "no engine to ask" arrived here as `playing: false`
    /// and was indistinguishable from a deliberate pause. So a signed-out app — or one
    /// merely still logging in — banked `PAUSE_SETTLE`, installed, and armed the silent
    /// restart. The listener had never touched the play button, and the next launch came up
    /// silent with nothing to explain it.
    #[test]
    fn not_knowing_is_never_mistaken_for_a_pause() {
        let blind = Conditions {
            playback: Playback::Unknown,
            may_interrupt: false,
            ..ready()
        };
        assert_eq!(
            decide(blind),
            Err(Hold::Unknown),
            "must not install on its own while blind — that path arms a silent restart"
        );

        // And when it does go ahead, it must never be silently. Nobody asked for that.
        assert_eq!(
            decide(Conditions {
                may_interrupt: true,
                ..blind
            }),
            Ok(Resume::Playing),
            "an explicit request while signed out comes back playing, not paused"
        );
    }

    /// Only an observed pause may arm the silent restart. Stated as an exhaustive match so
    /// adding a fourth state forces a decision here rather than defaulting to silence.
    #[test]
    fn only_a_real_pause_arms_a_silent_restart() {
        for playback in [Playback::Playing, Playback::Paused, Playback::Unknown] {
            let outcome = decide(Conditions {
                playback,
                may_interrupt: true,
                ..ready()
            });
            let silent = outcome == Ok(Resume::Paused);
            assert_eq!(
                silent,
                playback == Playback::Paused,
                "{playback:?} must not decide to come back silent"
            );
        }
    }

    /// Audible playback is the case that has to justify itself. Without a mandate — no
    /// track boundary, no expired backstop, no click — a running song is left alone.
    #[test]
    fn never_cuts_into_a_song_without_a_reason() {
        assert_eq!(
            decide(Conditions {
                may_interrupt: false,
                ..ready()
            }),
            Err(Hold::MidSong)
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
            playback: Playback::Playing,
            may_interrupt: false,
        };
        assert_eq!(decide(c), Err(Hold::Exporting));
        assert_eq!(
            decide(Conditions {
                exporting: false,
                ..c
            }),
            Err(Hold::Remote)
        );
        assert_eq!(
            decide(Conditions {
                exporting: false,
                remote: false,
                ..c
            }),
            Err(Hold::MidSong)
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
            may_interrupt: true,
            ..c
        };

        // The worst case: unarmed, paused, and a renderer owns playback.
        let held = Conditions {
            staged: true,
            armed: false,
            exporting: false,
            remote: true,
            playback: Playback::Paused,
            may_interrupt: false,
        };
        assert_eq!(decide(held), Err(Hold::NotArmed), "held automatically");
        assert_eq!(
            decide(forced(held)),
            Ok(Resume::Paused),
            "but a request goes through — and 'install now' is not a request to start music"
        );

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
        assert!(
            PAUSE_SETTLE < MAX_WAIT,
            "a pause should not wait out a song"
        );
    }

    /// The regression this whole split exists for.
    ///
    /// Left paused for an hour with an update staged, the old wall-clock timer had long
    /// since passed `MAX_WAIT` — so the first thing that happened after pressing play was
    /// the backstop firing and cutting the resumed song in half. Paused time must not count
    /// toward "no track boundary arrived".
    #[test]
    fn a_long_pause_does_not_expire_the_backstop() {
        let mut w = Waiting::default();
        for _ in 0..(3600 / TICK.as_secs()) {
            w.tick(true, Playback::Paused, TICK);
        }
        assert!(w.paused >= Duration::from_secs(3600 - TICK.as_secs()));
        assert_eq!(
            w.playing,
            Duration::ZERO,
            "an hour paused is not an hour of playing"
        );

        // And the very next tick after pressing play must not trip it either.
        w.tick(true, Playback::Playing, TICK);
        assert!(w.playing < MAX_WAIT);
    }

    /// The paused clock is a streak, not a total: pausing for 20s twice with playback in
    /// between is not the same as being away from the keyboard.
    #[test]
    fn resuming_resets_the_pause_streak() {
        let mut w = Waiting::default();
        w.tick(true, Playback::Paused, TICK);
        w.tick(true, Playback::Paused, TICK);
        assert_eq!(w.paused, TICK * 2);
        w.tick(true, Playback::Playing, TICK);
        assert_eq!(w.paused, Duration::ZERO);
        assert_eq!(w.playing, TICK);
    }

    /// A pause in the middle does not make a missing track boundary any less missing, so
    /// the playing clock accumulates across it rather than restarting.
    #[test]
    fn the_playing_clock_survives_a_pause() {
        let mut w = Waiting::default();
        w.tick(true, Playback::Playing, TICK);
        w.tick(true, Playback::Paused, TICK);
        w.tick(true, Playback::Playing, TICK);
        assert_eq!(w.playing, TICK * 2);
    }

    /// Nothing staged means nothing to wait for — including after a failed install put the
    /// bytes back, which starts the clocks over rather than resuming a spent countdown.
    #[test]
    fn losing_the_staged_update_resets_both_clocks() {
        let mut w = Waiting::default();
        w.tick(true, Playback::Playing, TICK);
        w.tick(true, Playback::Paused, TICK);
        w.tick(false, Playback::Paused, TICK);
        assert_eq!(w, Waiting::default());
    }

    /// The same bug wearing a different gate.
    ///
    /// Under `ManualInstall` an update sits staged but unarmed indefinitely. If the clock
    /// ran anyway, an evening's listening would bank the whole backstop — and the click
    /// that arms it, whose own tooltip promises "install after this song", would restart
    /// the app mid-song within one tick. An export running does the same thing.
    #[test]
    fn a_countdown_never_runs_behind_a_shut_gate() {
        let mut w = Waiting::default();
        // Six minutes of listening while something other than timing holds the install.
        for _ in 0..(MAX_WAIT.as_secs() / TICK.as_secs() + 1) {
            w.tick(false, Playback::Playing, TICK);
        }
        assert_eq!(w, Waiting::default());

        // Now it is armed. The backstop starts from zero, so the track boundary that is
        // moments away gets to be the trigger, exactly as promised.
        w.tick(true, Playback::Playing, TICK);
        assert_eq!(w.playing, TICK);
        assert!(w.playing < MAX_WAIT);
    }
}
