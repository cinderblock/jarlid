use serde::Serialize;
use tauri::{Listener, Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Serialize, Default)]
struct Lyrics {
    synced: Option<String>,
    plain: Option<String>,
    source: String,
}

/// Fetch lyrics from LRCLIB. Tries the exact `get` endpoint first (best match, needs
/// duration), then falls back to `search`. Returns synced (LRC) and/or plain text.
#[tauri::command]
async fn fetch_lyrics(
    artist: String,
    track: String,
    album: Option<String>,
    duration: Option<f64>,
) -> Result<Lyrics, String> {
    let client = reqwest::Client::builder()
        .user_agent("PandoraDesktop (personal; https://github.com/cinderblock)")
        .build()
        .map_err(|e| e.to_string())?;

    // 1) exact match
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
                    return Ok(from_lrclib(&v, "lrclib/get"));
                }
            }
        }
    }

    // 2) fuzzy search, take the first hit
    let resp = client
        .get("https://lrclib.net/api/search")
        .query(&[("track_name", &track), ("artist_name", &artist)])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Ok(Lyrics {
            source: "none".into(),
            ..Default::default()
        });
    }
    let arr = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(first) = arr.as_array().and_then(|a| a.first()) {
        return Ok(from_lrclib(first, "lrclib/search"));
    }
    Ok(Lyrics {
        source: "none".into(),
        ..Default::default()
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
#[tauri::command]
fn player_cmd(app: tauri::AppHandle, cmd: String) -> Result<(), String> {
    let engine = app
        .get_webview_window("engine")
        .ok_or_else(|| "engine window not found".to_string())?;
    let arg = serde_json::to_string(&cmd).unwrap_or_else(|_| "\"\"".into());
    let js = format!(
        "window.__PANDORA_BRIDGE__ && window.__PANDORA_BRIDGE__.cmd({});",
        arg
    );
    engine.eval(&js).map_err(|e| e.to_string())
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
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--disable-gpu --disable-gpu-compositing --autoplay-policy=no-user-gesture-required",
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            fetch_lyrics,
            player_cmd,
            show_engine
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
