//! Auto-synced lyrics: when LRCLIB has a song's words but no timings, work the timings out here.
//!
//! A speech model (Parakeet, in a separately downloaded sidecar) hears the song; the `autosync`
//! crate lines the known words up with what it heard. Nothing is published anywhere: the result
//! is kept on this machine and shown as "auto-synced", and the listener can review it in the
//! timing editor and publish it to LRCLIB from there. See `plans/auto-synced-lyrics.md`.
//!
//! **Drafts.** When LRCLIB has no words at all, what the model heard *is* the lyrics: a draft,
//! shown as one, for the listener to review line by line before it can be published. See
//! `plans/draft-lyrics.md`.
//!
//! **When.** The engine downloads the next song whole while the current one plays (into the song
//! cache, or a transient one when that is off) and says so with `NextDownloaded`. That is the
//! moment to analyse it: the bytes it reads are the bytes that will play, and a typical song
//! leaves three or four minutes to finish before it starts. A song that starts with nothing
//! prepared — the first of a session, or after a skip — is analysed from the playhead onwards
//! instead, and lines appear as each window is heard.

mod component;
mod sidecar;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

pub use component::Status;
use sidecar::{JobEvent, Sidecar};

/// Coverage below which a result is not shown as synced. Measured on the 56-track corpus: tracks
/// under it are where the model barely heard the vocals, and their timings are mostly guesses.
const GATE: f64 = 0.6;

/// How long an idle sidecar keeps its model loaded before it is stopped to free the memory.
const IDLE: Duration = Duration::from_secs(90);

/// Don't start on a song that is about to end: its lines would arrive after it had.
const TOO_LATE: Duration = Duration::from_secs(20);

/// Managed state.
#[derive(Default)]
pub struct AutoSyncCtl {
    sidecar: tokio::sync::Mutex<Option<Arc<Sidecar>>>,
    /// Jobs running or queued, by Pandora track id.
    jobs: Mutex<HashMap<String, ()>>,
    installing: AtomicBool,
    stop_install: Arc<AtomicBool>,
    /// The download's progress, or why it failed, while there is one to report.
    status: Mutex<Option<Status>>,
    last_used: Mutex<Option<Instant>>,
}

/// Start-up: clear what an earlier run left mid-job, and start the idle reaper.
pub fn init(app: &AppHandle) {
    if let Some(dir) = work_dir(app) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let ctl = app.state::<AutoSyncCtl>();
            let idle = ctl.jobs.lock().unwrap().is_empty()
                && ctl
                    .last_used
                    .lock()
                    .unwrap()
                    .is_none_or(|t| t.elapsed() > IDLE);
            if idle {
                if let Some(s) = ctl.sidecar.lock().await.take() {
                    eprintln!("[autosync] idle; stopping the speech engine");
                    drop(s);
                }
            }
        }
    });
}

/// The setting changed: start or stop accordingly.
pub fn apply(app: &AppHandle, setting: &crate::settings::AutoSync) {
    let ctl = app.state::<AutoSyncCtl>();
    if !setting.enabled {
        ctl.stop_install.store(true, Ordering::Relaxed);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            app.state::<AutoSyncCtl>().sidecar.lock().await.take();
        });
        return;
    }
    if component::available() && component::installed(app).is_none() {
        start_install(app);
        return;
    }
    // A changed GPU choice applies from the sidecar's next start, so an idle one can go now.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let ctl = app.state::<AutoSyncCtl>();
        if ctl.jobs.lock().unwrap().is_empty() {
            ctl.sidecar.lock().await.take();
        }
        kick(&app);
    });
}

/// Whether the engine should fetch the next song whole for us: auto-sync is on and installed.
pub fn wants_next_file(app: &AppHandle, setting: &crate::settings::AutoSync) -> bool {
    setting.enabled && component::installed(app).is_some()
}

/// Stored timings for a track, if they were made from exactly this plain text and passed the
/// quality gate. Called by `lyrics::fetch_lyrics`.
pub fn stored_lrc(app: &AppHandle, key: &str, plain: &str) -> Option<String> {
    current(app, key, plain)?.lrc
}

/// What a stored result was made from, so a flag on auto-synced lyrics can keep it after the
/// cache is gone: with the plain words, it is enough to re-run the aligner on the song without
/// hearing it again.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub engine: String,
    pub aligner: u32,
    pub coverage: f64,
    /// `(text, start, end)`, as [`Stored::heard`].
    pub heard: Vec<(String, f64, f64)>,
}

