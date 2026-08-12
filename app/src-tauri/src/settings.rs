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

/// Which colours the app draws itself in.
///
/// Nothing in Rust reads this — the webview does the painting — but it lives here
/// with the rest of the settings so there is one file that *is* the preferences,
/// rather than one file and a webview storage bucket that has to agree with it.
/// The webview keeps a copy in `localStorage` purely so it can paint the first
/// frame without waiting for a round trip; this is what decides.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    /// Follow the Windows app-colour setting, and keep following it when it changes.
    #[default]
    System,
    Light,
    Dark,
}

/// Jarlid's own output level, 0-100, where 100 is the decoded signal untouched.
///
/// This exists so the music can sit *below* the rest of the machine: turn Windows up for
/// everything else, and keep the radio at a level you would actually leave running. Windows'
/// per-app mixer can do the same thing, but it is three dialogs away, invisible from here,
/// and resets its mind when the output device changes.
///
/// A percentage rather than a raw gain because it is what the slider shows, and because
/// storing the number the user chose leaves the *curve* free to be corrected later without
/// silently reinterpreting everyone's saved setting.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(transparent)]
pub struct Volume(u8);

/// Full volume. Anything else would mean installing an update could quietly turn the music
/// down, and a settings file written before this existed must sound exactly as it did.
impl Default for Volume {
    fn default() -> Self {
        Self(100)
    }
}

impl Volume {
    pub fn new(percent: u8) -> Self {
        Self(percent.min(100))
    }

    /// The stored percentage, clamped — a hand-edited file saying `250` is not a licence to
    /// blow the headroom.
    pub fn percent(self) -> u8 {
        self.0.min(100)
    }

    /// The linear gain to hand the audio device.
    ///
    /// A **constant-dB fader**: the whole travel spans [`FADER_RANGE_DB`], so every 1 % is the
    /// same 0.6 dB wherever you grab the slider. That evenness is the point, and it is what a
    /// power law cannot give — `x³` moves 0.26 dB per step near the top, so the last stretch
    /// feels dead under the hand, and nearly 3 dB per step down at 10 %, so the bottom is
    /// twitchy. 0.6 dB also sits just above the smallest change most people can hear, which
    /// makes every position on the slider a distinct one rather than a rounding of its
    /// neighbour. Chosen by ear against linear, `x^1.5`, `x²`, `x³` and a 40 dB fader.
    ///
    /// Exact at both ends by construction. 100 must be *precisely* 1.0 or everyone who never
    /// opened the setting would be quietly attenuated; 0 is special-cased to true silence,
    /// because a constant-dB curve's real bottom is −∞ dB and never arrives.
    pub fn amplitude(self) -> f32 {
        let p = self.percent() as f32 / 100.0;
        if p <= 0.0 {
            0.0
        } else {
            10f32.powf((p - 1.0) * FADER_RANGE_DB / 20.0)
        }
    }
}

/// How much range the volume slider spans, end to end.
///
/// 60 dB is roughly a mixing desk's travel. Below about 15 % it is already past −50 dB and
/// inaudible, so a seventh of the slider is effectively dead — the price of keeping every
/// audible step the same size, which is the property actually being bought here.
const FADER_RANGE_DB: f32 = 60.0;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub update_policy: Policy,
    pub check_schedule: CheckSchedule,
    pub theme: Theme,
    pub volume: Volume,
    /// Which output endpoint to play on. `None` means "whatever Windows currently calls the
    /// default", and it is genuinely *followed* — change the default mid-song and the music
    /// moves with it. A name that is not present right now is kept rather than rewritten, so
    /// plugging the device back in restores the choice.
    pub output_device: Option<String>,
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
            r#"{"updatePolicy":"afterSong","checkSchedule":{"kind":"every","minutes":30},"theme":"system","volume":100,"outputDevice":null}"#
        );

        let daily = Settings {
            update_policy: Policy::NotifyOnly,
            check_schedule: CheckSchedule::DailyAt {
                time: "03:30".into(),
            },
            theme: Theme::Light,
            volume: Volume::new(60),
            output_device: Some("Speakers (USB DAC)".into()),
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
        // Written before the theme setting existed: follow the system, as a fresh
        // install would, rather than refusing to load the file.
        assert_eq!(s.theme, Theme::System);
        // And written before there was a volume: full, not the 0 a derived Default would
        // have given a `u8`. Getting this wrong updates someone into silence.
        assert_eq!(s.volume, Volume::new(100));
        assert_eq!(s.volume.amplitude(), 1.0);
        // No stored device means follow the Windows default, which is what every install
        // predating the setting was already doing.
        assert_eq!(s.output_device, None);

        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Settings::default());
    }

    /// A chosen device is remembered verbatim, including one that is not plugged in — the
    /// fallback to the default happens at open time and must not rewrite the preference.
    #[test]
    fn an_absent_output_device_is_still_remembered() {
        let name = "Speakers (Scarlett 2i2 USB)";
        let s: Settings = serde_json::from_str(&format!(r#"{{"outputDevice":"{name}"}}"#)).unwrap();
        assert_eq!(s.output_device.as_deref(), Some(name));

        // And survives a round trip, so saving any other setting cannot drop it.
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), s);
    }

    /// The two ends have to be exact: full means untouched, and zero means silent. A curve
    /// that only approximately reaches 1.0 would attenuate everyone who never opened the
    /// setting at all.
    #[test]
    fn the_ends_of_the_volume_range_are_exact() {
        assert_eq!(Volume::new(100).amplitude(), 1.0);
        assert_eq!(Volume::new(0).amplitude(), 0.0);
    }

    /// The property the whole taper was chosen for: every 1 % of travel is the same number
    /// of dB, so the slider feels identical wherever it is grabbed. A power law passes a
    /// "not linear" test just as well and fails this one badly, which is why the assertion
    /// is about the *step* rather than about any single point.
    #[test]
    fn every_step_of_the_fader_is_the_same_size() {
        let db = |p: u8| 20.0 * Volume::new(p).amplitude().log10();
        let expected = FADER_RANGE_DB / 100.0; // 0.6 dB
        for p in 1..100u8 {
            let step = db(p + 1) - db(p);
            assert!(
                (step - expected).abs() < 0.01,
                "step from {p}% is {step} dB, not {expected}"
            );
        }
        // And the travel really spans the range it claims to.
        assert!((db(1) + FADER_RANGE_DB * 0.99).abs() < 0.01, "{}", db(1));
        assert!((db(50) + 30.0).abs() < 0.01, "{}", db(50));

        // Monotonic, or the slider would fight the hand somewhere along it.
        for p in 1..=100u8 {
            assert!(
                Volume::new(p).amplitude() > Volume::new(p - 1).amplitude(),
                "not increasing at {p}"
            );
        }
    }

    /// A value from outside this build — a hand-edited file, or one written by a version
    /// that allowed boosting — must not become gain above unity, which would clip.
    #[test]
    fn an_out_of_range_volume_is_clamped_rather_than_trusted() {
        let s: Settings = serde_json::from_str(r#"{"volume":250}"#).unwrap();
        assert_eq!(s.volume.percent(), 100);
        assert_eq!(s.volume.amplitude(), 1.0);
        assert_eq!(Volume::new(200).percent(), 100);
    }
}
