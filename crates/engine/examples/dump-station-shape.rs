//! Print the *shape* of `station.getStation` with `includeExtendedAttributes` — the response the
//! station-preferences export is built on.
//!
//! Same spirit as `pandora`'s `dump-shapes`: prints structure, not content. No station names, no
//! song titles, no tokens — only key names, value types and list lengths. That is enough to
//! confirm the export mapping reads the right fields, without putting anyone's listening history
//! in a terminal or a commit.
//!
//! Read-only: it calls `getStation` and nothing else. It never fetches a playlist, so it cannot
//! take the account's single permitted stream away from a running player.
//!
//! Run: cargo run -p engine --example dump-station-shape

use std::collections::BTreeSet;

use serde_json::Value;

/// Describe a value by its type, so the schema is learned without printing the data.
fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(n) if n.is_i64() || n.is_u64() => "int".into(),
        Value::Number(_) => "float".into(),
        Value::String(s) => format!("string [{} chars]", s.len()),
        Value::Array(items) => match items.first() {
            Some(first) => format!("array[{}] of {}", items.len(), describe(first)),
            None => "array[0]".into(),
        },
        Value::Object(map) => format!("object {{{}}}", map.len()),
    }
}

fn keys_of(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Report whether each field the exporter reads is actually present.
fn check(label: &str, present: &BTreeSet<String>, wanted: &[&str]) {
    println!("\n{label}");
    for field in wanted {
        let mark = if present.contains(*field) {
            "ok  "
        } else {
            "MISSING"
        };
        println!("  [{mark}] {field}");
    }
    let extra: Vec<&String> = present
        .iter()
        .filter(|k| !wanted.contains(&k.as_str()))
        .collect();
    if !extra.is_empty() {
        println!("  (also present, unused: {extra:?})");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(saved) = engine::credentials::load()? else {
        eprintln!("No saved credentials — sign in through Jarlid first.");
        std::process::exit(1);
    };

    let mut client = pandora::Client::login(&saved.username, &saved.password).await?;
    let stations = client.station_list().await?;
    println!("stations in collection: {}", stations.len());

    // Look for the RICHEST station, not merely the first usable one. Two traps:
    //
    // - the first station is typically QuickMix, a shuffle *over* other stations, which has no
    //   seeds and no thumbs of its own — reading the shape off that one wrongly suggests
    //   `includeExtendedAttributes` does nothing at all;
    // - an empty `thumbsUp` array proves nothing about the field names inside it, and those names
    //   are the single most important thing the exporter depends on.
    let len = |v: &Value, path: [&str; 2]| -> usize {
        v.get(path[0])
            .and_then(|p| p.get(path[1]))
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0)
    };

    let mut details = Value::Null;
    let mut best_score = -1i64;
    for (index, station) in stations.iter().enumerate().take(14) {
        // The list now carries `isQuickMix`, so a shuffle station can be skipped without
        // spending a request to discover it has no seeds or thumbs.
        if station.is_quick_mix {
            println!("  station #{index}: quickMix — skipped (no seeds or thumbs of its own)");
            continue;
        }
        let candidate = client.station_details(&station.station_token).await?;
        let quick_mix = station.is_quick_mix;
        let up = len(&candidate, ["feedback", "thumbsUp"]);
        let down = len(&candidate, ["feedback", "thumbsDown"]);
        let artists = len(&candidate, ["music", "artists"]);
        let songs = len(&candidate, ["music", "songs"]);
        println!(
            "  station #{index}: quickMix={quick_mix} seeds(song/artist)={songs}/{artists} thumbs(up/down)={up}/{down}"
        );

        // Prefer thumbs above all, then artist seeds — those are the unverified shapes.
        let score = (up + down) as i64 * 10 + artists as i64;
        if !quick_mix && score > best_score {
            best_score = score;
            details = candidate;
        }
        // Enough to prove the shape without walking the whole collection.
        if best_score >= 10 && artists > 0 {
            break;
        }
        // One request per station, spaced — same politeness the exporter uses.
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    }
    if details.is_null() {
        eprintln!("No stations on this account.");
        return Ok(());
    }
    let details = details;

    println!("\n--- top level ---");
    if let Some(map) = details.as_object() {
        for (key, value) in map {
            println!("  {key}: {}", describe(value));
        }
    }

    let music = details.get("music");
    println!("\n--- music ---");
    if let Some(map) = music.and_then(Value::as_object) {
        for (key, value) in map {
            println!("  {key}: {}", describe(value));
        }
    } else {
        println!("  (absent — includeExtendedAttributes may not have applied)");
    }

    let feedback = details.get("feedback");
    println!("\n--- feedback ---");
    if let Some(map) = feedback.and_then(Value::as_object) {
        for (key, value) in map {
            println!("  {key}: {}", describe(value));
        }
    } else {
        println!("  (absent — includeExtendedAttributes may not have applied)");
    }

    // The point of the exercise: do the exporter's field names exist? Keep these lists in step
    // with `map_station` in `app/src-tauri/src/export.rs`, so re-running this is a genuine
    // contract check rather than a description of what the code used to read.
    check(
        "station header (export reads):",
        &keys_of(Some(&details)),
        &[
            "stationId",
            "stationToken",
            "stationName",
            "artUrl",
            "dateCreated",
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
            "modes",
            "quickMixStationIds",
        ],
    );

    let first = |parent: Option<&Value>, key: &str| -> BTreeSet<String> {
        keys_of(
            parent
                .and_then(|p| p.get(key))
                .and_then(Value::as_array)
                .and_then(|a| a.first()),
        )
    };

    let seed_fields = &["musicToken", "pandoraId", "seedId", "artUrl"];
    check(
        "song seed (music.songs[0]):",
        &first(music, "songs"),
        &[&["songName", "artistName"][..], seed_fields].concat(),
    );
    check(
        "artist seed (music.artists[0]):",
        &first(music, "artists"),
        &[&["artistName"][..], seed_fields].concat(),
    );
    // Genre seeds are rare; `genreName` here is a guess the exporter hedges against by trying
    // several spellings. If a station ever does have one, this is where to confirm the real name.
    check(
        "genre seed (music.genres[0]) — name field UNCONFIRMED:",
        &first(music, "genres"),
        &[&["genreName", "name", "stationName"][..], seed_fields].concat(),
    );

    let thumb_fields = &[
        "songName",
        "artistName",
        "songIdentity",
        "pandoraId",
        "musicToken",
        "feedbackId",
        "dateCreated",
        "albumArtUrl",
    ];
    check(
        "thumb up (feedback.thumbsUp[0]):",
        &first(feedback, "thumbsUp"),
        thumb_fields,
    );
    check(
        "thumb down (feedback.thumbsDown[0]):",
        &first(feedback, "thumbsDown"),
        thumb_fields,
    );

    // Totals vs. rows actually returned — the export warns when these disagree, so confirm
    // whether getStation caps what it hands over.
    let count = |key: &str| {
        feedback
            .and_then(|f| f.get(key))
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0)
    };
    let total = |key: &str| {
        feedback
            .and_then(|f| f.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    println!(
        "\nthumbs returned: up={} down={}   totals reported: up={} down={}",
        count("thumbsUp"),
        count("thumbsDown"),
        total("totalThumbsUp"),
        total("totalThumbsDown"),
    );

    Ok(())
}