/// The stored result's evidence for exactly these words, as it is on disk. Never re-times: the
/// flag is about the timings that were shown, and those came from the stored aligner.
pub fn evidence(app: &AppHandle, key: &str, plain: &str) -> Option<Evidence> {
    let s = stored(app, key, plain)?;
    Some(Evidence {
        engine: s.engine,
        aligner: s.aligner,
        coverage: s.coverage,
        heard: s.heard,
    })
}

/// A song started playing.
pub fn on_track_started(app: &AppHandle, track: &pandora::Track) {
    let app = app.clone();
    let track = track.clone();
    tauri::async_runtime::spawn(async move {
        cancel_stale(&app, &track.pandora_id).await;
        ensure(app, track, Source::Playing).await;
    });
}

/// The next song is on disk in full.
pub fn on_next_downloaded(app: &AppHandle, track: pandora::Track, path: PathBuf) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        ensure(app, track, Source::File(path)).await;
    });
}

enum Source {
    /// The song playing now: from its cached file if it has one, else from its stream.
    Playing,
    /// A complete file on disk.
    File(PathBuf),
}

/// Look at the playing and next songs, e.g. right after the engine was installed or turned on.
fn kick(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(engine) = app.state::<crate::native::NativeEngine>().try_engine() else {
            return;
        };
        if let Some(track) = engine.now_playing().await {
            ensure(app.clone(), track, Source::Playing).await;
        }
        if let Some(track) = engine.upcoming().await {
            if let Some(path) = engine.cached_file(&track.pandora_id) {
                ensure(app, track, Source::File(path)).await;
            }
        }
    });
}

/// Cancel jobs for songs that are neither playing nor next: a skip made them moot.
async fn cancel_stale(app: &AppHandle, playing: &str) {
    let ctl = app.state::<AutoSyncCtl>();
    let next = match app.state::<crate::native::NativeEngine>().try_engine() {
        Some(engine) => engine.upcoming().await.map(|t| t.pandora_id),
        None => None,
    };
    let stale: Vec<String> = ctl
        .jobs
        .lock()
        .unwrap()
        .keys()
        .filter(|id| id.as_str() != playing && Some(id.as_str()) != next.as_deref())
        .cloned()
        .collect();
    if stale.is_empty() {
        return;
    }
    let sidecar = ctl.sidecar.lock().await.clone();
    if let Some(s) = sidecar {
        for id in stale {
            s.cancel(&id);
        }
    }
}

/// Make sure `track` gets timings if it needs them and can have them.
async fn ensure(app: AppHandle, track: pandora::Track, source: Source) {
    let setting = crate::settings::get(&app).auto_sync;
    if !setting.enabled || track.pandora_id.is_empty() {
        return;
    }
    let Some(installed) = component::installed(&app) else {
        return;
    };
    {
        let ctl = app.state::<AutoSyncCtl>();
        let mut jobs = ctl.jobs.lock().unwrap();
        if jobs.contains_key(&track.pandora_id) {
            return;
        }
        jobs.insert(track.pandora_id.clone(), ());
    }
    let result = run(&app, &installed, &track, source, &setting).await;
    let ctl = app.state::<AutoSyncCtl>();
    ctl.jobs.lock().unwrap().remove(&track.pandora_id);
    *ctl.last_used.lock().unwrap() = Some(Instant::now());
    if let Err(e) = result {
        eprintln!(
            "[autosync] {} — {}: {e}",
            track.artist_name, track.song_title
        );
        emit_lines(&app, &track, Shown::default(), 0, 0, true, false);
    }
}

/// What a job is for.
enum Job {
    /// Time these words (LRCLIB's plain lyrics).
    Align(String),
    /// LRCLIB has no words: write them.
    Draft,
}

