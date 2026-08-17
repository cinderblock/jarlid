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

/// Which end of the recently-played strip the newest song sits at.
///
/// Like [`Theme`], nothing in Rust reads this — the webview draws the strip — but the
/// preference belongs with the rest of them rather than in a webview storage bucket that has
/// to be kept in agreement.
///
/// An enum rather than a `reversed: bool`, because "reversed" does not say reversed *from
/// what*: the answer is only obvious to whoever wrote the default, and it stops being obvious
/// the moment the default moves.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum RecentsOrder {
    /// Newest at the right-hand end, the way the Pandora app does it, so the strip reads as
    /// time running left to right and the song that just finished is nearest the transport.
    #[default]
    NewestRight,
    /// Newest at the left-hand end.
    NewestLeft,
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
    /// Which way round the recently-played strip runs.
    pub recents_order: RecentsOrder,
    pub volume: Volume,
    /// How one song gives way to the next.
    pub blend: Blend,
    /// Which output endpoint to play on. `None` means "whatever Windows currently calls the
    /// default", and it is genuinely *followed* — change the default mid-song and the music
    /// moves with it. A name that is not present right now is kept rather than rewritten, so
    /// plugging the device back in restores the choice.
    pub output_device: Option<String>,
}

/// How one song gives way to the next.
///
/// Three genuinely different behaviours rather than degrees of one, so this is a choice and not
/// a checkbox: whether the songs overlap at all, and whether we are willing to bend a tempo to
/// make the overlap line up.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum BlendMode {
    /// One song ends, the next begins. What a radio has always done.
    #[default]
    Off,
    /// Overlap them and fade across. No tempo is touched, so two songs at different speeds
    /// simply play over each other for a few seconds.
    Crossfade,
    /// Overlap them *and* pull the incoming track's tempo onto the outgoing one's so the beats
    /// line up. When the two are further apart than [`Blend::max_pull_percent`] allows, there is
    /// no blend at all — a normal transition is better than a bad mix.
    BeatMatched,
}

/// Settings for [`BlendMode`].
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Blend {
    pub mode: BlendMode,
    /// How long the two songs overlap.
    pub seconds: f32,
    /// How far a tempo may be pulled, as a percentage of it.
    ///
    /// This is a DJ pitch-fader range, and a percentage rather than an absolute BPM because that
    /// is what the ear cares about: ±6% is the same musical stretch at 90 BPM as at 160, where
    /// "±8 BPM" would be a shrug at one end and a lurch at the other. ±6/±10/±16 are the ranges
    /// a CDJ offers, for the same reason.
    ///
    /// It doubles as the decision to blend at all in [`BlendMode::BeatMatched`]: two tracks
    /// further apart than this are left alone.
    pub max_pull_percent: f32,
    /// After the blend finishes, glide the incoming track back to its own tempo.
    ///
    /// Worth having on. A pull of a few percent spread over half a minute is well under a cent
    /// per second — inaudible — and without it every track plays at the speed of whatever
    /// happened to precede it, for its whole length.
    pub restore_tempo: bool,
}

/// Off, because it changes how *every* song ends. Someone who wants it will go and ask.
impl Default for Blend {
    fn default() -> Self {
        Self {
            mode: BlendMode::Off,
            seconds: 5.0,
            max_pull_percent: 6.0,
            restore_tempo: true,
        }
    }
}

impl Blend {
    /// The overlap, clamped. Below a couple of seconds it is a cut rather than a blend; above
    /// twenty it eats whole endings, and it cannot exceed what we pre-buffer anyway.
    pub fn seconds(&self) -> f32 {
        self.seconds.clamp(2.0, 20.0)
    }

    /// The permitted pull as a fraction. Clamped to a real pitch-fader range — beyond about 16%
    /// the shift stops reading as tempo and starts reading as a different singer.
    pub fn max_pull(&self) -> f32 {
        (self.max_pull_percent / 100.0).clamp(0.0, 0.16)
    }

    pub fn overlaps(&self) -> bool {
        !matches!(self.mode, BlendMode::Off)
    }

    /// The playback rate to apply to `incoming` so its beats line up with `outgoing`, or `None`
    /// if that cannot be done within the permitted pull.
    ///
    /// **Half and double time count as matched.** A 64 BPM track already lines up with a 128 BPM
    /// one — every beat of the slower lands on every other beat of the faster — so a DJ would
    /// mix them without touching the speed of either. Naively demanding equal numbers would call
    /// that a 100% pull and refuse a blend that needs no pull at all. So the candidates are
    /// `outgoing · 2ᵏ / incoming` for `k` in −1, 0, 1, and the winner is whichever sits closest
    /// to 1.0. Doubling the *rate* is never the answer: that would raise the pitch an octave.
    ///
    /// `None` for either tempo means we never measured one — a track with no steady pulse, or
    /// one we have not heard enough of. That is a refusal: a beat-matched blend needs two beats.
    pub fn rate_for(&self, outgoing: Option<f32>, incoming: Option<f32>) -> Option<f32> {
        let (Some(out), Some(inc)) = (outgoing, incoming) else {
            return None;
        };
        if out <= 0.0 || inc <= 0.0 {
            return None;
        }
        [0.5, 1.0, 2.0]
            .into_iter()
            .map(|octave| out * octave / inc)
            .filter(|rate| (rate - 1.0).abs() <= self.max_pull())
            .min_by(|a, b| (a - 1.0).abs().total_cmp(&(b - 1.0).abs()))
    }

