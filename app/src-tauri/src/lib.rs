mod diagnostics;
mod export;
mod native;
#[cfg(windows)]
mod thumbbar;
mod updates;
mod upnp;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Listener, Manager};

#[derive(Serialize, Deserialize, Default)]
struct Lyrics {
    synced: Option<String>,
    plain: Option<String>,
    source: String,
}

/// Disk cache for LRCLIB responses (the service is slow). One JSON file per
/// track under the app cache dir, keyed by FNV-1a of artist|track|album.
fn lyrics_cache_path(app: &tauri::AppHandle, key: &str) -> Option<std::path::PathBuf> {
    let dir = app.path().app_cache_dir().ok()?.join("lyrics");
    std::fs::create_dir_all(&dir).ok()?;
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    Some(dir.join(format!("{h:016x}.json")))
}

/// Collapse a string that is exactly one part repeated 2-4 times (Pandora's
/// marquee clones its content while scrolling: "AbcAbcAbc" -> "Abc").
fn undouble(s: &str) -> String {
    let t = s.trim();
    let n = t.len();
    for k in (2..=4).rev() {
        if n >= k && n % k == 0 {
            let part = &t[..n / k];
            if !part.is_empty() && t.as_bytes().chunks(n / k).all(|c| c == part.as_bytes()) {
                return undouble(part);
            }
        }
    }
    t.to_string()
}

/// Strip trailing parentheticals / descriptors that hurt matching, e.g.
/// "Song (I Just Wanna Fall in Love)" -> "Song", "Song - Single" -> "Song".
fn simplify_title(s: &str) -> String {
    let mut t = s.to_string();
    if let Some(i) = t.find('(') {
        t.truncate(i);
    }
    if let Some(i) = t.find(" - ") {
        t.truncate(i);
    }
    t.trim().to_string()
}

/// Fetch lyrics from LRCLIB. Tries an exact `get`, then progressively looser
/// `search`es, choosing the best synced result by closest duration.
#[tauri::command]
async fn fetch_lyrics(
    app: tauri::AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
    duration: Option<f64>,
) -> Result<Lyrics, String> {
    let artist = undouble(&artist);
    let track = undouble(&track);
    let album = album.map(|a| undouble(&a));

    // Cache first — LRCLIB is slow and lyrics for a given track don't change.
    let cache_key = format!(
        "{}|{}|{}",
        artist.to_lowercase(),
        track.to_lowercase(),
        album.as_deref().unwrap_or("").to_lowercase()
    );
    let cache_file = lyrics_cache_path(&app, &cache_key);
    if let Some(ref p) = cache_file {
        if let Ok(bytes) = std::fs::read(p) {
            if let Ok(mut hit) = serde_json::from_slice::<Lyrics>(&bytes) {
                hit.source = format!("{} (cached)", hit.source);
                return Ok(hit);
            }
        }
    }

    let client = reqwest::Client::builder()
        .user_agent("PandoraDesktop (personal; https://github.com/cinderblock/pandora-desktop)")
        .build()
        .map_err(|e| e.to_string())?;

    // 1) exact match via /get (needs duration)
    if let Some(dur) = duration {
        let mut q: Vec<(&str, String)> = vec![
            ("artist_name", artist.clone()),
            ("track_name", track.clone()),
            ("duration", (dur.round() as i64).to_string()),
        ];
        if let Some(al) = album.clone() {
            if !al.is_empty() {
                q.push(("album_name", al));
            }
        }
        if let Ok(resp) = client
            .get("https://lrclib.net/api/get")
            .query(&q)
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    return Ok(cache_and_return(from_lrclib(&v, "lrclib/get"), &cache_file));
                }
            }
        }
    }

    // 2) search — full title first, then simplified title
    let mut candidates: Vec<(&str, String)> = vec![("full", track.clone())];
    let simple = simplify_title(&track);
    if simple != track && !simple.is_empty() {
        candidates.push(("simple", simple));
    }
    for (label, t) in candidates {
        if let Ok(resp) = client
            .get("https://lrclib.net/api/search")
            .query(&[("track_name", t.as_str()), ("artist_name", artist.as_str())])
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(arr) = resp.json::<serde_json::Value>().await {
                    if let Some(best) = pick_best(&arr, duration) {
                        return Ok(cache_and_return(
                            from_lrclib(best, &format!("lrclib/search/{label}")),
                            &cache_file,
                        ));
                    }
                }
            }
        }
    }

    Ok(Lyrics {
        source: "none".into(),
        ..Default::default()
    })
}