async fn run(
    app: &AppHandle,
    installed: &component::Installed,
    track: &pandora::Track,
    source: Source,
    setting: &crate::settings::AutoSync,
) -> Result<(), String> {
    let key = crate::lyrics::key_for(
        &track.artist_name,
        &track.song_title,
        Some(&track.album_title),
    );
    let duration = track.track_length as f64;
    let lyrics = crate::lyrics::lookup(
        app,
        track.artist_name.clone(),
        track.song_title.clone(),
        Some(track.album_title.clone()),
        (duration > 0.0).then_some(duration),
        None,
    )
    .await?;
    // Only words without timings, or no words at all: real timings, and the listener's own
    // edits, always win.
    if lyrics.overridden || lyrics.synced.is_some() {
        return Ok(());
    }
    let job = match lyrics.plain {
        // Done before for these words — or tried, and failed the gate. Not again until they
        // change, or the aligner does and the song has to be heard again to re-time it.
        Some(plain) if current(app, &key, &plain).is_some() => return Ok(()),
        Some(plain) => Job::Align(plain),
        // Drafted before, or found to have nothing worth drafting.
        None if !setting.drafts || current_draft(app, &key).is_some() => return Ok(()),
        None => Job::Draft,
    };

    let engine = app.state::<crate::native::NativeEngine>().try_engine();
    let (src, from) = match source {
        Source::File(path) => (path.to_string_lossy().into_owned(), 0.0),
        Source::Playing => {
            let position = engine.as_ref().map(|e| e.position()).unwrap_or_default();
            if duration > 0.0 && duration - position.as_secs_f64() < TOO_LATE.as_secs_f64() {
                return Ok(());
            }
            let src = engine
                .as_ref()
                .and_then(|e| e.cached_file(&track.pandora_id))
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| track.audio_url.clone());
            (src, position.as_secs_f64())
        }
    };
    eprintln!(
        "[autosync] {} — {}: {} from {from:.0}s",
        track.artist_name,
        track.song_title,
        match job {
            Job::Align(_) => "syncing",
            Job::Draft => "drafting",
        }
    );

    let dir = work_dir(app).ok_or("no cache directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let wav = dir.join(format!("{}.wav", file_stem(&track.pandora_id)));
    {
        let wav = wav.clone();
        tokio::task::spawn_blocking(move || decode_to_wav(&src, &wav))
            .await
            .map_err(|e| e.to_string())??;
    }

    let gpu = setting.gpu;
    let result = match &job {
        Job::Align(plain) => {
            let lines: Vec<&str> = plain.lines().collect();
            let shown = |heard: &[autosync::Heard], done: usize, of: usize| {
                let aligned = autosync::align::align(&lines, heard);
                // Partway through, only part of the song has been heard, so judge coverage
                // against that part. Below it, show progress but not timings that are guesses.
                let heard_share = done as f64 / of.max(1) as f64;
                let lrc = (aligned.coverage() >= GATE * heard_share)
                    .then(|| lrc_of(&aligned, &lines, duration))
                    .flatten();
                Shown { lrc, doubt: None }
            };
            hear(app, installed, track, &wav, from, gpu, shown).await
        }
        // The draft so far is shown as it grows; while there isn't one yet, nothing is.
        Job::Draft => {
            let shown = |heard: &[autosync::Heard], _: usize, _: usize| {
                draft_of(heard).map(Shown::from).unwrap_or_default()
            };
            hear(app, installed, track, &wav, from, gpu, shown).await
        }
    };
    let _ = std::fs::remove_file(&wav);
    let Some(heard) = result? else {
        // Cancelled: a skip made it moot. Nothing stored, so it is tried again if it comes back.
        return Ok(());
    };

    let plain = match job {
        Job::Align(plain) => plain,
        Job::Draft => {
            finish_draft(app, installed, track, &key, &heard, duration);
            return Ok(());
        }
    };
    let (lrc, coverage) = time(&plain, &heard, duration);
    let ok = lrc.is_some();
    eprintln!(
        "[autosync] {} — {}: {:.0}% of words heard{}",
        track.artist_name,
        track.song_title,
        coverage * 100.0,
        if ok {
            ""
        } else {
            "; below the gate, not shown"
        }
    );
    store(
        app,
        &key,
        &Stored {
            plain_hash: fnv(&plain),
            lrc: lrc.clone(),
            coverage,
            engine: installed.manifest.version.clone(),
            created: unix_now(),
            aligner: autosync::align::VERSION,
            heard: heard
                .iter()
                .map(|w| (w.text.clone(), w.start, w.end))
                .collect(),
            duration,
        },
    );
    emit_lines(app, track, Shown { lrc, doubt: None }, 0, 0, true, ok);
    Ok(())
}

