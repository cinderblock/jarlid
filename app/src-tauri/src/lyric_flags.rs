//! Lyrics flags: the listener marking a song's lyrics, or a stretch of them, as wrong.
//!
//! A flag is a note to self, kept on this machine, never sent anywhere. (Reporting a record
//! to LRCLIB is `lyrics::flag_lyrics`, a different and public thing.) Pandora is radio, so a
//! flagged song can't be called back up to look at again; a flag has to stand on its own.
//! So each one keeps the whole set of lyrics it was made on and, when those were auto-synced,
//! what the speech engine heard, which is enough to re-run the aligner on the song later.
//!
//! One file per track in the data dir, keyed like edits and pins by Pandora's naming. These
//! are the listener's own work, not something a cache wipe should take with it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::Manager;

use crate::autosync::Evidence;

/// Everything flagged on one track.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackFlags {
    /// Pandora's naming. Kept in the file so a list of all flags can say what each is for
    /// without reversing the file name's hash.
    pub artist: String,
    pub track: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    pub flags: Vec<Flag>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Flag {
    /// Made by the frontend, unique within the track.
    pub id: String,
    /// Unix milliseconds.
    pub created: u64,
    /// `None` flags the whole song.
    #[serde(default)]
    pub span: Option<Span>,
    /// What kind of wrong: `timing`, `words`, `version` so far. Strings rather than an enum, so
    /// a new kind doesn't need old files migrated or new files refused by an old build.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub note: String,
    /// The lyrics on screen when it was made.
    pub shown: Shown,
}

/// A stretch of the song.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    /// Pane rows, inclusive, counted as the pane counts them: a two-language pair is one row.
    pub first: usize,
    pub last: usize,
    /// Seconds on the lyrics' clock (the playhead plus [`Shown::offset`]): the first row's
    /// stamp to the next row's, or the song's end. `None` for lyrics with no timings.
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub end: Option<f64>,
    /// The flagged rows' words, so the flag reads without the lyrics beside it.
    #[serde(default)]
    pub text: Vec<String>,
}

/// A copy of what was on screen.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Shown {
    pub source: String,
    /// The LRCLIB record, when there is one.
    pub id: Option<i64>,
    pub synced: Option<String>,
    pub plain: Option<String>,
    pub auto: bool,
    pub overridden: bool,
    pub pinned: bool,
    /// The track's `[`/`]` sync offset, seconds, in effect at the time.
    pub offset: f64,
    /// Auto-synced lyrics only. Filled in here, never taken from the frontend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autosync: Option<Evidence>,
}

fn flags_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("lyrics-flags"))
}

fn flags_path(app: &tauri::AppHandle, key: &str) -> Option<PathBuf> {
    crate::lyrics::hashed_file(flags_dir(app)?, key)
}

fn read(path: &std::path::Path) -> Option<TrackFlags> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// What's flagged on this track, if anything. A file that is there but can't be read is an
/// error, not "nothing flagged": the frontend saves its whole list, so mistaking one for the
/// other would write over the flags in it.
#[tauri::command]
pub async fn lyric_flags(
    app: tauri::AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
) -> Result<Option<TrackFlags>, String> {
    let key = crate::lyrics::key_for(&artist, &track, album.as_deref());
    let path = flags_path(&app, &key).ok_or("No data directory")?;
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let t: TrackFlags =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(t).filter(|f| !f.flags.is_empty()))
}

