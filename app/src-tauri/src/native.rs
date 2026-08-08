//! The native Pandora engine, wired into the app in place of the engine webview.
//!
//! This emits **exactly the same `engine://` events** the injected `bridge.js` used to emit, so
//! the UI, SMTC session, taskbar thumb toolbar and lyrics sync all keep working untouched. The
//! webview is replaced; nothing downstream of it is.
//!
//! One thing genuinely improves: the old playhead had to *infer* paused state, because Pandora's
//! DOM and audio elements misreported it. Here we own the player, so `paused` is authoritative.

use std::sync::Arc;
use std::time::Duration;

use engine::{Engine, Event};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

/// How often to publish playback position. The UI drives synced lyrics off this, so it wants to
/// be smooth without flooding the event bus.
const PLAYHEAD_INTERVAL: Duration = Duration::from_millis(250);

/// Where the last-played station is remembered, so launching resumes what you were listening to
/// rather than whatever happens to sort first.
fn last_station_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("last-station.json"))
}

fn save_last_station(app: &AppHandle, name: &str, token: &str) {
    let Some(path) = last_station_path(app) else {
        return;
    };
    let blob = json!({ "name": name, "token": token });
    if let Ok(text) = serde_json::to_string(&blob) {
        let _ = std::fs::write(path, text);
    }
}

fn load_last_station(app: &AppHandle) -> Option<(String, String)> {
    let text = std::fs::read_to_string(last_station_path(app)?).ok()?;
    let blob: serde_json::Value = serde_json::from_str(&text).ok()?;
    let name = blob.get("name")?.as_str()?.to_string();
    let token = blob.get("token")?.as_str()?.to_string();
    Some((name, token))
}

/// Managed app state. `None` until sign-in succeeds.
#[derive(Clone, Default)]
pub struct NativeEngine(Arc<Mutex<Option<Arc<Engine>>>>);

impl NativeEngine {
    async fn get(&self) -> Result<Arc<Engine>, String> {
        self.0
            .lock()
            .await
            .clone()
            .ok_or_else(|| "not signed in".to_string())
    }

    /// The running engine, for code outside this module (the export walk).
    pub async fn engine(&self) -> Result<Arc<Engine>, String> {
        self.get().await
    }
}

/// Start the engine from saved credentials at launch, or ask the UI for a login.
pub fn init(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match Engine::start_from_saved().await {
            Ok((started, events)) => attach(&app, started, events).await,
            Err(engine::Error::NotSignedIn) => {
                // Normal first run — not an error worth logging as one.
                let _ = app.emit("engine://needs-login", json!({}));
            }
            Err(e) => {
                eprintln!("[native] could not start from saved credentials: {e}");
                let _ = app.emit("engine://needs-login", json!({}));
            }
        }
    });
}

/// Take ownership of a started engine: publish its events, drive the radio, tick the playhead.
async fn attach(
    app: &AppHandle,
    started: Engine,
    mut events: tokio::sync::mpsc::UnboundedReceiver<Event>,
) {
    let engine = Arc::new(started);
    *app.state::<NativeEngine>().0.lock().await = Some(Arc::clone(&engine));

    // Station list for the picker. The tokens go with it: switching station and
    // exporting one both need the token, and emitting names alone left the
    // picker unable to do either.
    if let Ok(stations) = engine.tuner_stations().await {
        let names: Vec<&String> = stations.iter().map(|(name, _)| name).collect();
        let tokens: Vec<&String> = stations.iter().map(|(_, token)| token).collect();
        let _ = app.emit(
            "engine://stations",
            json!({ "stations": names, "tokens": tokens }),
        );
    }

    // Engine events -> the `engine://` events the rest of the app already listens for.
    {
        let app = app.clone();
        let engine = Arc::clone(&engine);
        tauri::async_runtime::spawn(async move {
            let mut station = String::new();
            while let Some(event) = events.recv().await {
                match event {
                    Event::StationChanged(name) => station = name,
                    Event::TrackStarted(track) => {
                        // The UI wants a big image and a smaller fallback, exactly as before.
                        let art = |min: u32| {
                            pandora::models::art_at_least(track.art(), min)
                                .map(|a| a.url.clone())
                                .unwrap_or_default()
                        };
                        let _ = app.emit(
                            "engine://nowplaying",
                            json!({
                                "title": track.song_title,
                                "artist": track.artist_name,
                                "album": track.album_title,
                                "station": station,
                                "art": art(1080),
                                "artFallback": art(500),
                                // Pandora's own recorded feedback, not a guess. This is what
                                // reconciles an optimistic thumb: if the click didn't register,
                                // the next play of that track shows the truth.
                                "thumbUp": track.is_thumbed_up(),
                                // Thumbed-down tracks are simply not served, so a track we're
                                // playing is never thumbed down.
                                "thumbDown": false,
                            }),
                        );
                        let _ = app.emit(
                            "engine://thumbs",
                            json!({ "thumbUp": track.is_thumbed_up(), "thumbDown": false }),
                        );
                    }
                    Event::StreamTaken => {
                        let _ = app.emit(
                            "engine://error",
                            json!({ "message": "Pandora is playing on another device." }),
                        );
                    }
                    Event::Error(message) => {
                        let _ = app.emit("engine://error", json!({ "message": message }));
                    }
                    Event::TrackEnded | Event::Paused(_) => {}
                }
            }
            drop(engine);
        });
    }

    // Playhead ticker. Position comes from frames delivered to the audio device, so it is the
    // correct clock for lyric sync even though decoding runs seconds ahead.
    {
        let app = app.clone();
        let engine = Arc::clone(&engine);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(PLAYHEAD_INTERVAL).await;
                let duration = engine
                    .now_playing()
                    .await
                    .map(|t| t.track_length as f64)
                    .unwrap_or(0.0);
                let _ = app.emit(
                    "engine://playhead",
                    json!({
                        "position": engine.position().as_secs_f64(),
                        "duration": duration,
                        // Authoritative, unlike the DOM-scraping era's guesswork.
                        "paused": engine.is_paused(),
                        "volume": 1.0,
                    }),
                );
            }
        });
    }

    // Drive auto-advance between tracks.
    {
        let engine = Arc::clone(&engine);
        tauri::async_runtime::spawn(async move { engine.run().await });
    }

    // Resume the station you were last listening to. Falling back to the first station only
    // happens on a genuinely fresh install.
    let resume = match load_last_station(app) {
        // Confirm the saved station still exists — it may have been deleted elsewhere, and
        // playing a dead token just fails confusingly.
        Some((name, token)) => match engine.tuner_stations().await {
            Ok(stations) => stations
                .iter()
                .find(|(_, t)| *t == token)
                .cloned()
                .or_else(|| {
                    eprintln!("[native] saved station {name:?} no longer exists; using the first");
                    stations.first().cloned()
                }),
            Err(_) => Some((name, token)),
        },
        None => engine
            .tuner_stations()
            .await
            .ok()
            .and_then(|s| s.first().cloned()),
    };

    if let Some((name, token)) = resume {
        if let Err(e) = engine.play_station(&name, &token).await {
            eprintln!("[native] could not start playback: {e}");
        } else {
            save_last_station(app, &name, &token);
        }
    }
}

