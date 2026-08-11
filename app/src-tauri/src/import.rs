//! Reading a station-preferences export back in — the planning half.
//!
//! This module **only reads**. It parses an export file, checks it, compares it against
//! the account as it is now, and reports what applying it would do. Nothing here writes to
//! Pandora; the applying half is deliberately a separate step, because an import creates
//! stations and adds seeds and there should be a chance to look at that list first.
//!
//! # What can and cannot come back
//!
//! Export is lossless. Import cannot be, and the reason is worth stating plainly rather
//! than discovering later:
//!
//! - **Seeds** restore. `station.addMusic` takes a `musicToken`, which the export keeps.
//! - **Stations** restore. `station.createStation` takes a `musicToken` — so a station is
//!   recreated from one of its own seeds, then renamed.
//! - **Thumbs do not.** `station.addFeedback` needs a `trackToken`, which is ephemeral:
//!   Pandora issues it per playlist fragment for a track it has just served on that
//!   station. There is no bulk "restore my thumbs" call and no way to synthesise one. The
//!   realistic approach is to hold the exported list and re-apply a thumb opportunistically
//!   when that track happens to play — which is why the export keeps `musicToken` on every
//!   thumb, so a thumbed song can at least be turned back into a seed.
//!
//! The plan says so per station rather than quietly restoring less than the file contains.

use serde::Serialize;

use crate::export::{ExportFile, SCHEMA_VERSION};

/// What importing one station would do.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StationPlan {
    pub name: String,
    /// Matched against the account by token first, then by name.
    pub exists: bool,
    /// Seeds in the file that the live station does not already have.
    pub seeds_to_add: usize,
    /// Seeds already present, so importing is a no-op for them.
    pub seeds_already_there: usize,
    /// Seeds we cannot act on because the file has no `musicToken` for them.
    pub seeds_unusable: usize,
    /// Thumbs recorded in the file. Reported, never applied — see the module note.
    pub thumbs_not_restorable: usize,
    /// Why this station cannot be imported at all, if so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub exported_at: String,
    pub exported_by: String,
    pub stations: Vec<StationPlan>,
    /// Ordered, human-readable notes about the file as a whole.
    pub notes: Vec<String>,
}

impl ImportPlan {
    pub fn stations_to_create(&self) -> usize {
        self.stations
            .iter()
            .filter(|s| !s.exists && s.blocked.is_none())
            .count()
    }
    pub fn total_seeds_to_add(&self) -> usize {
        self.stations
            .iter()
            .filter(|s| s.blocked.is_none())
            .map(|s| s.seeds_to_add)
            .sum()
    }
}

/// A station as it exists on the account right now, reduced to what planning needs.
pub struct LiveStation {
    pub name: String,
    pub token: String,
    /// `musicToken`s of the seeds already on it. Empty when unknown — which makes the plan
    /// over-report work rather than silently skip it.
    pub seed_tokens: Vec<String>,
}

/// Parse an export file, rejecting anything we cannot honestly act on.
pub fn parse(text: &str) -> Result<ExportFile, String> {
    let file: ExportFile = serde_json::from_str(text)
        .map_err(|e| format!("this does not look like a Jarlid export: {e}"))?;
    if file.jarlid_export == 0 {
        return Err("missing \"jarlidExport\" version — is this a Jarlid export?".into());
    }
    // Refuse a newer schema rather than guess at fields we do not know about.
    if file.jarlid_export > SCHEMA_VERSION {
        return Err(format!(
            "this file is version {} but this build only understands {SCHEMA_VERSION}. Update Jarlid first.",
            file.jarlid_export
        ));
    }
    Ok(file)
}