/// Store a finished draft, or why there isn't one, and tell the pane.
fn finish_draft(
    app: &AppHandle,
    installed: &component::Installed,
    track: &pandora::Track,
    key: &str,
    heard: &[autosync::Heard],
    duration: f64,
) {
    let drafted = draft_of(heard);
    eprintln!(
        "[autosync] {} — {}: {}",
        track.artist_name,
        track.song_title,
        match &drafted {
            Ok(d) => format!("drafted {} lines", d.doubt.len()),
            Err(why) => format!("no draft ({why})"),
        }
    );
    let mut stored = StoredDraft {
        lrc: None,
        doubt: Vec::new(),
        none: None,
        engine: installed.manifest.version.clone(),
        created: unix_now(),
        drafter: autosync::draft::VERSION,
        heard: heard
            .iter()
            .map(|w| (w.text.clone(), w.start, w.end, w.p))
            .collect(),
        duration,
    };
    stored.set(drafted.clone());
    store_draft(app, key, &stored);
    let ok = drafted.is_ok();
    let shown = drafted.map(Shown::from).unwrap_or_default();
    emit_lines(app, track, shown, 0, 0, true, ok);
}

/// A draft, as the pane and editor get it.
#[derive(Clone)]
pub struct Draft {
    pub lrc: String,
    /// Per line of `lrc`, the positions of the words the engine was unsure of.
    pub doubt: Vec<Vec<usize>>,
}

impl From<Draft> for Shown {
    fn from(d: Draft) -> Self {
        Shown {
            lrc: Some(d.lrc),
            doubt: Some(d.doubt),
        }
    }
}

/// The draft of what was heard, or why there is none: `noSinging` or `unclear`, the names the
/// pane knows.
fn draft_of(heard: &[autosync::Heard]) -> Result<Draft, &'static str> {
    use autosync::draft::{self, NoDraft};
    let params = draft::Params::default();
    let lines = draft::draft_with(heard, &params).map_err(|why| match why {
        NoDraft::NoSinging { .. } => "noSinging",
        NoDraft::Unclear { .. } => "unclear",
    })?;
    Ok(Draft {
        // Without the final newline, which the editor would show as an empty last line.
        lrc: autosync::lrc::format(&draft::to_lrc(&lines))
            .trim_end()
            .to_string(),
        doubt: lines.iter().map(|l| l.doubtful(&params)).collect(),
    })
}

/// Run the job, showing what `shown` makes of the words so far after every window. Everything
/// heard, in order, or `None` if it was cancelled.
async fn hear(
    app: &AppHandle,
    installed: &component::Installed,
    track: &pandora::Track,
    wav: &Path,
    from: f64,
    gpu: bool,
    shown: impl Fn(&[autosync::Heard], usize, usize) -> Shown,
) -> Result<Option<Vec<autosync::Heard>>, String> {
    let sidecar = sidecar(app, installed, gpu).await?;
    let mut rx = sidecar.transcribe(&track.pandora_id, wav, from)?;
    let mut heard = Vec::new();
    loop {
        match rx.recv().await {
            Some(JobEvent::Words { done, of, words }) => {
                heard.extend(words);
                heard.sort_by(|a: &autosync::Heard, b| a.start.total_cmp(&b.start));
                emit_lines(app, track, shown(&heard, done, of), done, of, false, true);
            }
            Some(JobEvent::Done) => break,
            Some(JobEvent::Cancelled) => return Ok(None),
            Some(JobEvent::Error(e)) => return Err(e),
            None => return Err("the speech engine went away".into()),
        }
    }
    Ok(Some(heard))
}

/// The timings for `plain` from `heard`, or `None` below the quality gate, and the share of the
/// lyric words heard.
fn time(plain: &str, heard: &[autosync::Heard], duration: f64) -> (Option<String>, f64) {
    let lines: Vec<&str> = plain.lines().collect();
    let aligned = autosync::align::align(&lines, heard);
    let lrc = (aligned.coverage() >= GATE)
        .then(|| lrc_of(&aligned, &lines, duration))
        .flatten();
    (lrc, aligned.coverage())
}

fn lrc_of(aligned: &autosync::align::Alignment, lines: &[&str], duration: f64) -> Option<String> {
    let timed = aligned.timed(lines, if duration > 0.0 { duration } else { f64::MAX });
    (!timed.is_empty()).then(|| autosync::lrc::format(&timed))
}

