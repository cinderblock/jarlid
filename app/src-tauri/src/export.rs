//! Station-preferences export.
//!
//! Thumbs and seeds are years of accumulated listening that exist only inside the
//! Pandora account. There is no official export, and losing the account loses all
//! of it — so this writes a copy the user owns.
//!
//! One `station.getStation` call per station returns seeds and every thumb
//! together (see [`pandora::Client::station_details`]), so the walk is one
//! request per station. It is still deliberately serial and gapped: Pandora
//! permits one stream per account and we are holding it, so an export runs
//! *while music is playing* and must not look like a scrape.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};

/// Gap between stations. Slow on purpose; see the module note.
const STATION_GAP: std::time::Duration = std::time::Duration::from_millis(700);

const SCHEMA_VERSION: u32 = 1;

#[derive(Default)]
pub struct ExportCtl {
    cancel: AtomicBool,
    running: AtomicBool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Seed {
    /// "artist" | "song" | "genre"
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artist: String,
    /// Pandora's token for this seed. The one value an import can act on:
    /// `station.addSeed` takes a musicToken and nothing else identifies it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub music_token: String,
    /// Catalogue id ("AR:…" / "TR:…"), stable across the seed being re-added.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pandora_id: String,
    /// Identifies this seed *on this station*, for `station.deleteSeed`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub seed_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub art: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Feedback {
    /// "up" | "down"
    pub rating: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artist: String,
    /// Identifies the song itself across stations.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub song_identity: String,
    /// Pandora's catalogue id for the song ("TR:…").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pandora_id: String,
    /// Usable as a station seed, which is the one thing a thumb can be turned
    /// back into — re-applying the thumb itself needs an ephemeral trackToken.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub music_token: String,
    /// What `station.deleteFeedback` takes, so an import can undo a thumb.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub feedback_id: String,
    /// When the thumb was given (epoch ms, as Pandora reports it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dated: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub art: String,
}

// NOTE: there is deliberately no `album` field. `feedback.thumbsUp[]` entries
// carry no `albumName` — confirmed against the live API on 2026-08-08 with
// `cargo run -p engine --example dump-station-shape`. A field that is always
// empty in a backup is worse than an absent one.

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub up: u64,
    pub down: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub station_id: String,
    pub station_token: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub art: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date_created: String,
    /// What Pandora says the totals are, which is not always what it hands over.
    #[serde(default)]
    pub counts: Counts,
    #[serde(default)]
    pub settings: serde_json::Map<String, Value>,
    #[serde(default)]
    pub seeds: Vec<Seed>,
    #[serde(default)]
    pub feedback: Vec<Feedback>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportFile {
    /// Bump when the shape changes incompatibly; an importer should refuse a
    /// version it doesn't know rather than guess.
    jarlid_export: u32,
    exported_at: String,
    exported_by: String,
    stations: Vec<Station>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// `None` when the user dismissed the save dialog.
    path: Option<String>,
    stations: usize,
    thumbs: usize,
    seeds: usize,
    skipped: Vec<String>,
    /// Set when the run ended early. Whatever was collected is still offered —
    /// discarding 80 stations of deliberately slow work because station 81
    /// failed would be indefensible.
    stopped_reason: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: usize,
    total: usize,
    station: String,
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn today_stamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        now.month() as u8,
        now.day()
    )
}