/// Work out what importing `file` would do to `live`, without doing any of it.
pub fn plan(file: &ExportFile, live: &[LiveStation]) -> ImportPlan {
    let mut notes = Vec::new();
    let mut stations = Vec::new();

    for st in &file.stations {
        let existing = live
            .iter()
            .find(|l| !st.station_token.is_empty() && l.token == st.station_token)
            // Tokens are per-account, so a file from another account (or a station deleted
            // and remade) only matches by name.
            .or_else(|| live.iter().find(|l| l.name == st.name));

        let is_quick_mix = st
            .settings
            .get("isQuickMix")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let usable: Vec<&str> = st
            .seeds
            .iter()
            .filter(|s| !s.music_token.is_empty())
            .map(|s| s.music_token.as_str())
            .collect();
        let unusable = st.seeds.len() - usable.len();

        let (to_add, already) = match existing {
            Some(l) if !l.seed_tokens.is_empty() => {
                let have: std::collections::HashSet<&str> =
                    l.seed_tokens.iter().map(String::as_str).collect();
                let add = usable.iter().filter(|t| !have.contains(*t)).count();
                (add, usable.len() - add)
            }
            // Seeds unknown: assume none are present. Over-reporting work is safer than
            // claiming a station is already complete when it might not be.
            _ => (usable.len(), 0),
        };

        let blocked = if is_quick_mix {
            Some(
                "QuickMix is a shuffle over other stations — recreate it in Pandora once \
                 those exist"
                    .into(),
            )
        } else if existing.is_none() && usable.is_empty() {
            Some("no seed with a musicToken, so there is nothing to create it from".into())
        } else {
            None
        };

        stations.push(StationPlan {
            name: st.name.clone(),
            exists: existing.is_some(),
            seeds_to_add: to_add,
            seeds_already_there: already,
            seeds_unusable: unusable,
            thumbs_not_restorable: st.feedback.len(),
            blocked,
        });
    }

    let thumbs: usize = stations.iter().map(|s| s.thumbs_not_restorable).sum();
    if thumbs > 0 {
        notes.push(format!(
            "{thumbs} thumbs are recorded but cannot be restored: Pandora only accepts a \
             thumb for a track it has just served you. They stay in the file."
        ));
    }
    let unusable: usize = stations.iter().map(|s| s.seeds_unusable).sum();
    if unusable > 0 {
        notes.push(format!(
            "{unusable} seeds have no musicToken and will be skipped."
        ));
    }
    if stations.iter().any(|s| s.blocked.is_some()) {
        notes.push("Some stations cannot be imported — see the list.".into());
    }

    ImportPlan {
        exported_at: file.exported_at.clone(),
        exported_by: file.exported_by.clone(),
        stations,
        notes,
    }
}