/// The running sidecar, starting one if needed.
async fn sidecar(
    app: &AppHandle,
    installed: &component::Installed,
    gpu: bool,
) -> Result<Arc<Sidecar>, String> {
    let ctl = app.state::<AutoSyncCtl>();
    // Held across the start, so two jobs arriving together share one process.
    let mut slot = ctl.sidecar.lock().await;
    if let Some(s) = slot.as_ref().filter(|s| s.alive()) {
        return Ok(s.clone());
    }
    let installed = installed.clone();
    let started = tokio::task::spawn_blocking(move || Sidecar::start(&installed, gpu, threads()))
        .await
        .map_err(|e| e.to_string())??;
    let started = Arc::new(started);
    *slot = Some(started.clone());
    Ok(started)
}

/// A quarter of the logical processors, between 1 and 4: enough to stay well ahead of playback
/// (3 threads ran 3.5x realtime on a 15 W laptop chip) without crowding everything else.
fn threads() -> u32 {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (n / 4).clamp(1, 4) as u32
}

/// What the lyrics pane is sent. `lrc` is the whole song's timings so far (every line has one),
/// or `None` while there is nothing worth showing. `doubt` is set for a draft: the words of each
/// line the engine was unsure of.
#[derive(Default)]
struct Shown {
    lrc: Option<String>,
    doubt: Option<Vec<Vec<usize>>>,
}

/// Tell the lyrics pane. `last` marks the end, with `ok` saying whether the result passed the
/// gate (or, for a draft, whether there is one).
fn emit_lines(
    app: &AppHandle,
    track: &pandora::Track,
    shown: Shown,
    done: usize,
    of: usize,
    last: bool,
    ok: bool,
) {
    let _ = app.emit(
        "autosync://lines",
        json!({
            "title": track.song_title,
            "artist": track.artist_name,
            "lrc": shown.lrc,
            "draft": shown.doubt.is_some(),
            "doubt": shown.doubt,
            "done": done,
            "of": of,
            "final": last,
            "ok": ok,
        }),
    );
}

// ---- the speech engine download ----------------------------------------------------------

fn start_install(app: &AppHandle) {
    let ctl = app.state::<AutoSyncCtl>();
    if ctl.installing.swap(true, Ordering::SeqCst) {
        return;
    }
    ctl.stop_install.store(false, Ordering::Relaxed);
    let stop = ctl.stop_install.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let progress_app = app.clone();
        let result = component::install(&app, &stop, |done, total| {
            let status = Status::Downloading { done, total };
            *progress_app.state::<AutoSyncCtl>().status.lock().unwrap() = Some(status.clone());
            if last_emit.elapsed() > Duration::from_millis(250) || done == total {
                last_emit = Instant::now();
                component::emit(&progress_app, &status);
            }
        })
        .await;
        let ctl = app.state::<AutoSyncCtl>();
        ctl.installing.store(false, Ordering::SeqCst);
        let status = match result {
            Ok(_) => {
                *ctl.status.lock().unwrap() = None;
                // The engine can now fetch the next song whole for analysis.
                crate::native::apply_audio_cache(&app, &crate::settings::get(&app));
                kick(&app);
                component::status(&app)
            }
            Err(_) if stop.load(Ordering::Relaxed) => {
                *ctl.status.lock().unwrap() = None;
                component::status(&app)
            }
            Err(message) => {
                eprintln!("[autosync] download failed: {message}");
                let failed = Status::Failed { message };
                *ctl.status.lock().unwrap() = Some(failed.clone());
                failed
            }
        };
        component::emit(&app, &status);
    });
}

fn current_status(app: &AppHandle) -> Status {
    app.state::<AutoSyncCtl>()
        .status
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| component::status(app))
}

#[tauri::command]
pub fn autosync_status(app: AppHandle) -> Status {
    current_status(&app)
}

/// Download (or resume, or retry) the speech engine.
#[tauri::command]
pub fn autosync_install(app: AppHandle) -> Status {
    if component::available() && component::installed(&app).is_none() {
        start_install(&app);
    }
    current_status(&app)
}