fn str_at(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// First of `keys` that holds a non-empty string. For shapes we could not
/// confirm against a live account.
fn first_non_empty(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .map(|k| str_at(v, k))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

/// Pandora reports dates as an object carrying epoch millis in `time`.
fn epoch_ms(v: Option<&Value>) -> String {
    v.and_then(|d| d.get("time"))
        .and_then(Value::as_i64)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// Pandora's seed and feedback entries both carry art under several names
/// depending on which list they came from.
fn art_at(v: &Value) -> String {
    for key in ["albumArtUrl", "artUrl", "imageUrl"] {
        let url = str_at(v, key);
        if !url.is_empty() {
            return url;
        }
    }
    String::new()
}

/// Map one `station.getStation` response onto the export schema.
///
/// Kept as a free function taking a `Value` so it can be tested against a
/// recorded response without a live account — this mapping is the part most
/// likely to be silently wrong.
pub fn map_station(token: &str, v: &Value) -> Station {
    let music = v.get("music");
    let seed_list = |key: &str, kind: &str| -> Vec<Seed> {
        music
            .and_then(|m| m.get(key))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|s| Seed {
                        kind: kind.to_string(),
                        // Songs name the track; artist and genre seeds name themselves.
                        // Song and artist shapes are confirmed against the live API; the
                        // genre one is not (no account station had a genre seed to look
                        // at), so try the plausible spellings rather than bet on one.
                        name: match kind {
                            "song" => str_at(s, "songName"),
                            "genre" => first_non_empty(s, &["genreName", "name", "stationName"]),
                            _ => str_at(s, "artistName"),
                        },
                        // Only a song seed has a separate artist worth recording.
                        artist: if kind == "song" {
                            str_at(s, "artistName")
                        } else {
                            String::new()
                        },
                        music_token: str_at(s, "musicToken"),
                        pandora_id: str_at(s, "pandoraId"),
                        seed_id: str_at(s, "seedId"),
                        art: art_at(s),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut seeds = seed_list("songs", "song");
    seeds.extend(seed_list("artists", "artist"));
    seeds.extend(seed_list("genres", "genre"));

    let fb = v.get("feedback");
    let thumb_list = |key: &str, rating: &str| -> Vec<Feedback> {
        fb.and_then(|f| f.get(key))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|t| Feedback {
                        rating: rating.to_string(),
                        name: str_at(t, "songName"),
                        artist: str_at(t, "artistName"),
                        song_identity: str_at(t, "songIdentity"),
                        pandora_id: str_at(t, "pandoraId"),
                        music_token: str_at(t, "musicToken"),
                        feedback_id: str_at(t, "feedbackId"),
                        dated: epoch_ms(t.get("dateCreated")),
                        art: art_at(t),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut feedback = thumb_list("thumbsUp", "up");
    feedback.extend(thumb_list("thumbsDown", "down"));

    let num = |k: &str| {
        fb.and_then(|f| f.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let counts = Counts {
        up: num("totalThumbsUp"),
        down: num("totalThumbsDown"),
    };

    // Per-station options, copied through as Pandora reports them. Free-form
    // because the set is theirs to change, and an export that silently drops a
    // field it doesn't model is worse than one carrying an unfamiliar field.
    //
    // This list is what `getStation` actually returns, confirmed 2026-08-08 via
    // `cargo run -p engine --example dump-station-shape`. The old partner-API
    // names `isThumbprintStation` and `requiresCleanAds` are NOT among them.
    let mut settings = serde_json::Map::new();
    for key in [
        "isShared",
        "isQuickMix",
        "isGenreStation",
        "allowAddMusic",
        "allowRename",
        "allowDelete",
        "allowEditDescription",
        "genre",
        "hasTakeoverModes",
        "hasCuratedModes",
        // Discovery Tuner state.
        "modes",
        // A QuickMix is defined entirely by which stations it shuffles, so
        // without this the station is not actually backed up.
        "quickMixStationIds",
    ] {
        if let Some(val) = v.get(key) {
            settings.insert(key.to_string(), val.clone());
        }
    }

    let mut warnings = Vec::new();
    let got_up = feedback.iter().filter(|f| f.rating == "up").count() as u64;
    let got_down = feedback.iter().filter(|f| f.rating == "down").count() as u64;
    // getStation caps how much feedback it returns on heavily-thumbed stations.
    // Saying so beats a backup that looks complete and isn't.
    if counts.up > got_up || counts.down > got_down {
        warnings.push(format!(
            "Pandora reported {}/{} thumbs up/down but returned {got_up}/{got_down}",
            counts.up, counts.down
        ));
    }

    Station {
        station_id: str_at(v, "stationId"),
        station_token: if str_at(v, "stationToken").is_empty() {
            token.to_string()
        } else {
            str_at(v, "stationToken")
        },
        name: str_at(v, "stationName"),
        art: art_at(v),
        date_created: epoch_ms(v.get("dateCreated")),
        counts,
        settings,
        seeds,
        feedback,
        warnings,
    }
}

#[tauri::command]
pub fn cancel_export(ctl: tauri::State<'_, ExportCtl>) {
    ctl.cancel.store(true, Ordering::Relaxed);
}

/// `stations` is a list of `[name, tunerToken]` pairs, as the picker holds them.
#[tauri::command]
pub async fn export_stations(
    app: tauri::AppHandle,
    stations: Vec<(String, String)>,
) -> Result<ExportResult, String> {
    if stations.is_empty() {
        return Err("no stations selected".into());
    }
    {
        let ctl = app.state::<ExportCtl>();
        if ctl.running.swap(true, Ordering::SeqCst) {
            return Err("an export is already running".into());
        }
        ctl.cancel.store(false, Ordering::Relaxed);
    }

    let result = run(&app, &stations).await;

    let ctl = app.state::<ExportCtl>();
    ctl.running.store(false, Ordering::SeqCst);
    ctl.cancel.store(false, Ordering::Relaxed);
    result
}

async fn run(
    app: &tauri::AppHandle,
    stations: &[(String, String)],
) -> Result<ExportResult, String> {
    let engine = app.state::<crate::native::NativeEngine>().engine().await?;

    let total = stations.len();
    let mut collected: Vec<Station> = Vec::with_capacity(total);
    let mut skipped: Vec<String> = Vec::new();
    let mut stopped_reason: Option<String> = None;

    for (i, (name, token)) in stations.iter().enumerate() {
        if app.state::<ExportCtl>().cancel.load(Ordering::Relaxed) {
            stopped_reason = Some(format!("Cancelled after {i} of {total} stations"));
            break;
        }

        match engine.station_details(token).await {
            Ok(v) => collected.push(map_station(token, &v)),
            Err(e) => {
                // A stream violation means another device grabbed the account's
                // one permitted stream; pressing on would just fail repeatedly.
                if e.to_string().contains("STREAM_VIOLATION") {
                    stopped_reason = Some("Pandora is playing on another device".into());
                    break;
                }
                skipped.push(format!("{name} ({e})"));
            }
        }

        let _ = app.emit(
            "export://progress",
            Progress {
                done: i + 1,
                total,
                station: name.clone(),
            },
        );

        if i + 1 < total {
            tokio::time::sleep(STATION_GAP).await;
        }
    }

    if collected.is_empty() {
        return Err(stopped_reason.unwrap_or_else(|| "no station data came back".into()));
    }
    save(app, collected, skipped, stopped_reason).await
}

/// Ask where to put the file, then write it.
async fn save(
    app: &tauri::AppHandle,
    stations: Vec<Station>,
    skipped: Vec<String>,
    stopped_reason: Option<String>,
) -> Result<ExportResult, String> {
    use tauri_plugin_dialog::DialogExt;

    let thumbs = stations.iter().map(|s| s.feedback.len()).sum();
    let seeds = stations.iter().map(|s| s.seeds.len()).sum();

    let file = ExportFile {
        jarlid_export: SCHEMA_VERSION,
        exported_at: now_rfc3339(),
        exported_by: format!("Jarlid {}", app.package_info().version),
        stations,
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Save station preferences")
        .set_file_name(format!("jarlid-stations-{}.json", today_stamp()))
        .add_filter("Jarlid export", &["json"])
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    let picked = rx.await.map_err(|_| "save dialog closed".to_string())?;

    let Some(picked) = picked else {
        return Ok(ExportResult {
            path: None,
            stations: file.stations.len(),
            thumbs,
            seeds,
            skipped,
            stopped_reason,
        });
    };
    let path = picked
        .into_path()
        .map_err(|e| format!("unusable save path: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("could not write {}: {e}", path.display()))?;

    Ok(ExportResult {
        path: Some(path.display().to_string()),
        stations: file.stations.len(),
        thumbs,
        seeds,
        skipped,
        stopped_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A `station.getStation` response with `includeExtendedAttributes`.
    ///
    /// The key set here is not invented — it mirrors what the live API returned
    /// on 2026-08-08, captured with
    /// `cargo run -p engine --example dump-station-shape` (which prints field
    /// names and types only, never anyone's listening data). Values are made up;
    /// the *shape* is real, which is the part worth pinning down. Notably a thumb
    /// carries **no `albumName`**, and does carry `musicToken`/`pandoraId`/
    /// `dateCreated`.
    fn sample() -> Value {
        json!({
            "stationId": "3427217006436217",
            "stationToken": "3427217006436217",
            "stationName": "Dove Cameron Radio",
            "artUrl": "https://cont/500W_500H.jpg",
            "dateCreated": { "time": 1600000000000i64 },
            "isShared": false,
            "isQuickMix": false,
            "isGenreStation": false,
            "allowAddMusic": true,
            "allowRename": true,
            "allowDelete": true,
            "allowEditDescription": true,
            "hasTakeoverModes": true,
            "hasCuratedModes": false,
            "modes": { "currentModeId": 3 },
            "genre": ["Pop"],
            "stationSharingUrl": "https://www.pandora.com/…",
            "music": {
                "songs": [{
                    "songName": "Boyfriend", "artistName": "Dove Cameron",
                    "seedId": "9034", "musicToken": "S1234", "artUrl": "https://a.jpg",
                    "pandoraId": "TR:1", "pandoraType": "TR"
                }],
                "artists": [{
                    "artistName": "Chappell Roan",
                    "seedId": "9035", "musicToken": "R5678", "artUrl": "https://b.jpg",
                    "pandoraId": "AR:2", "pandoraType": "AR"
                }],
                "genres": [{
                    "genreName": "Dance Pop", "seedId": "9036", "musicToken": "G9"
                }]
            },
            "feedback": {
                "totalThumbsUp": 2,
                "totalThumbsDown": 1,
                "thumbsUp": [{
                    "songName": "Good Luck, Babe!", "artistName": "Chappell Roan",
                    "songIdentity": "SI1", "feedbackId": "F1",
                    "albumArtUrl": "https://c.jpg", "musicToken": "S77",
                    "pandoraId": "TR:77", "pandoraType": "TR", "isPositive": true,
                    "dateCreated": { "time": 1700000000000i64 }
                }, {
                    "songName": "Espresso", "artistName": "Sabrina Carpenter",
                    "songIdentity": "SI2", "feedbackId": "F2"
                }],
                "thumbsDown": [{
                    "songName": "Nope", "artistName": "Someone",
                    "songIdentity": "SI3", "feedbackId": "F3", "isPositive": false
                }]
            }
        })
    }

    #[test]
    fn maps_seeds_of_every_kind() {
        let s = map_station("TOK", &sample());
        assert_eq!(s.seeds.len(), 3);

        let song = &s.seeds[0];
        assert_eq!(song.kind, "song");
        assert_eq!(song.name, "Boyfriend");
        assert_eq!(song.artist, "Dove Cameron", "a song seed keeps its artist");
        assert_eq!(
            song.music_token, "S1234",
            "musicToken is what re-import needs"
        );

        let artist = &s.seeds[1];
        assert_eq!(artist.kind, "artist");
        assert_eq!(artist.name, "Chappell Roan");
        assert!(
            artist.artist.is_empty(),
            "an artist seed has no separate artist"
        );

        let genre = &s.seeds[2];
        assert_eq!(genre.kind, "genre");
        assert_eq!(genre.name, "Dance Pop");
    }

    #[test]
    fn maps_both_thumb_polarities() {
        let s = map_station("TOK", &sample());
        assert_eq!(s.feedback.len(), 3);
        assert_eq!(s.feedback[0].rating, "up");
        assert_eq!(s.feedback[0].name, "Good Luck, Babe!");
        assert_eq!(s.feedback[0].feedback_id, "F1");
        assert_eq!(s.feedback[0].art, "https://c.jpg");
        assert_eq!(s.feedback[0].music_token, "S77");
        assert_eq!(s.feedback[0].pandora_id, "TR:77");
        assert_eq!(
            s.feedback[0].dated, "1700000000000",
            "when the thumb was given"
        );
        // A thumb without the optional extras must still map, not panic.
        assert_eq!(s.feedback[1].name, "Espresso");
        assert!(s.feedback[1].dated.is_empty());
        assert_eq!(s.feedback[2].rating, "down");
        assert_eq!(s.feedback[2].name, "Nope");
        assert_eq!(s.counts, Counts { up: 2, down: 1 });
    }

    #[test]
    fn maps_header_and_settings() {
        let s = map_station("TOK", &sample());
        assert_eq!(s.name, "Dove Cameron Radio");
        assert_eq!(s.station_id, "3427217006436217");
        assert_eq!(s.art, "https://cont/500W_500H.jpg");
        assert_eq!(s.date_created, "1600000000000");
        assert_eq!(s.settings.get("allowAddMusic"), Some(&json!(true)));
        assert_eq!(s.settings.get("genre"), Some(&json!(["Pop"])));
        assert_eq!(
            s.settings.get("modes"),
            Some(&json!({ "currentModeId": 3 })),
            "Discovery Tuner state is a real per-station setting"
        );
        // Absent keys must not appear as nulls...
        assert!(!s.settings.contains_key("isThumbprintStation"));
        // ...and keys we deliberately don't carry must stay out.
        assert!(!s.settings.contains_key("stationSharingUrl"));
    }

    /// A QuickMix is defined entirely by which stations it shuffles. Exporting
    /// one without that list would back up nothing at all.
    #[test]
    fn keeps_quickmix_membership() {
        let v = json!({
            "stationName": "QuickMix",
            "isQuickMix": true,
            "quickMixStationIds": ["111", "222"],
        });
        let s = map_station("TOK", &v);
        assert_eq!(
            s.settings.get("quickMixStationIds"),
            Some(&json!(["111", "222"]))
        );
    }

    /// The counts Pandora reports and the rows it hands over can disagree on a
    /// heavily-thumbed station. A backup that looks complete but isn't is the
    /// worst outcome here, so that has to be recorded.
    #[test]
    fn warns_when_pandora_returns_fewer_thumbs_than_it_claims() {
        let mut v = sample();
        v["feedback"]["totalThumbsUp"] = json!(500);
        let s = map_station("TOK", &v);
        assert_eq!(s.warnings.len(), 1);
        assert!(s.warnings[0].contains("500"), "got {:?}", s.warnings);

        // ...and stays quiet when they agree.
        assert!(map_station("TOK", &sample()).warnings.is_empty());
    }

    /// A station with no seeds/feedback keys at all must map to empty lists
    /// rather than panicking.
    #[test]
    fn tolerates_a_bare_response() {
        let s = map_station("TOK", &json!({ "stationName": "Bare" }));
        assert_eq!(s.name, "Bare");
        assert_eq!(s.station_token, "TOK", "falls back to the requested token");
        assert!(s.seeds.is_empty());
        assert!(s.feedback.is_empty());
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn export_file_is_camel_case_and_keeps_tokens() {
        let file = ExportFile {
            jarlid_export: SCHEMA_VERSION,
            exported_at: "2026-08-08T00:00:00Z".into(),
            exported_by: "Jarlid test".into(),
            stations: vec![map_station("TOK", &sample())],
        };
        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains("\"jarlidExport\":1"));
        assert!(json.contains("\"musicToken\":\"S1234\""));
        assert!(json.contains("\"feedbackId\":\"F1\""));
        assert!(json.contains("\"stationToken\":\"3427217006436217\""));
        // Empty optionals shouldn't clutter the file.
        assert!(
            !json.contains("\"album\""),
            "no album field: thumbs never carry one"
        );
    }

    #[test]
    fn timestamps_are_well_formed() {
        assert!(now_rfc3339().ends_with('Z'));
        assert_eq!(today_stamp().len(), 10);
    }
}