/// Pick a file and report what importing it would do. Reads only.
#[tauri::command]
pub async fn import_preview(app: tauri::AppHandle) -> Result<Option<ImportPlan>, String> {
    use tauri::Manager;
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Open a Jarlid export")
        .add_filter("Jarlid export", &["json"])
        .pick_file(move |p| {
            let _ = tx.send(p);
        });
    let Some(picked) = rx.await.map_err(|_| "dialog closed".to_string())? else {
        return Ok(None); // dismissed
    };
    let path = picked
        .into_path()
        .map_err(|e| format!("unusable path: {e}"))?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let file = parse(&text)?;

    // The live side: names and tokens. Seed lists would need one request per station, so
    // the plan reports "would add" without claiming to know what is already there.
    let live = match app
        .state::<crate::native::NativeEngine>()
        .engine()
        .await
        .map(|e| e)
    {
        Ok(engine) => engine
            .station_list()
            .await
            .map(|list| {
                list.into_iter()
                    .map(|s| LiveStation {
                        name: s.station_name,
                        token: s.station_token,
                        seed_tokens: Vec::new(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    Ok(Some(plan(&file, &live)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Counts, Feedback, Seed, Station};

    fn seed(token: &str) -> Seed {
        Seed {
            kind: "artist".into(),
            name: format!("Artist {token}"),
            artist: String::new(),
            music_token: token.into(),
            pandora_id: String::new(),
            seed_id: String::new(),
            art: String::new(),
        }
    }

    fn thumb() -> Feedback {
        Feedback {
            rating: "up".into(),
            name: "Song".into(),
            artist: "Artist".into(),
            song_identity: String::new(),
            pandora_id: String::new(),
            music_token: String::new(),
            feedback_id: String::new(),
            dated: String::new(),
            art: String::new(),
        }
    }

    fn station(name: &str, token: &str, seeds: Vec<Seed>) -> Station {
        Station {
            station_id: String::new(),
            station_token: token.into(),
            name: name.into(),
            art: String::new(),
            date_created: String::new(),
            counts: Counts::default(),
            settings: serde_json::Map::new(),
            seeds,
            feedback: vec![],
            warnings: vec![],
        }
    }

    fn file(stations: Vec<Station>) -> ExportFile {
        ExportFile {
            jarlid_export: SCHEMA_VERSION,
            exported_at: "2026-08-08T00:00:00Z".into(),
            exported_by: "Jarlid test".into(),
            stations,
        }
    }

    #[test]
    fn rejects_things_that_are_not_exports() {
        assert!(parse("{}").is_err(), "no version field");
        assert!(parse("not json").is_err());
        assert!(
            parse(r#"{"jarlidExport":1}"#).is_ok(),
            "stations may be absent"
        );
    }

    /// A newer file may contain fields this build would silently drop. Refusing is the
    /// honest response; a half-applied import is worse than none.
    #[test]
    fn refuses_a_newer_schema() {
        let err = parse(r#"{"jarlidExport":99}"#).unwrap_err();
        assert!(err.contains("99"), "{err}");
        assert!(err.contains("Update Jarlid"), "{err}");
    }

    #[test]
    fn a_missing_station_is_one_to_create() {
        let f = file(vec![station(
            "New Radio",
            "T1",
            vec![seed("S1"), seed("S2")],
        )]);
        let p = plan(&f, &[]);
        assert_eq!(p.stations_to_create(), 1);
        assert!(!p.stations[0].exists);
        assert_eq!(p.stations[0].seeds_to_add, 2);
    }

    /// Tokens are per-account, so a file from a different account has to fall back to
    /// matching by name or it would propose recreating everything.
    #[test]
    fn matches_by_token_then_by_name() {
        let f = file(vec![station("Dove Cameron Radio", "T1", vec![seed("S1")])]);

        let by_token = plan(
            &f,
            &[LiveStation {
                name: "Renamed".into(),
                token: "T1".into(),
                seed_tokens: vec![],
            }],
        );
        assert!(by_token.stations[0].exists, "token wins even if renamed");

        let by_name = plan(
            &f,
            &[LiveStation {
                name: "Dove Cameron Radio".into(),
                token: "DIFFERENT".into(),
                seed_tokens: vec![],
            }],
        );
        assert!(by_name.stations[0].exists, "name is the fallback");
    }

    #[test]
    fn does_not_propose_adding_seeds_that_are_already_there() {
        let f = file(vec![station("R", "T1", vec![seed("S1"), seed("S2")])]);
        let p = plan(
            &f,
            &[LiveStation {
                name: "R".into(),
                token: "T1".into(),
                seed_tokens: vec!["S1".into()],
            }],
        );
        assert_eq!(p.stations[0].seeds_to_add, 1);
        assert_eq!(p.stations[0].seeds_already_there, 1);
    }

    /// A seed with no musicToken cannot be added by any API we have. Counting it as
    /// unusable beats reporting work that will then silently not happen.
    #[test]
    fn counts_seeds_it_cannot_act_on() {
        let mut orphan = seed("");
        orphan.name = "Someone".into();
        let f = file(vec![station("R", "T1", vec![seed("S1"), orphan])]);
        let p = plan(&f, &[]);
        assert_eq!(p.stations[0].seeds_to_add, 1);
        assert_eq!(p.stations[0].seeds_unusable, 1);
        assert!(
            p.notes.iter().any(|n| n.contains("no musicToken")),
            "{:?}",
            p.notes
        );
    }

    /// The honest headline: thumbs are in the file and are not coming back.
    #[test]
    fn reports_thumbs_as_not_restorable() {
        let mut s = station("R", "T1", vec![seed("S1")]);
        s.feedback = vec![thumb(), thumb(), thumb()];
        let p = plan(&file(vec![s]), &[]);
        assert_eq!(p.stations[0].thumbs_not_restorable, 3);
        assert!(
            p.notes.iter().any(|n| n.contains("cannot be restored")),
            "{:?}",
            p.notes
        );
    }

    /// A QuickMix is defined by the stations it shuffles, which may not exist yet, and
    /// there is no API to set its membership. Say so rather than creating an empty one.
    #[test]
    fn refuses_to_recreate_a_quickmix() {
        let mut s = station("QuickMix", "T0", vec![]);
        s.settings
            .insert("isQuickMix".into(), serde_json::json!(true));
        let p = plan(&file(vec![s]), &[]);
        assert!(p.stations[0].blocked.is_some());
        assert_eq!(p.stations_to_create(), 0);
    }

    /// Nothing to build a station from is a block, not a silent zero-seed creation.
    #[test]
    fn a_new_station_with_no_usable_seed_is_blocked() {
        let f = file(vec![station("R", "T1", vec![])]);
        let p = plan(&f, &[]);
        assert!(p.stations[0].blocked.is_some());
        assert_eq!(p.stations_to_create(), 0);

        // ...but an *existing* station with no seeds is simply nothing to do.
        let p2 = plan(
            &f,
            &[LiveStation {
                name: "R".into(),
                token: "T1".into(),
                seed_tokens: vec![],
            }],
        );
        assert_eq!(p2.stations[0].blocked, None);
        assert_eq!(p2.stations[0].seeds_to_add, 0);
    }

    /// An export written by this build must plan cleanly — the two halves share one struct
    /// precisely so this round trip holds.
    #[test]
    fn round_trips_a_real_export() {
        let f = file(vec![station("R", "T1", vec![seed("S1")])]);
        let text = serde_json::to_string(&f).unwrap();
        let back = parse(&text).unwrap();
        assert_eq!(back.stations.len(), 1);
        assert_eq!(plan(&back, &[]).total_seeds_to_add(), 1);
    }
}