/// Delete the speech engine and everything it made, freeing the disk.
#[tauri::command]
pub async fn autosync_remove(app: AppHandle) -> Result<Status, String> {
    let ctl = app.state::<AutoSyncCtl>();
    ctl.stop_install.store(true, Ordering::Relaxed);
    // The process has to be gone before its files can be deleted.
    ctl.sidecar.lock().await.take();
    for _ in 0..50 {
        if !ctl.installing.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    component::remove(&app)?;
    for dir in [results_dir(&app), drafts_dir(&app)].into_iter().flatten() {
        let _ = std::fs::remove_dir_all(dir);
    }
    *ctl.status.lock().unwrap() = None;
    crate::native::apply_audio_cache(&app, &crate::settings::get(&app));
    let status = component::status(&app);
    component::emit(&app, &status);
    Ok(status)
}

// ---- results --------------------------------------------------------------------------------

/// One song's outcome, kept so a song is heard once per version of its words.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    /// Of the plain lyrics the timings were made from: new words mean new timings.
    plain_hash: String,
    /// `None` when the result failed the gate; stored anyway, so it isn't retried every play.
    lrc: Option<String>,
    coverage: f64,
    /// The engine release that made it.
    engine: String,
    created: u64,
    /// The aligner that timed it (`autosync::align::VERSION`); 0 before that was kept.
    #[serde(default)]
    aligner: u32,
    /// What the engine heard, as `(text, start, end)`, so a newer aligner can re-time the song
    /// without hearing it again. Empty in results from before that was kept.
    #[serde(default)]
    heard: Vec<(String, f64, f64)>,
    /// The track's length, which placing lines after the last one heard needs.
    #[serde(default)]
    duration: f64,
}

/// In the cache dir: all of it can be made again.
fn results_dir(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_cache_dir().ok()?.join("lyrics-auto"))
}

fn stored(app: &AppHandle, key: &str, plain: &str) -> Option<Stored> {
    let path = crate::lyrics::hashed_file(results_dir(app)?, key)?;
    let s: Stored = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (s.plain_hash == fnv(plain)).then_some(s)
}

/// The stored result for these words, re-timed first if an older aligner made it. `None` when
/// there isn't one, or when it is too old to re-time because the words heard weren't kept: then
/// the song is heard again the next time it plays.
fn current(app: &AppHandle, key: &str, plain: &str) -> Option<Stored> {
    let mut s = stored(app, key, plain)?;
    if s.aligner == autosync::align::VERSION {
        return Some(s);
    }
    if s.heard.is_empty() {
        return None;
    }
    let heard: Vec<autosync::Heard> = s
        .heard
        .iter()
        .map(|(text, start, end)| autosync::Heard {
            text: text.clone(),
            start: *start,
            end: *end,
            p: 1.0,
        })
        .collect();
    (s.lrc, s.coverage) = time(plain, &heard, s.duration);
    s.aligner = autosync::align::VERSION;
    store(app, key, &s);
    Some(s)
}

fn store(app: &AppHandle, key: &str, result: &Stored) {
    let Some(path) = results_dir(app).and_then(|d| crate::lyrics::hashed_file(d, key)) else {
        return;
    };
    if let Ok(json) = serde_json::to_vec_pretty(result) {
        let _ = std::fs::write(path, json);
    }
}

// ---- drafts ---------------------------------------------------------------------------------

/// One song's draft, or why it has none, kept so a song is heard once.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredDraft {
    lrc: Option<String>,
    /// Per line of `lrc`, the positions of the words the engine was unsure of.
    doubt: Vec<Vec<usize>>,
    /// Why there is no draft, when there isn't: `noSinging` or `unclear`.
    none: Option<String>,
    engine: String,
    created: u64,
    /// The `autosync::draft::VERSION` that wrote it.
    drafter: u32,
    /// Everything heard, as `(text, start, end, p)`, so a newer drafter can redo the draft without
    /// hearing the song again. Unlike [`Stored::heard`] it keeps the confidence: drafting uses it.
    heard: Vec<(String, f64, f64, f32)>,
    duration: f64,
}

impl StoredDraft {
    fn set(&mut self, drafted: Result<Draft, &str>) {
        (self.lrc, self.doubt, self.none) = match drafted {
            Ok(d) => (Some(d.lrc), d.doubt, None),
            Err(why) => (None, Vec::new(), Some(why.to_string())),
        };
    }
}

/// Beside the aligned results, in the cache dir: a draft can be made again by hearing the song.
/// The listener's review of one is a saved edit, kept with the other edits instead.
fn drafts_dir(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_cache_dir().ok()?.join("lyrics-draft"))
}