/// From a search-results array, prefer entries with synced lyrics and the closest
/// duration to what's playing.
fn pick_best<'a>(
    arr: &'a serde_json::Value,
    duration: Option<f64>,
) -> Option<&'a serde_json::Value> {
    let items = arr.as_array()?;
    if items.is_empty() {
        return None;
    }
    let target = duration.unwrap_or(0.0);
    let score = |v: &serde_json::Value| -> (i32, f64) {
        let has_synced = v
            .get("syncedLyrics")
            .and_then(|x| x.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let dur = v.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let dur_gap = if target > 0.0 && dur > 0.0 {
            (dur - target).abs()
        } else {
            9999.0
        };
        // synced first (lower rank better), then smallest duration gap
        (if has_synced { 0 } else { 1 }, dur_gap)
    };
    items.iter().min_by(|a, b| {
        let (sa, ga) = score(a);
        let (sb, gb) = score(b);
        sa.cmp(&sb)
            .then(ga.partial_cmp(&gb).unwrap_or(std::cmp::Ordering::Equal))
    })
}

/// Persist found lyrics to the cache (misses are not cached so they retry).
fn cache_and_return(l: Lyrics, path: &Option<std::path::PathBuf>) -> Lyrics {
    if l.synced.is_some() || l.plain.is_some() {
        if let Some(p) = path {
            if let Ok(json) = serde_json::to_vec(&l) {
                let _ = std::fs::write(p, json);
            }
        }
    }
    l
}

fn from_lrclib(v: &serde_json::Value, source: &str) -> Lyrics {
    let s = v.get("syncedLyrics").and_then(|x| x.as_str());
    let p = v.get("plainLyrics").and_then(|x| x.as_str());
    Lyrics {
        synced: s.filter(|x| !x.is_empty()).map(|x| x.to_string()),
        plain: p.filter(|x| !x.is_empty()).map(|x| x.to_string()),
        source: source.to_string(),
    }
}

/// Fire-and-forget transport for callers that aren't async (media keys, taskbar toolbar).
///
/// Previously `eval`'d into the injected bridge script; now drives the native engine. Errors are
/// logged rather than returned because these callers are OS callbacks with nowhere to report to.
fn engine_cmd(app: &tauri::AppHandle, cmd: &str) {
    // These callers are OS callbacks with nowhere to report to, so an unknown command would fail
    // completely silently — which is how the play/pause media key ended up doing nothing for a
    // release. Fail loudly in dev, and still log in release.
    debug_assert!(
        native::is_known_command(cmd),
        "media/taskbar dispatcher sent {cmd:?}, which native_cmd does not handle — \
         add an arm and list it in native::COMMANDS"
    );
    if !native::is_known_command(cmd) {
        eprintln!("[native] dispatcher sent unhandled command {cmd:?}");
    }

    let app = app.clone();
    let cmd = cmd.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<native::NativeEngine>();
        if let Err(e) = native::native_cmd(app.clone(), state, cmd.clone()).await {
            eprintln!("[native] command {cmd:?} failed: {e}");
        }
    });
}

/// Transport, routed to the native engine.
///
/// The name is kept from the webview era so the UI, media keys and taskbar toolbar keep working
/// unchanged — this used to `eval` into an injected bridge script; now it drives our own player.
#[tauri::command]
async fn player_cmd(
    app: tauri::AppHandle,
    state: tauri::State<'_, native::NativeEngine>,
    cmd: String,
) -> Result<(), String> {
    native::native_cmd(app, state, cmd).await
}

/// Transport command for the network (UPnP/DLNA) player shown in remote mode.
#[tauri::command]
async fn remote_cmd(ctl: tauri::State<'_, upnp::RemoteCtl>, cmd: String) -> Result<(), String> {
    let client = upnp::device_client();
    upnp::command(&client, &ctl, &cmd).await
}

