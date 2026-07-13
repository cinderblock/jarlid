mod upnp;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Listener, Manager, WebviewUrl, WebviewWindowBuilder};

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
        if let Ok(resp) = client.get("https://lrclib.net/api/get").query(&q).send().await {
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
        sa.cmp(&sb).then(ga.partial_cmp(&gb).unwrap_or(std::cmp::Ordering::Equal))
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

/// Drive the Pandora engine webview by calling the injected bridge.
fn engine_cmd(app: &tauri::AppHandle, cmd: &str) -> Result<(), String> {
    let engine = app
        .get_webview_window("engine")
        .ok_or_else(|| "engine window not found".to_string())?;
    let arg = serde_json::to_string(cmd).unwrap_or_else(|_| "\"\"".into());
    let js = format!(
        "window.__PANDORA_BRIDGE__ && window.__PANDORA_BRIDGE__.cmd({});",
        arg
    );
    engine.eval(&js).map_err(|e| e.to_string())
}

#[tauri::command]
fn player_cmd(app: tauri::AppHandle, cmd: String) -> Result<(), String> {
    engine_cmd(&app, &cmd)
}

/// Transport command for the network (UPnP/DLNA) player shown in remote mode.
#[tauri::command]
async fn remote_cmd(
    ctl: tauri::State<'_, upnp::RemoteCtl>,
    cmd: String,
) -> Result<(), String> {
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

    let handle = app.handle().clone();
    let cb_cell = cell.clone();
    let cb_playing = playing_now.clone();
    let cb_until = optimistic_until.clone();
    let cb_remote_active = remote_active.clone();
    let cb_ctl = ctl.clone();
    controls
        .attach(move |event: MediaControlEvent| {
            use std::sync::atomic::Ordering;
            let cur = cb_playing.load(Ordering::Relaxed);
            let (engine_c, remote_c, desired): (&str, &str, Option<bool>) = match event {
                MediaControlEvent::Play => ("play", "play", Some(true)),
                MediaControlEvent::Pause => ("pause", "pause", Some(false)),
                MediaControlEvent::Toggle => {
                    ("toggle", if cur { "pause" } else { "play" }, Some(!cur))
                }
                MediaControlEvent::Next => ("skip", "skip", None),
                MediaControlEvent::Previous => ("replay", "prev", None),
                MediaControlEvent::Stop => ("pause", "pause", Some(false)),
                _ => return,
            };
            if cb_remote_active.load(Ordering::Relaxed) {
                let ctl2 = cb_ctl.clone();
                let rc = remote_c.to_string();
                tauri::async_runtime::spawn(async move {
                    let client = upnp::device_client();
                    let _ = upnp::command(&client, &ctl2, &rc).await;
                });
            } else {
                let _ = engine_cmd(&handle, engine_c);
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
            }
        })
        .map_err(|e| format!("SMTC attach: {e:?}"))?;
    *cell.lock().unwrap() = Some(controls);

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
        if !active {
            if was_active {
                r_last_meta.lock().unwrap().clear();
            }
            return;
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

/// Download and install a pending update, then restart. The startup check only
/// notifies; this runs when the user clicks the banner.
#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
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

/// Toggle the engine window's visibility. Returns the new visibility.
#[tauri::command]
fn toggle_engine(app: tauri::AppHandle) -> Result<bool, String> {
    let w = app
        .get_webview_window("engine")
        .ok_or_else(|| "engine window not found".to_string())?;
    let visible = w.is_visible().unwrap_or(false);
    if visible {
        let _ = w.hide();
    } else {
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(!visible)
}

/// Show or hide the raw Pandora engine window (used for login / debugging).
#[tauri::command]
fn show_engine(app: tauri::AppHandle, visible: bool) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("engine") {
        if visible {
            let _ = w.show();
            let _ = w.set_focus();
        } else {
            let _ = w.hide();
        }
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // pandora.com crashed WebView2's renderer (STATUS_ACCESS_VIOLATION) with default
    // GPU settings. Full --disable-gpu fixed it but forced software rendering; the
    // D3D11 ANGLE backend keeps hardware acceleration while avoiding the default
    // (D3D11on12/GL mix) path that crashed. If STATUS_ACCESS_VIOLATION ever returns,
    // fall back to "--disable-gpu --disable-gpu-compositing".
    // autoplay-policy lets the UI start audio in the hidden engine without an
    // in-page gesture; HardwareMediaKeyHandling is off so media keys reach our
    // native SMTC session instead of the webview.
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--use-angle=d3d11 --autoplay-policy=no-user-gesture-required --disable-features=HardwareMediaKeyHandling",
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Remember main-window position/size across launches. The engine window
        // is excluded — it's a hidden background window with managed visibility.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_denylist(&["engine"])
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            check_update,
            fetch_lyrics,
            install_update,
            player_cmd,
            remote_cmd,
            remote_presets,
            show_engine,
            toggle_engine
        ])
        .setup(|app| {
            let bridge = include_str!("../bridge.js");
            let engine = WebviewWindowBuilder::new(
                app,
                "engine",
                WebviewUrl::External("https://www.pandora.com".parse().unwrap()),
            )
            .title("Pandora Engine")
            .inner_size(1000.0, 720.0)
            .initialization_script(bridge)
            // Hidden engine (intended design). Shown only when a login is needed.
            .visible(false)
            .build()?;

            // The engine is a background window: closing it would destroy the
            // WebView2 and stop playback. Intercept close and just hide it instead.
            let engine_for_close = engine.clone();
            engine.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = engine_for_close.hide();
                }
            });

            // Show the engine window only when Pandora needs a login.
            let h1 = app.handle().clone();
            app.listen_any("engine://needs-login", move |_| {
                if let Some(w) = h1.get_webview_window("engine") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            });
            // Re-hide the raw window once a track is playing (e.g. after a login).
            let h2 = app.handle().clone();
            app.listen_any("engine://nowplaying", move |_| {
                if let Some(w) = h2.get_webview_window("engine") {
                    let _ = w.hide();
                }
            });

            // Update check (release builds): notify the UI when a newer GitHub
            // release exists; the banner's button runs install_update.
            #[cfg(not(debug_assertions))]
            {
                use tauri_plugin_updater::UpdaterExt;
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // First check shortly after launch, then every 4 hours so
                    // long-running instances still learn about new releases.
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    loop {
                        if let Ok(updater) = handle.updater() {
                            match updater.check().await {
                                Ok(Some(update)) => {
                                    eprintln!("[updater] v{} available", update.version);
                                    let _ = handle
                                        .emit("app://update-available", update.version.clone());
                                }
                                Ok(None) => {}
                                Err(e) => eprintln!("[updater] check failed: {e}"),
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(4 * 3600)).await;
                    }
                });
            }

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

            // Watchdog: the bridge emits engine://heartbeat every 5s. If the engine
            // page wedges (renderer crash, stuck navigation), heartbeats stop and we
            // reload pandora.com automatically instead of requiring a manual refresh.
            {
                use std::sync::{Arc, Mutex};
                use std::time::{Duration, Instant};
                let last_beat = Arc::new(Mutex::new(Instant::now()));
                let beat = last_beat.clone();
                app.listen_any("engine://heartbeat", move |_| {
                    *beat.lock().unwrap() = Instant::now();
                });
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(10));
                    let silent = last_beat.lock().unwrap().elapsed();
                    if silent > Duration::from_secs(30) {
                        eprintln!("[watchdog] engine silent for {silent:?} — reloading");
                        if let Some(w) = handle.get_webview_window("engine") {
                            let _ = w.navigate("https://www.pandora.com".parse().unwrap());
                        }
                        *last_beat.lock().unwrap() = Instant::now();
                    }
                });
            }

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
                        let _ = saver_handle.save_window_state(StateFlags::all());
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
