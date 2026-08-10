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

/// Shape the UI's station list expects. One place, so the picker, the Stations page and
/// the `native_stations` command can never disagree about it.
fn station_payload(stations: &[pandora::TunerStation]) -> serde_json::Value {
    serde_json::Value::Array(
        stations
            .iter()
            .map(|s| {
                json!({
                    "name": s.station_name,
                    "token": s.station_token,
                    "isQuickMix": s.is_quick_mix,
                    "isGenreStation": s.is_genre_station,
                    "isThumbprint": s.is_thumbprint,
                })
            })
            .collect::<Vec<_>>(),
    )
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

    // Station list for the picker and the Stations page. Tokens go with it (switching
    // station and exporting one both need the token), and so do Pandora's special-station
    // flags — the Stations page marks them, because a QuickMix has no seeds or thumbs of
    // its own and would otherwise look like a broken export.
    if let Ok(stations) = engine.station_list().await {
        let _ = app.emit(
            "engine://stations",
            json!({ "stations": station_payload(&stations) }),
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
                    Event::ModeChanged(name) => {
                        let _ = app.emit("engine://mode", json!({ "mode": name }));
                    }
                    Event::TrackStarted(track) => {
                        // The UI wants a big image and a smaller fallback, exactly as before.
                        let art = |min: u32| {
                            pandora::models::art_at_least(track.art(), min)
                                .map(|a| a.url.clone())
                                .unwrap_or_default()
                        };
                        // On QuickMix (and other blends) the track comes from one of the
                        // contributing stations. Naming it answers "what am I actually listening
                        // to?" — invisible otherwise. `None` on an ordinary station, where it
                        // would just repeat the station name.
                        let source = engine.source_station().await.unwrap_or_default();

                        let _ = app.emit(
                            "engine://nowplaying",
                            json!({
                                "title": track.song_title,
                                "artist": track.artist_name,
                                "album": track.album_title,
                                "station": station,
                                "sourceStation": source,
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
                        // Its own event, not a generic error: this one is recoverable and has an
                        // obvious action, so the UI offers to claim the stream instead of just
                        // reporting a failure.
                        let _ = app.emit(
                            "engine://stream-taken",
                            json!({
                                "message": "Pandora is playing on another device.",
                            }),
                        );
                    }
                    Event::Error(message) => {
                        // Keep it for the next bug report, not just for this toast.
                        app.state::<crate::diagnostics::Diagnostics>()
                            .record("engine", &message);
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
        Some((name, token)) => match engine.station_list().await {
            Ok(stations) => stations
                .iter()
                .find(|s| s.station_token == token)
                .map(|s| (s.station_name.clone(), s.station_token.clone()))
                .or_else(|| {
                    eprintln!("[native] saved station {name:?} no longer exists; using the first");
                    stations
                        .first()
                        .map(|s| (s.station_name.clone(), s.station_token.clone()))
                }),
            // Can't confirm it still exists, but a saved token is better than nothing.
            Err(_) => Some((name, token)),
        },
        None => engine.station_list().await.ok().and_then(|stations| {
            stations
                .first()
                .map(|s| (s.station_name.clone(), s.station_token.clone()))
        }),
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
) -> Result<serde_json::Value, String> {
    let stations = state
        .get()
        .await?
        .station_list()
        .await
        .map_err(|e| e.to_string())?;
    Ok(station_payload(&stations))
}

/// Which Pandora account is signed in, for the Settings page. `None` when nothing is
/// stored. Returns only the username — the password never leaves the credential store.
#[tauri::command]
pub async fn native_account() -> Result<Option<String>, String> {
    match engine::credentials::load() {
        Ok(saved) => Ok(saved.map(|c| c.username)),
        Err(e) => Err(e.to_string()),
    }
}

/// Modes for the station currently playing ("My Station", "Crowd Faves", "Deep Cuts", …).
///
/// Returns an empty list rather than an error when nothing is playing yet, so the UI can call
/// this freely without special-casing startup.
/// Claim the account's single stream for this device, then resume.
#[tauri::command]
pub async fn native_take_over(state: tauri::State<'_, NativeEngine>) -> Result<(), String> {
    state
        .get()
        .await?
        .take_over()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn native_modes(
    state: tauri::State<'_, NativeEngine>,
) -> Result<Vec<pandora::Mode>, String> {
    match state.get().await?.modes().await {
        Ok(modes) => Ok(modes),
        Err(engine::Error::NoStation) => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn native_set_mode(
    state: tauri::State<'_, NativeEngine>,
    mode_id: i64,
) -> Result<(), String> {
    state
        .get()
        .await?
        .set_mode(mode_id)
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
/// Callers use several spellings: the UI sends "playpause", the media-key/taskbar dispatcher
/// sends camelCase ("thumbDown") and its own verbs ("toggle"). Normalise rather than making
/// every caller agree.
pub fn normalize_command(cmd: &str) -> String {
    cmd.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Every command [`native_cmd`] understands, normalised.
///
/// Exists so the dispatchers can be checked against the handler in a test. A command that only
/// one side knows about fails *silently* — `engine_cmd` is called from OS callbacks with nowhere
/// to report to, so it can only log. That is exactly how the play/pause media key ended up doing
/// nothing: the dispatcher sent "toggle" and the handler only knew "playpause".
pub const COMMANDS: &[&str] = &[
    "play",
    "pause",
    "playpause",
    "toggle",
    "skip",
    "next",
    "prev",
    "previous",
    "replay",
    "thumbup",
    "thumbdown",
];

pub fn is_known_command(cmd: &str) -> bool {
    COMMANDS.contains(&normalize_command(cmd).as_str())
}

#[tauri::command]
pub async fn native_cmd(
    app: AppHandle,
    state: tauri::State<'_, NativeEngine>,
    cmd: String,
) -> Result<(), String> {
    let engine = state.get().await?;

    let normalized = normalize_command(&cmd);

    match normalized.as_str() {
        "play" => engine.set_paused(false),
        "pause" => engine.set_paused(true),
        // "toggle" is what the media-key and taskbar dispatcher sends (Action::Toggle); the UI
        // sends "playpause". Both must work — omitting "toggle" made the play/pause media key
        // and the taskbar play/pause button silently do nothing, since engine_cmd only logs the
        // resulting "unknown command" and has no way to report it.
        "playpause" | "toggle" => engine.toggle_pause(),
        // Pandora radio has no previous-track; the media Previous key restarts the song, which is
        // what the transport's replay button does too.
        "prev" | "previous" => engine.replay().await.map_err(|e| e.to_string())?,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact strings `setup_media_controls`' dispatcher sends for each `Action`, plus the
    /// taskbar toolbar's buttons, which route through the same actions.
    ///
    /// If someone adds an Action and forgets the handler arm, this fails instead of the feature
    /// quietly doing nothing.
    #[test]
    fn every_dispatched_command_is_handled() {
        for cmd in [
            "play",      // Action::Play
            "pause",     // Action::Pause
            "toggle",    // Action::Toggle  <- media play/pause key, taskbar play/pause
            "skip",      // Action::Next
            "replay",    // Action::Prev
            "thumbUp",   // Action::ThumbUp   (camelCase on purpose)
            "thumbDown", // Action::ThumbDown (camelCase on purpose)
            "playpause", // the UI's spelling
        ] {
            assert!(is_known_command(cmd), "no handler arm for {cmd:?}");
        }
    }

    #[test]
    fn normalizes_case_and_punctuation() {
        assert_eq!(normalize_command("thumbDown"), "thumbdown");
        assert_eq!(normalize_command("play-pause"), "playpause");
        assert_eq!(normalize_command("PLAY"), "play");
    }

    /// A genuinely unknown command must stay unknown, or the test above proves nothing.
    #[test]
    fn rejects_unknown_commands() {
        assert!(!is_known_command("frobnicate"));
        assert!(!is_known_command(""));
    }
}