/// List the network player's presets (WiiM Home app presets).
#[tauri::command]
async fn remote_presets(
    ctl: tauri::State<'_, upnp::RemoteCtl>,
) -> Result<Vec<upnp::Preset>, String> {
    let client = upnp::device_client();
    upnp::presets(&client, &ctl).await
}

/// Native Windows SMTC (media keys, volume-flyout / lock-screen media panel).
/// WebView2 does not bridge the page's MediaSession to Windows, so we own the
/// media session from Rust: bridge events feed metadata/state in, and SMTC
/// button presses drive the engine.
#[cfg(windows)]
fn setup_media_controls(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition,
        PlatformConfig,
    };
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    let main = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let hwnd = main.hwnd()?.0 as *mut std::ffi::c_void;

    let mut controls = MediaControls::new(PlatformConfig {
        dbus_name: "jarlid",
        display_name: "Jarlid",
        hwnd: Some(hwnd),
    })
    .map_err(|e| format!("SMTC init: {e:?}"))?;

    // Shared state: the controls live in a cell so the button callback can
    // report the expected playback state to Windows IMMEDIATELY — waiting for
    // motion-derived confirmation leaves the SMTC buttons unresponsive for
    // seconds after each press.
    let cell: Arc<Mutex<Option<MediaControls>>> = Arc::new(Mutex::new(None));
    let playing_now = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let optimistic_until = Arc::new(Mutex::new(Instant::now()));
    // Remote (network player) session takes over SMTC when it's the active
    // audio source: local idle >3s while the renderer plays.
    let ctl = app.state::<upnp::RemoteCtl>().inner().clone();
    let remote_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let last_local_move = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(60)));

    // Thumb state as the bridge reads it off Pandora's own buttons; it drives
    // the taskbar glyphs (filled when set).
    let thumb_up = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thumb_down = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bar: Arc<Mutex<Option<thumbbar::Thumbbar>>> = Arc::new(Mutex::new(None));
    let push_bar: Arc<dyn Fn() + Send + Sync> = {
        let bar = bar.clone();
        let playing = playing_now.clone();
        let up = thumb_up.clone();
        let down = thumb_down.clone();
        let remote = remote_active.clone();
        Arc::new(move || {
            use std::sync::atomic::Ordering;
            if let Some(b) = bar.lock().unwrap().as_ref() {
                b.set_state(thumbbar::State {
                    playing: playing.load(Ordering::Relaxed),
                    thumb_up: up.load(Ordering::Relaxed),
                    thumb_down: down.load(Ordering::Relaxed),
                    remote: remote.load(Ordering::Relaxed),
                });
            }
        })
    };

    // Both media surfaces — SMTC (media keys, volume flyout) and the taskbar
    // thumbnail toolbar — funnel through one dispatcher, so they can never
    // drift apart on remote-vs-local routing or optimistic state.
    #[derive(Clone, Copy)]
    enum Action {
        Play,
        Pause,
        Toggle,
        Next,
        Prev,
        ThumbUp,
        ThumbDown,
    }

    let handle = app.handle().clone();
    let cb_cell = cell.clone();
    let cb_playing = playing_now.clone();
    let cb_until = optimistic_until.clone();
    let cb_remote_active = remote_active.clone();
    let cb_ctl = ctl.clone();
    let cb_push = push_bar.clone();
    let dispatch: Arc<dyn Fn(Action) + Send + Sync> = Arc::new(move |action| {
        use std::sync::atomic::Ordering;
        let cur = cb_playing.load(Ordering::Relaxed);
        // (engine command, network-player command, expected playing state).
        // An empty remote command means the action has no remote equivalent.
        let (engine_c, remote_c, desired): (&str, &str, Option<bool>) = match action {
            Action::Play => ("play", "play", Some(true)),
            Action::Pause => ("pause", "pause", Some(false)),
            Action::Toggle => ("toggle", if cur { "pause" } else { "play" }, Some(!cur)),
            Action::Next => ("skip", "skip", None),
            Action::Prev => ("replay", "prev", None),
            Action::ThumbUp => ("thumbUp", "", None),
            Action::ThumbDown => ("thumbDown", "", None),
        };
        if cb_remote_active.load(Ordering::Relaxed) {
            if remote_c.is_empty() {
                return; // thumbs mean nothing to a DLNA/WiiM renderer
            }
            let ctl2 = cb_ctl.clone();
            let rc = remote_c.to_string();
            tauri::async_runtime::spawn(async move {
                let client = upnp::device_client();
                let _ = upnp::command(&client, &ctl2, &rc).await;
            });
        } else {
            engine_cmd(&handle, engine_c);
        }
        if let Some(p) = desired {
            cb_playing.store(p, Ordering::Relaxed);
            *cb_until.lock().unwrap() = Instant::now() + Duration::from_secs(2);
            // Tell the UI immediately — its icon otherwise waits ~2s for
            // motion-derived confirmation.
            let _ = handle.emit("player://optimistic", serde_json::json!({ "playing": p }));
            if let Some(c) = cb_cell.lock().unwrap().as_mut() {
                let state = if p {
                    MediaPlayback::Playing { progress: None }
                } else {
                    MediaPlayback::Paused { progress: None }
                };
                let _ = c.set_playback(state);
            }
            cb_push();
        }
    });

    let smtc_dispatch = dispatch.clone();
    controls
        .attach(move |event: MediaControlEvent| {
            smtc_dispatch(match event {
                MediaControlEvent::Play => Action::Play,
                MediaControlEvent::Pause => Action::Pause,
                MediaControlEvent::Toggle => Action::Toggle,
                MediaControlEvent::Next => Action::Next,
                MediaControlEvent::Previous => Action::Prev,
                MediaControlEvent::Stop => Action::Pause,
                _ => return,
            })
        })
        .map_err(|e| format!("SMTC attach: {e:?}"))?;
    *cell.lock().unwrap() = Some(controls);

    // Taskbar thumbnail toolbar — the transport row under the taskbar hover
    // preview. A separate shell API from SMTC (see thumbbar.rs); losing it
    // should never take the rest of the media integration down with it.
    {
        let bar_dispatch = dispatch.clone();
        match thumbbar::install(main.hwnd()?.0 as isize, move |button| {
            bar_dispatch(match button {
                thumbbar::Button::ThumbDown => Action::ThumbDown,
                thumbbar::Button::Replay => Action::Prev,
                thumbbar::Button::PlayPause => Action::Toggle,
                thumbbar::Button::Skip => Action::Next,
                thumbbar::Button::ThumbUp => Action::ThumbUp,
            })
        }) {
            Ok(installed) => {
                *bar.lock().unwrap() = Some(installed);
                push_bar();
            }
            Err(e) => eprintln!("[thumbbar] setup failed: {e}"),
        }
    }

    // Thumb state from the bridge → taskbar glyphs.
    {
        let up = thumb_up.clone();
        let down = thumb_down.clone();
        let push = push_bar.clone();
        app.listen_any("engine://thumbs", move |event| {
            use std::sync::atomic::Ordering;
            let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
                return;
            };
            let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
            up.store(b("thumbUp"), Ordering::Relaxed);
            down.store(b("thumbDown"), Ordering::Relaxed);
            push();
        });
    }

    // Track metadata from the bridge (local mode only).
    let c_meta = cell.clone();
    let meta_remote_active = remote_active.clone();
    app.listen_any("engine://nowplaying", move |event| {
        if meta_remote_active.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
            return;
        };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty());
        if let Some(c) = c_meta.lock().unwrap().as_mut() {
            let _ = c.set_metadata(MediaMetadata {
                title: s("title"),
                artist: s("artist"),
                album: s("album"),
                cover_url: s("artFallback").or_else(|| s("art")),
                duration: None,
            });
        }
    });

    // Playback state, motion-derived from the playhead (same logic as the UI:
    // Pandora's DOM and audio elements misreport paused, position motion doesn't).
    // During the optimistic grace window the button-press state wins.
    let c_play = cell.clone();
    let p_now = playing_now.clone();
    let p_until = optimistic_until.clone();
    let p_remote_active = remote_active.clone();
    let p_local_move = last_local_move.clone();
    let p_push = push_bar.clone();
    let motion = Arc::new(Mutex::new((f64::MIN, Instant::now(), Instant::now())));
    app.listen_any("engine://playhead", move |event| {
        use std::sync::atomic::Ordering;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
            return;
        };
        let pos = v.get("position").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let now = Instant::now();
        let mut m = motion.lock().unwrap();
        let (ref mut last_pos, ref mut last_move, ref mut last_sent) = *m;
        let was_playing = p_now.load(Ordering::Relaxed);
        let mut new_playing = was_playing;
        if (pos - *last_pos).abs() > 0.05 {
            *last_pos = pos;
            *last_move = now;
            *p_local_move.lock().unwrap() = now;
            new_playing = true;
        } else if now.duration_since(*last_move) > Duration::from_millis(1600) {
            new_playing = false;
        }
        // While the remote session owns SMTC, local (paused) state stays out.
        if p_remote_active.load(Ordering::Relaxed) {
            return;
        }
        if now < *p_until.lock().unwrap() {
            new_playing = was_playing; // grace: don't fight a fresh button press
        }
        // Update SMTC on state change, or every 5s to keep progress fresh.
        if new_playing != was_playing || now.duration_since(*last_sent) > Duration::from_secs(5) {
            p_now.store(new_playing, Ordering::Relaxed);
            p_push();
            *last_sent = now;
            let progress = Some(MediaPosition(Duration::from_secs_f64(pos.max(0.0))));
            let state = if new_playing {
                MediaPlayback::Playing { progress }
            } else {
                MediaPlayback::Paused { progress }
            };
            if let Some(c) = c_play.lock().unwrap().as_mut() {
                let _ = c.set_playback(state);
            }
        }
    });

    // Remote session → SMTC: when the network player is the active source,
    // its metadata and state own the Windows media panel and media keys.
    let c_remote = cell.clone();
    let r_now = playing_now.clone();
    let r_active = remote_active.clone();
    let r_local_move = last_local_move.clone();
    let r_until = optimistic_until.clone();
    let r_push = push_bar.clone();
    let r_last_meta = Arc::new(Mutex::new(String::new()));
    let r_last_sent = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(60)));
    app.listen_any("remote://state", move |event| {
        use std::sync::atomic::Ordering;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
            return;
        };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let playing = v.get("playing").and_then(|x| x.as_bool()).unwrap_or(false);
        let title = s("title");
        let local_recent = r_local_move.lock().unwrap().elapsed() < Duration::from_secs(3);
        let active = playing && !title.is_empty() && !local_recent;
        let was_active = r_active.swap(active, Ordering::Relaxed);
        REMOTE_ACTIVE.store(active, Ordering::Relaxed);
        if !active {
            if was_active {
                r_last_meta.lock().unwrap().clear();
                r_push(); // back to local: the Pandora-only buttons live again
            }
            return;
        }
        if !was_active {
            r_push(); // renderer took over: grey out what it can't do
        }

        let artist = s("artist");
        let album = s("album");
        let art = s("art");
        let meta_key = format!("{title}|{artist}|{album}|{art}");
        {
            let mut lm = r_last_meta.lock().unwrap();
            if *lm != meta_key {
                *lm = meta_key;
                if let Some(c) = c_remote.lock().unwrap().as_mut() {
                    fn opt(x: &str) -> Option<&str> {
                        if x.is_empty() {
                            None
                        } else {
                            Some(x)
                        }
                    }
                    let _ = c.set_metadata(MediaMetadata {
                        title: opt(&title),
                        artist: opt(&artist),
                        album: opt(&album),
                        cover_url: opt(&art),
                        duration: None,
                    });
                }
            }
        }

        let now = Instant::now();
        let was_playing = r_now.load(Ordering::Relaxed);
        if now < *r_until.lock().unwrap() && playing != was_playing {
            return; // optimistic grace after an SMTC button press
        }
        let mut send = playing != was_playing || !was_active;
        {
            let mut ls = r_last_sent.lock().unwrap();
            if now.duration_since(*ls) > Duration::from_secs(5) {
                send = true;
            }
            if send {
                *ls = now;
            }
        }
        if send {
            r_now.store(playing, Ordering::Relaxed);
            r_push();
            let pos = v.get("position").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let progress = Some(MediaPosition(Duration::from_secs_f64(pos.max(0.0))));
            let state = if playing {
                MediaPlayback::Playing { progress }
            } else {
                MediaPlayback::Paused { progress }
            };
            if let Some(c) = c_remote.lock().unwrap().as_mut() {
                let _ = c.set_playback(state);
            }
        }
    });

    Ok(())
}

