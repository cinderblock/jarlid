use serde::Serialize;
use tauri::{Listener, Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Serialize, Default)]
struct Lyrics {
    synced: Option<String>,
    plain: Option<String>,
    source: String,
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
    artist: String,
    track: String,
    album: Option<String>,
    duration: Option<f64>,
) -> Result<Lyrics, String> {
    let client = reqwest::Client::builder()
        .user_agent("PandoraDesktop (personal; https://github.com/cinderblock/pandora-desktop)")
        .build()
        .map_err(|e| e.to_string())?;

    let artist = undouble(&artist);
    let track = undouble(&track);
    let album = album.map(|a| undouble(&a));

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
                    return Ok(from_lrclib(&v, "lrclib/get"));
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
                        return Ok(from_lrclib(best, &format!("lrclib/search/{label}")));
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

    let handle = app.handle().clone();
    let cb_cell = cell.clone();
    let cb_playing = playing_now.clone();
    let cb_until = optimistic_until.clone();
    controls
        .attach(move |event: MediaControlEvent| {
            use std::sync::atomic::Ordering;
            let (cmd, desired) = match event {
                MediaControlEvent::Play => ("play", Some(true)),
                MediaControlEvent::Pause => ("pause", Some(false)),
                MediaControlEvent::Toggle => {
                    ("toggle", Some(!cb_playing.load(Ordering::Relaxed)))
                }
                MediaControlEvent::Next => ("skip", None),
                MediaControlEvent::Previous => ("replay", None),
                MediaControlEvent::Stop => ("pause", Some(false)),
                _ => return,
            };
            let _ = engine_cmd(&handle, cmd);
            if let Some(p) = desired {
                cb_playing.store(p, Ordering::Relaxed);
                *cb_until.lock().unwrap() = Instant::now() + Duration::from_secs(2);
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

    // Track metadata from the bridge.
    let c_meta = cell.clone();
    app.listen_any("engine://nowplaying", move |event| {
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
            new_playing = true;
        } else if now.duration_since(*last_move) > Duration::from_millis(1600) {
            new_playing = false;
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
    // WebView2 was crashing on pandora.com (STATUS_ACCESS_VIOLATION) — a native renderer
    // crash fixed by disabling GPU acceleration (that, not the autoplay flag, was the
    // cause). With GPU disabled, the autoplay-policy flag is safe and lets the UI's play
    // button start audio in the hidden engine without an in-page user gesture.
    // HardwareMediaKeyHandling is disabled so WebView2 can't intercept media keys —
    // the native SMTC session (souvlaki) owns them instead.
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--disable-gpu --disable-gpu-compositing --autoplay-policy=no-user-gesture-required --disable-features=HardwareMediaKeyHandling",
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Remember main-window position/size across launches. The engine window
        // is excluded — it's a hidden background window with managed visibility.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_denylist(&["engine"])
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            fetch_lyrics,
            player_cmd,
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
            if let Some(main) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                main.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { .. } = event {
                        handle.exit(0);
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