/// Replace the track's flags with `flags`. An empty list removes the file. Returns what was
/// stored, evidence included.
#[tauri::command]
pub async fn save_lyric_flags(
    app: tauri::AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
    duration: Option<f64>,
    flags: Vec<Flag>,
) -> Result<TrackFlags, String> {
    let key = crate::lyrics::key_for(&artist, &track, album.as_deref());
    let path = flags_path(&app, &key).ok_or("No data directory")?;
    let old = read(&path).map(|f| f.flags).unwrap_or_default();
    let mut flags = carry_evidence(&old, flags);
    for f in &mut flags {
        if f.shown.autosync.is_none() && f.shown.auto {
            if let Some(plain) = &f.shown.plain {
                f.shown.autosync = crate::autosync::evidence(&app, &key, plain);
            }
        }
    }
    let out = TrackFlags {
        artist,
        track,
        album,
        duration,
        flags,
    };
    if out.flags.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    } else {
        let json = serde_json::to_vec_pretty(&out).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// The evidence is ours to keep, not the frontend's to send back: whatever a flag had when it
/// was stored stays with it, matched by id, and anything the frontend sent is dropped.
fn carry_evidence(old: &[Flag], new: Vec<Flag>) -> Vec<Flag> {
    new.into_iter()
        .map(|mut f| {
            f.shown.autosync = old
                .iter()
                .find(|o| o.id == f.id)
                .and_then(|o| o.shown.autosync.clone());
            f
        })
        .collect()
}

/// Every flagged track, the most recently flagged first.
#[tauri::command]
pub async fn all_lyric_flags(app: tauri::AppHandle) -> Vec<TrackFlags> {
    let Some(dir) = flags_dir(&app) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut all: Vec<TrackFlags> = entries
        .filter_map(|e| read(&e.ok()?.path()))
        .filter(|f| !f.flags.is_empty())
        .collect();
    sort_newest_first(&mut all);
    all
}

fn sort_newest_first(all: &mut [TrackFlags]) {
    let newest = |t: &TrackFlags| t.flags.iter().map(|f| f.created).max().unwrap_or(0);
    all.sort_by_key(|t| std::cmp::Reverse(newest(t)));
}

/// Open the folder the flags are kept in.
#[tauri::command]
pub fn lyric_flags_reveal(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let dir = flags_dir(&app).ok_or("No data directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(dir.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(id: &str, created: u64, evidence: Option<Evidence>) -> Flag {
        Flag {
            id: id.into(),
            created,
            span: None,
            tags: vec![],
            note: String::new(),
            shown: Shown {
                auto: true,
                plain: Some("words".into()),
                autosync: evidence,
                ..Default::default()
            },
        }
    }

    fn evidence(engine: &str) -> Evidence {
        Evidence {
            engine: engine.into(),
            aligner: 2,
            coverage: 0.7,
            heard: vec![("hello".into(), 1.0, 1.4)],
        }
    }

    #[test]
    fn evidence_stays_with_its_flag_and_is_never_taken_from_the_frontend() {
        let old = vec![flag("a", 1, Some(evidence("stored")))];
        let sent = vec![
            flag("a", 1, None),
            flag("b", 2, Some(evidence("from the frontend"))),
        ];
        let out = carry_evidence(&old, sent);
        assert_eq!(out[0].shown.autosync, Some(evidence("stored")));
        assert_eq!(out[1].shown.autosync, None);
    }

    #[test]
    fn reads_a_minimal_file() {
        // What a later build that adds fields must still be able to read, and what this one
        // writes at its sparsest.
        let json = r#"{"artist":"A","track":"T","flags":[
            {"id":"x","created":5,"shown":{"source":"lrclib"}}
        ]}"#;
        let t: TrackFlags = serde_json::from_str(json).unwrap();
        assert_eq!(t.album, None);
        assert_eq!(t.flags[0].span, None);
        assert!(t.flags[0].tags.is_empty());
        assert_eq!(t.flags[0].shown.offset, 0.0);
    }

    #[test]
    fn round_trips_a_stretch() {
        let mut f = flag("s", 9, Some(evidence("e")));
        f.span = Some(Span {
            first: 3,
            last: 5,
            start: Some(41.2),
            end: Some(52.0),
            text: vec!["one".into(), "".into(), "three".into()],
        });
        f.tags = vec!["timing".into()];
        let t = TrackFlags {
            artist: "A".into(),
            track: "T".into(),
            album: Some("Al".into()),
            duration: Some(200.0),
            flags: vec![f],
        };
        let back: TrackFlags = serde_json::from_slice(&serde_json::to_vec(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn newest_flag_sorts_its_track_first() {
        let track = |name: &str, created: &[u64]| TrackFlags {
            artist: "A".into(),
            track: name.into(),
            album: None,
            duration: None,
            flags: created.iter().map(|&c| flag("f", c, None)).collect(),
        };
        let mut all = vec![
            track("old", &[1, 2]),
            track("new", &[3]),
            track("mixed", &[0, 9]),
        ];
        sort_newest_first(&mut all);
        let names: Vec<_> = all.iter().map(|t| t.track.as_str()).collect();
        assert_eq!(names, ["mixed", "new", "old"]);
    }
}