/// On-demand update check (version-badge click). Returns the available
/// version, or None when current.
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    Ok(updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .map(|u| u.version.clone()))
}

/// Install now, because the badge was clicked.
///
/// Updates normally install themselves at the next gap between songs (see `updates.rs`),
/// so this only means "don't wait for the song to end". If one is already staged the
/// install is immediate and needs no network; otherwise fall back to fetching it first.
#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    // Does not return when it succeeds — the installer launches and the process exits.
    if updates::install_staged(&app) {
        return Err("install failed".into());
    }
    let updater = app.updater().map_err(|e| e.to_string())?;
    if let Some(update) = updater.check().await.map_err(|e| e.to_string())? {
        update
            .download_and_install(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())?;
        app.restart();
    }
    Ok(())
}

/// Mirrors the network-player takeover flag for code outside `setup_media_controls`,
/// which owns the `Arc` and threads it through a dozen closures. Read-only elsewhere.
static REMOTE_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Is a network renderer (WiiM/DLNA) currently driving playback?
pub(crate) fn remote_active() -> bool {
    REMOTE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set by the panic hook, drained into diagnostics once the app is up.
static LAST_PANIC: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // HardwareMediaKeyHandling is off so media keys reach our native SMTC session rather than
    // being swallowed by WebView2.
    //
    // The ANGLE/D3D11 backend was needed because pandora.com crashed WebView2's renderer
    // (STATUS_ACCESS_VIOLATION) under the default D3D11on12/GL path. We no longer load that page,
    // so this may well be unnecessary — but it costs nothing, still keeps hardware acceleration,
    // and dropping it is a graphics-stack gamble with no upside. Left deliberately.
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--use-angle=d3d11 --disable-features=HardwareMediaKeyHandling",
    );

    // Panics in background threads otherwise vanish into a stderr nobody sees. Recording them
    // means the next bug report carries the panic that caused the trouble.
    {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            LAST_PANIC
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .replace(info.to_string());
            previous(info);
        }));
    }

    tauri::Builder::default()
        // MUST be registered first, per the plugin's contract.
        //
        // A second copy is not merely untidy now that Jarlid owns playback itself: both instances
        // grab the audio device and both register their own SMTC session, so a media key or
        // taskbar press lands on whichever one Windows picked while the other keeps playing —
        // which reads as "play/pause is broken". Pandora also permits only one stream per
        // account, so the loser gets STREAM_VIOLATION. Focus the running copy instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Save dialog for the station-preferences export (driven from Rust).
        .plugin(tauri_plugin_dialog::init())
        .manage(export::ExportCtl::default())
        .manage(updates::UpdateCtl::default())
        .manage(diagnostics::Diagnostics::default())
        // Remember main-window position/size across launches.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            check_update,
            export::cancel_export,
            export::export_stations,
            fetch_lyrics,
            install_update,
            native::native_account,
            native::native_cmd,
            native::native_is_signed_in,
            native::native_play_station,
            native::native_sign_in,
            native::native_sign_out,
            native::native_stations,
            native::native_modes,
            native::native_take_over,
            diagnostics::native_report_issue,
            diagnostics::native_record_incident,
            native::native_set_mode,
            native::native_volume,
            player_cmd,
            remote_cmd,
            remote_presets
        ])
        .setup(|app| {
            // The native engine: speaks Pandora's protocol directly and plays audio itself.
            // It emits the same `engine://` events the webview bridge did, so the UI, SMTC
            // session, thumb toolbar and lyrics sync need no changes.
            app.manage(native::NativeEngine::default());
            if let Some(panic) = LAST_PANIC.lock().unwrap_or_else(|e| e.into_inner()).take() {
                app.state::<diagnostics::Diagnostics>()
                    .record("panic", &panic);
            }
            native::init(&app.handle().clone());

            // The engine webview is gone. Pandora's site is no longer loaded, scraped or
            // driven — `native::init` above speaks the protocol directly and plays the audio
            // itself. `engine://needs-login` now means "ask for credentials in our own UI"
            // rather than "reveal the Pandora page".

            // Updates stage themselves in the background and install at a track
            // boundary, so an update never cuts a song in half. See `updates.rs`.
            #[cfg(not(debug_assertions))]
            updates::spawn(&app.handle().clone());

            // Network (UPnP/DLNA) player watcher — feeds remote-mode overlay and
            // SMTC remote routing (must be managed before SMTC setup).
            let remote_ctl = upnp::RemoteCtl::new();
            app.manage(remote_ctl.clone());
            upnp::start(app.handle().clone(), remote_ctl);

            // Native Windows media session (media keys + volume-flyout panel).
            #[cfg(windows)]
            if let Err(e) = setup_media_controls(app) {
                eprintln!("[smtc] setup failed: {e}");
            }

            // (The engine-heartbeat watchdog lived here. It reloaded pandora.com when the
            // scraped page wedged — a failure mode that no longer exists now that playback is
            // native. Recovery for the native engine is a token refresh, which `Client` already
            // does inline.)

            // Dev-only: trace playhead events so the engine→host pipeline is
            // observable in the dev log (every ~5s).
            #[cfg(debug_assertions)]
            {
                use std::sync::atomic::{AtomicU64, Ordering};
                static N: AtomicU64 = AtomicU64::new(0);
                app.listen_any("engine://playhead", move |event| {
                    let n = N.fetch_add(1, Ordering::Relaxed);
                    if n % 10 == 0 {
                        eprintln!("[playhead #{n}] {}", event.payload());
                    }
                });
            }

            // Closing the main (UI) window quits the whole app — otherwise the
            // hidden engine window would keep the process alive with nothing visible.
            //
            // Window geometry is also saved ~1s after any move/resize (debounced):
            // the window-state plugin only writes on clean exit, so without this a
            // kill (installer upgrade, crash) would lose the position.
            if let Some(main) = app.get_webview_window("main") {
                use std::sync::{Arc, Mutex};
                use std::time::{Duration, Instant};
                use tauri_plugin_window_state::{AppHandleExt, StateFlags};

                let dirty: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
                let saver_dirty = dirty.clone();
                let saver_handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(500));
                    let due = {
                        let mut d = saver_dirty.lock().unwrap();
                        match *d {
                            Some(t) if t.elapsed() > Duration::from_millis(800) => {
                                *d = None;
                                true
                            }
                            _ => false,
                        }
                    };
                    if due {
                        // MUST be marshalled onto the main thread. `save_window_state`
                        // holds the plugin's state-cache mutex across `is_maximized()`,
                        // `outer_position()` etc.; called from any other thread each of
                        // those blocks on a round-trip to the event loop, which may
                        // itself be inside the plugin's own Moved/Resized handler
                        // waiting for that same mutex — a deadlock that freezes the
                        // whole app (diagnosed from a hang dump on v0.6.11).
                        // On the main thread tauri-runtime-wry's `send_user_message`
                        // short-circuits, so the getters resolve inline and the cache
                        // is only ever locked from one thread. `run_on_main_thread`
                        // itself only enqueues, so this loop never blocks.
                        let save_handle = saver_handle.clone();
                        let _ = saver_handle.run_on_main_thread(move || {
                            let _ = save_handle.save_window_state(StateFlags::all());
                        });
                    }
                });

                let handle = app.handle().clone();
                main.on_window_event(move |event| match event {
                    tauri::WindowEvent::CloseRequested { .. } => handle.exit(0),
                    tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                        *dirty.lock().unwrap() = Some(Instant::now());
                    }
                    _ => {}
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
