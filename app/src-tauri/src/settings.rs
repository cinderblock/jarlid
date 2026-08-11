//! User settings that the Rust side has to read.
//!
//! Kept out of the webview's `localStorage` on purpose: the update loop decides whether to
//! check, download and install long before (and sometimes entirely without) the UI being
//! involved, so the answer has to live somewhere the backend can read directly.
//!
//! One small JSON file next to `last-station.json`.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::Manager;

/// What happens once a newer version exists.
///
/// These are four genuinely different behaviours rather than degrees of one, which is why
/// this is not a checkbox: the axes are *when do we download* and *when do we install*, and
/// only three of the four combinations are useful (nobody wants "install without
/// downloading").
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Policy {
    /// Download, then install as soon as it is ready — cutting the current song.
    Instant,
    /// Download, then install in the gap after the current song. The default.
    #[default]
    AfterSong,
    /// Download, but install nothing until asked. The first request schedules it for the
    /// end of the current song; asking again means now.
    ManualInstall,
    /// Do not download. Just say a new version exists.
    NotifyOnly,
}

impl Policy {
    /// Should the background loop download without being asked?
    pub fn downloads_automatically(self) -> bool {
        !matches!(self, Policy::NotifyOnly)
    }

    /// Is a staged update armed to install on its own, or does it wait to be asked?
    pub fn arms_automatically(self) -> bool {
        matches!(self, Policy::Instant | Policy::AfterSong)
    }
}

/// How often to look for a new version.
///
/// `DailyAt` is a wall-clock time rather than an interval so the restart lands at a
/// predictable hour — the point being to choose one you are not listening at.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CheckSchedule {
    Never,
    Every { minutes: u32 },
    DailyAt { time: String },
}

impl Default for CheckSchedule {
    fn default() -> Self {
        Self::Every { minutes: 30 }
    }
}

impl CheckSchedule {
    /// Minutes until the next check, given the current local wall clock as
    /// `(hour, minute)`. `None` means never.
    ///
    /// Split out from any clock reading so the schedule arithmetic — the part that is easy
    /// to get wrong around midnight — can be tested without touching the system time.
    pub fn minutes_until_next(&self, now_h: u32, now_m: u32) -> Option<u32> {
        match self {
            CheckSchedule::Never => None,
            CheckSchedule::Every { minutes } => Some((*minutes).max(1)),
            CheckSchedule::DailyAt { time } => {
                let (h, m) = parse_hhmm(time).unwrap_or((3, 0));
                let target = h * 60 + m;
                let now = now_h * 60 + now_m;
                // Same time today means a full day away, not zero — otherwise it would
                // re-check every tick for a whole minute.
                Some(if target > now {
                    target - now
                } else {
                    24 * 60 - (now - target)
                })
            }
        }
    }
}

/// "HH:MM" → (hours, minutes), rejecting anything out of range.
fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub update_policy: Policy,
    pub check_schedule: CheckSchedule,
}

/// Cached so the update loop can read settings without touching disk every tick.
#[derive(Default)]
pub struct SettingsCtl(Mutex<Option<Settings>>);

fn path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("settings.json"))
}

/// Current settings, reading from disk once and caching thereafter.
///
/// A missing or unparseable file just means defaults, which is what a first run should
/// get — not an error worth surfacing.
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

/// Replace the whole settings object. One command rather than one per field: the UI holds
/// the complete state anyway, and partial setters drift out of sync with the struct.
#[tauri::command]
pub fn set_settings(app: tauri::AppHandle, settings: Settings) -> Result<Settings, String> {
    save(&app, &settings)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Updating in the gap after a song is the default: it keeps itself current without
    /// ever interrupting a track.
    #[test]
    fn defaults_are_automatic_and_polite() {
        let s = Settings::default();
        assert_eq!(s.update_policy, Policy::AfterSong);
        assert_eq!(s.check_schedule, CheckSchedule::Every { minutes: 30 });
        assert!(s.update_policy.downloads_automatically());
        assert!(s.update_policy.arms_automatically());
    }

    #[test]
    fn notify_only_never_downloads() {
        assert!(!Policy::NotifyOnly.downloads_automatically());
        assert!(!Policy::NotifyOnly.arms_automatically());
    }

    /// Manual-install still downloads ahead of time — that is the whole point of it, so
    /// that saying yes is instant rather than starting a download.
    #[test]
    fn manual_install_downloads_but_waits_to_be_asked() {
        assert!(Policy::ManualInstall.downloads_automatically());
        assert!(!Policy::ManualInstall.arms_automatically());
    }

    #[test]
    fn never_means_never() {
        assert_eq!(CheckSchedule::Never.minutes_until_next(12, 0), None);
    }

    #[test]
    fn intervals_are_just_themselves() {
        let every = CheckSchedule::Every { minutes: 240 };
        assert_eq!(every.minutes_until_next(12, 0), Some(240));
        // A zero interval would spin; clamp it.
        assert_eq!(
            CheckSchedule::Every { minutes: 0 }.minutes_until_next(12, 0),
            Some(1)
        );
    }

    /// The awkward part of a wall-clock schedule is the wrap around midnight.
    #[test]
    fn daily_waits_for_the_next_occurrence() {
        let at3am = CheckSchedule::DailyAt {
            time: "03:00".into(),
        };
        // Later today.
        assert_eq!(at3am.minutes_until_next(1, 0), Some(120));
        // Already past: tomorrow, not a negative or a zero.
        assert_eq!(at3am.minutes_until_next(4, 0), Some(23 * 60));
        // Exactly now counts as a full day, or it would re-fire every tick for a minute.
        assert_eq!(at3am.minutes_until_next(3, 0), Some(24 * 60));
        // Just before midnight, target early morning.
        assert_eq!(at3am.minutes_until_next(23, 30), Some(3 * 60 + 30));
    }

    /// A malformed time must not panic or disable checking; fall back to 03:00.
    #[test]
    fn a_broken_time_falls_back_rather_than_failing() {
        let bad = CheckSchedule::DailyAt {
            time: "not a time".into(),
        };
        assert_eq!(bad.minutes_until_next(1, 0), Some(120));
        assert_eq!(parse_hhmm("25:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("07:05"), Some((7, 5)));
    }

    /// The file is the contract with the UI; these names are load-bearing.
    #[test]
    fn serialises_as_the_ui_expects() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert_eq!(
            json,
            r#"{"updatePolicy":"afterSong","checkSchedule":{"kind":"every","minutes":30}}"#
        );

        let daily = Settings {
            update_policy: Policy::NotifyOnly,
            check_schedule: CheckSchedule::DailyAt {
                time: "03:30".into(),
            },
        };
        let text = serde_json::to_string(&daily).unwrap();
        assert!(text.contains(r#""updatePolicy":"notifyOnly""#), "{text}");
        assert!(
            text.contains(r#""kind":"dailyAt","time":"03:30""#),
            "{text}"
        );
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), daily);
    }

    /// An older file, or one written by a newer build, must still load — missing fields
    /// fall back to defaults rather than throwing the whole file away.
    #[test]
    fn partial_files_load_with_defaults() {
        let s: Settings = serde_json::from_str(r#"{"updatePolicy":"instant"}"#).unwrap();
        assert_eq!(s.update_policy, Policy::Instant);
        assert_eq!(s.check_schedule, CheckSchedule::default());

        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Settings::default());
    }
}