/// The stored draft for a track, redone first if an older drafter wrote it.
fn current_draft(app: &AppHandle, key: &str) -> Option<StoredDraft> {
    let path = crate::lyrics::hashed_file(drafts_dir(app)?, key)?;
    let mut s: StoredDraft = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if s.drafter == autosync::draft::VERSION {
        return Some(s);
    }
    let heard: Vec<autosync::Heard> = s
        .heard
        .iter()
        .map(|(text, start, end, p)| autosync::Heard {
            text: text.clone(),
            start: *start,
            end: *end,
            p: *p,
        })
        .collect();
    s.set(draft_of(&heard));
    s.drafter = autosync::draft::VERSION;
    store_draft(app, key, &s);
    Some(s)
}

/// A track's draft, or why it has none; `None` if it hasn't been heard. Called by
/// `lyrics::fetch_lyrics` for tracks LRCLIB has nothing for.
pub fn stored_draft(app: &AppHandle, key: &str) -> Option<Result<Draft, String>> {
    let s = current_draft(app, key)?;
    Some(match s.lrc {
        Some(lrc) => Ok(Draft {
            lrc,
            doubt: s.doubt,
        }),
        None => Err(s.none.unwrap_or_default()),
    })
}

fn store_draft(app: &AppHandle, key: &str, draft: &StoredDraft) {
    let Some(path) = drafts_dir(app).and_then(|d| crate::lyrics::hashed_file(d, key)) else {
        return;
    };
    if let Ok(json) = serde_json::to_vec_pretty(draft) {
        let _ = std::fs::write(path, json);
    }
}

// ---- audio ----------------------------------------------------------------------------------

/// Where the 16 kHz copies live while a job runs.
fn work_dir(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_cache_dir().ok()?.join("autosync-work"))
}

/// Decode `src` (a file or a URL) to the 16 kHz mono 16-bit WAV the sidecar reads. Media
/// Foundation does the resampling.
fn decode_to_wav(src: &str, wav: &Path) -> Result<(), String> {
    let want = audio::Format {
        sample_rate: 16_000,
        channels: 1,
        bits_per_sample: 16,
    };
    let mut decoder = audio::Decoder::open_at(src, Some(want)).map_err(|e| e.to_string())?;
    if decoder.format() != want {
        return Err(format!(
            "decoder gave {:?}, not 16 kHz mono",
            decoder.format()
        ));
    }
    let pcm = decoder.decode_all().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(44 + pcm.len());
    let len = pcm.len() as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&pcm);
    std::fs::write(wav, out).map_err(|e| e.to_string())
}

/// A file name for a Pandora id like `TR:12345`.
fn file_stem(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stems_are_safe() {
        assert_eq!(file_stem("TR:123/../x"), "TR_123____x");
    }

    /// Media Foundation straight to the sidecar's input format. Needs a real song:
    /// `JARLID_AUTOSYNC_TEST_AUDIO=<file.m4a> cargo test -- --ignored decodes_to_16k`.
    #[test]
    #[ignore = "needs an audio file; see the doc comment"]
    fn decodes_to_16k_wav() {
        let src =
            std::env::var("JARLID_AUTOSYNC_TEST_AUDIO").expect("set JARLID_AUTOSYNC_TEST_AUDIO");
        let wav = std::env::temp_dir().join("jarlid-autosync-decode-test.wav");
        decode_to_wav(&src, &wav).unwrap();
        let bytes = std::fs::read(&wav).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            16_000
        );
        assert!(
            bytes.len() > 44 + 16_000 * 2 * 10,
            "under ten seconds of audio"
        );
        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn results_from_before_the_aligner_was_kept_read_as_too_old() {
        // As v1.13.0 wrote them: no aligner version, no heard words.
        let old = r#"{"plainHash":"c4cc69423559b7d0","lrc":"[00:04.33] Fake faces\n",
            "coverage":0.65,"engine":"0.1.0","created":1792117168}"#;
        let s: Stored = serde_json::from_str(old).unwrap();
        assert_ne!(s.aligner, autosync::align::VERSION);
        assert!(s.heard.is_empty());
    }

    #[test]
    fn plain_hash_changes_with_the_words() {
        assert_ne!(fnv("a line"), fnv("a line!"));
        assert_eq!(fnv("same"), fnv("same"));
    }
}