#[tauri::command]
pub async fn native_sign_in(
    app: AppHandle,
    username: String,
    password: String,
) -> Result<(), String> {
    // sign_in only persists credentials once a login actually succeeds, so a typo can't be saved.
    let (started, events) = Engine::sign_in(&username, &password)
        .await
        .map_err(|e| e.to_string())?;
    attach(&app, started, events).await;
    Ok(())
}

#[tauri::command]
pub async fn native_sign_out(state: tauri::State<'_, NativeEngine>) -> Result<(), String> {
    engine::credentials::clear().map_err(|e| e.to_string())?;
    *state.0.lock().await = None;
    Ok(())
}

#[tauri::command]
pub async fn native_is_signed_in() -> Result<bool, String> {
    Ok(engine::credentials::exists())
}

/// Station list for the picker: name plus the token playback needs.
#[tauri::command]
pub async fn native_stations(
    state: tauri::State<'_, NativeEngine>,
) -> Result<Vec<(String, String)>, String> {
    state
        .get()
        .await?
        .tuner_stations()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn native_play_station(
    app: AppHandle,
    state: tauri::State<'_, NativeEngine>,
    name: String,
    token: String,
) -> Result<(), String> {
    state
        .get()
        .await?
        .play_station(&name, &token)
        .await
        .map_err(|e| e.to_string())?;
    // Only remember it once playback actually started, so a failed switch doesn't become the
    // station we resume into next launch.
    save_last_station(&app, &name, &token);
    Ok(())
}

/// Transport, matching the command vocabulary the UI and media keys already use.
#[tauri::command]
pub async fn native_cmd(
    app: AppHandle,
    state: tauri::State<'_, NativeEngine>,
    cmd: String,
) -> Result<(), String> {
    let engine = state.get().await?;

    // Callers use several spellings: the UI sends "playpause", the media-key/taskbar dispatcher
    // sends camelCase ("thumbDown"). Normalise rather than making every caller agree.
    let normalized: String = cmd
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();

    match normalized.as_str() {
        "play" => engine.set_paused(false),
        "pause" => engine.set_paused(true),
        "playpause" => engine.toggle_pause(),
        "skip" | "next" => engine.skip().await.map_err(|e| e.to_string())?,
        // Thumbs are optimistic so the button responds instantly, but they reconcile: on failure
        // we roll the UI back immediately, and the next time Pandora serves the track its real
        // `songRating` overrides whatever we assumed.
        "thumbup" => {
            let _ = app.emit(
                "engine://thumbs",
                json!({ "thumbUp": true, "thumbDown": false }),
            );
            if let Err(e) = engine.thumb_up().await {
                let _ = app.emit(
                    "engine://thumbs",
                    json!({ "thumbUp": false, "thumbDown": false }),
                );
                return Err(e.to_string());
            }
        }
        "thumbdown" => {
            let _ = app.emit(
                "engine://thumbs",
                json!({ "thumbUp": false, "thumbDown": true }),
            );
            if let Err(e) = engine.thumb_down().await {
                let _ = app.emit(
                    "engine://thumbs",
                    json!({ "thumbUp": false, "thumbDown": false }),
                );
                return Err(e.to_string());
            }
            // Pandora moves on after a thumbs down, and so should we.
            engine.skip().await.map_err(|e| e.to_string())?;
        }
        "replay" => engine.replay().await.map_err(|e| e.to_string())?,
        other => return Err(format!("unknown command {other:?}")),
    }
    Ok(())
}

#[tauri::command]
pub async fn native_volume(
    state: tauri::State<'_, NativeEngine>,
    volume: f32,
) -> Result<(), String> {
    state.get().await?.set_volume(volume);
    Ok(())
}