    /// Whether these two tracks can be beat-matched at all.
    pub fn can_match(&self, outgoing: Option<f32>, incoming: Option<f32>) -> bool {
        self.rate_for(outgoing, incoming).is_some()
    }
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
    // Reach the *running* engine, not only the next launch. Volume and output device already
    // apply live from the UI; blending would otherwise appear to do nothing until a restart,
    // which reads as a broken setting rather than a deferred one.
    crate::native::apply_blend(&app, &settings.blend);
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blend(max_pull_percent: f32) -> Blend {
        Blend {
            mode: BlendMode::BeatMatched,
            max_pull_percent,
            ..Blend::default()
        }
    }

    /// Close tempos get a small pull, in the right direction.
    #[test]
    fn a_near_miss_is_pulled_onto_the_outgoing_tempo() {
        let b = blend(6.0);
        // 124 has to speed up slightly to become 128.
        let rate = b.rate_for(Some(128.0), Some(124.0)).expect("within range");
        assert!((rate - 128.0 / 124.0).abs() < 1e-6, "rate was {rate}");
        assert!(rate > 1.0, "the slower track should speed up, got {rate}");

        // And the reverse.
        let rate = b.rate_for(Some(124.0), Some(128.0)).expect("within range");
        assert!(rate < 1.0, "the faster track should slow down, got {rate}");
    }

    /// Half time needs no pull at all. Demanding equal numbers would score this as 100% and
    /// refuse a blend that a DJ would make without touching either deck.
    #[test]
    fn half_and_double_time_match_without_a_pull() {
        let b = blend(6.0);
        for (out, inc) in [(128.0, 64.0), (64.0, 128.0), (170.0, 85.0)] {
            let rate = b
                .rate_for(Some(out), Some(inc))
                .unwrap_or_else(|| panic!("{out} against {inc} should match"));
            assert!(
                (rate - 1.0).abs() < 1e-6,
                "{out} against {inc} wanted rate {rate}, expected no pull"
            );
        }
    }

    /// Genuinely different tempos are refused, so the blend is skipped rather than made badly.
    #[test]
    fn a_real_mismatch_is_refused() {
        let b = blend(6.0);
        assert!(b.rate_for(Some(128.0), Some(90.0)).is_none());
        assert!(b.rate_for(Some(75.0), Some(174.0)).is_none());
    }

    /// A track we could not measure never gets beat-matched — there is nothing to match to.
    #[test]
    fn an_unmeasured_tempo_is_never_matched() {
        let b = blend(16.0);
        assert!(!b.can_match(None, Some(120.0)));
        assert!(!b.can_match(Some(120.0), None));
        assert!(!b.can_match(Some(120.0), Some(0.0)));
    }

    /// The range is the setting doing its job: what is refused at ±2% is accepted at ±10%.
    #[test]
    fn the_pull_range_decides() {
        assert!(blend(2.0).rate_for(Some(128.0), Some(120.0)).is_none());
        assert!(blend(10.0).rate_for(Some(128.0), Some(120.0)).is_some());
    }

    /// Hand-edited files don't get to blow past a sane pitch-fader range or overlap a whole song.
    #[test]
    fn absurd_values_are_clamped() {
        let wild = Blend {
            mode: BlendMode::BeatMatched,
            seconds: 900.0,
            max_pull_percent: 400.0,
            restore_tempo: true,
        };
        assert_eq!(wild.seconds(), 20.0);
        assert!((wild.max_pull() - 0.16).abs() < 1e-6);
    }

    /// A settings file written before blending existed must still parse, and must not silently
    /// start blending someone's music.
    #[test]
    fn older_settings_files_default_to_no_blending() {
        let old = r#"{"updatePolicy":"afterSong","theme":"system","volume":100}"#;
        let parsed: Settings = serde_json::from_str(old).expect("old file still parses");
        assert_eq!(parsed.blend.mode, BlendMode::Off);
        assert!(!parsed.blend.overlaps());
    }

    /// A settings file written before the strip could be turned round still parses, and lands
    /// on the same default as a fresh install rather than on whichever variant happens to be
    /// listed first.
    #[test]
    fn older_settings_files_get_the_newest_on_the_right() {
        let old = r#"{"updatePolicy":"afterSong","theme":"system","volume":100}"#;
        let parsed: Settings = serde_json::from_str(old).expect("old file still parses");
        assert_eq!(parsed.recents_order, RecentsOrder::NewestRight);
        assert_eq!(parsed.recents_order, Settings::default().recents_order);
    }

    /// The stored name is what the webview switches on, so it is part of the file format and
    /// not free to be renamed.
    #[test]
    fn recents_order_round_trips_by_name() {
        let json = serde_json::to_string(&RecentsOrder::NewestLeft).unwrap();
        assert_eq!(json, r#""newestLeft""#);
        assert_eq!(
            serde_json::from_str::<RecentsOrder>(&json).unwrap(),
            RecentsOrder::NewestLeft
        );
    }

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
            r#"{"updatePolicy":"afterSong","checkSchedule":{"kind":"every","minutes":30},"theme":"system","recentsOrder":"newestRight","volume":100,"blend":{"mode":"off","seconds":5.0,"maxPullPercent":6.0,"restoreTempo":true},"outputDevice":null}"#
        );

        let daily = Settings {
            update_policy: Policy::NotifyOnly,
            check_schedule: CheckSchedule::DailyAt {
                time: "03:30".into(),
            },
            theme: Theme::Light,
            recents_order: RecentsOrder::NewestLeft,
            volume: Volume::new(60),
            blend: Blend {
                mode: BlendMode::BeatMatched,
                seconds: 10.0,
                max_pull_percent: 10.0,
                restore_tempo: false,
            },
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
