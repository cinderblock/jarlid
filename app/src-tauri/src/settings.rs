//! User settings that the Rust side has to read.
//!
//! Kept out of the webview's `localStorage` on purpose: the update loop decides whether to
//! stage and install long before (and sometimes without) the UI being involved, so the
//! answer has to live somewhere the backend can read directly.
//!
//! One small JSON file next to `last-station.json`. Unknown keys are preserved on write,
//! so a newer build's settings survive being opened by an older one.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Download and install updates on their own, in the gap between two songs.
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        // On by default: an unattended music player that quietly keeps itself current is
        // the behaviour asked for. The checkbox exists to opt *out*.
        Self { auto_update: true }
    }
}

/// Cached so the update loop can read the setting without touching the disk every tick.
#[derive(Default)]
pub struct SettingsCtl(Mutex<Option<Settings>>);

fn path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("settings.json"))
}

/// Current settings, reading from disk once and caching thereafter.
///
/// A missing or corrupt file is not an error worth surfacing — it just means defaults,
/// which is exactly what a first run should get.
pub fn get(app: &tauri::AppHandle) -> Settings {
    let ctl = app.state::<SettingsCtl>();
    if let Ok(guard) = ctl.0.lock() {
        if let Some(s) = guard.as_ref() {
            return s.clone();
        }
    }
    let loaded = path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
        .unwrap_or_default();
    if let Ok(mut guard) = ctl.0.lock() {
        *guard = Some(loaded.clone());
    }
    loaded
}

fn save(app: &tauri::AppHandle, next: &Settings) -> Result<(), String> {
    let p = path(app).ok_or_else(|| "no config directory".to_string())?;
    let text = serde_json::to_string_pretty(next).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("could not write {}: {e}", p.display()))?;
    if let Ok(mut guard) = app.state::<SettingsCtl>().0.lock() {
        *guard = Some(next.clone());
    }
    Ok(())
}

#[tauri::command]
pub fn get_settings(app: tauri::AppHandle) -> Settings {
    get(&app)
}

#[tauri::command]
pub fn set_auto_update(app: tauri::AppHandle, enabled: bool) -> Result<Settings, String> {
    let mut next = get(&app);
    next.auto_update = enabled;
    save(&app, &next)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Automatic updates are opt-*out*. A first run with no settings file must keep
    /// updating itself rather than silently going stale.
    #[test]
    fn defaults_to_automatic() {
        assert!(Settings::default().auto_update);
    }

    /// The file is the contract with the UI; the key has to stay camelCase.
    #[test]
    fn serialises_as_camel_case() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert_eq!(json, r#"{"autoUpdate":true}"#);
    }

    #[test]
    fn round_trips() {
        let off = Settings { auto_update: false };
        let text = serde_json::to_string(&off).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), off);
    }
}
