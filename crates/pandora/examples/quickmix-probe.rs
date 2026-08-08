//! What is QuickMix, really — and can we tell which underlying station a track came from?
//!
//! Three questions:
//!   1. How does the API mark QuickMix (and other special stations) apart from normal ones?
//!   2. Does QuickMix expose Modes? (If not, the mode chip must hide rather than show a
//!      one-item picker.)
//!   3. Does a QuickMix track say which of the contributing stations produced it?
//!
//! Run: cargo run --example quickmix-probe

use serde_json::{json, Value};

fn heading(text: &str) {
    println!("\n=== {text} ===");
}

/// Print every key whose name hints at station identity or specialness.
fn interesting_keys(value: &Value, prefix: &str) {
    if let Some(map) = value.as_object() {
        for (key, val) in map {
            let lower = key.to_lowercase();
            let hit = lower.contains("station")
                || lower.contains("quickmix")
                || lower.contains("shuffle")
                || lower.contains("thumbprint")
                || lower.contains("genre")
                || lower.starts_with("is")
                || lower.contains("seed");
            if hit && !val.is_object() && !val.is_array() {
                println!("  {prefix}{key}: {val}");
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD.");
        std::process::exit(2);
    };

    let mut client = pandora::Client::login(&username, &password).await.expect("login");

    heading("1. how the tuner station list marks special stations");
    let list = client
        .tuner_call("user.getStationList", json!({ "includeStationArtUrl": true }))
        .await
        .expect("station list");

    let stations = list
        .get("stations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut quickmix_token = None;
    for station in &stations {
        let name = station.get("stationName").and_then(Value::as_str).unwrap_or("?");
        let special = station
            .as_object()
            .map(|m| {
                m.iter()
                    .filter(|(k, v)| k.starts_with("is") && v.as_bool() == Some(true))
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if !special.is_empty() {
            println!("  {name}: {}", special.join(", "));
        }
        if station.get("isQuickMix").and_then(Value::as_bool) == Some(true) {
            quickmix_token = station
                .get("stationToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            println!("\n  full QuickMix entry:");
            interesting_keys(station, "    ");
            // Which stations feed it.
            if let Some(ids) = station.get("quickMixStationIds").and_then(Value::as_array) {
                println!("    quickMixStationIds: {} stations", ids.len());
            }
        }
    }

    let Some(quickmix_token) = quickmix_token else {
        println!("\nNo station flagged isQuickMix — check the field name.");
        return;
    };

    heading("2. does QuickMix have Modes?");
    match client.station_modes(&quickmix_token).await {
        Ok(modes) if modes.is_empty() => {
            println!("  none — the mode picker must stay hidden for QuickMix.");
        }
        Ok(modes) => {
            println!("  {} modes:", modes.len());
            for mode in &modes {
                println!("    [{}] {}", mode.mode_id, mode.label());
            }
        }
        Err(e) => println!("  error (treat as 'no modes'): {e}"),
    }

    heading("3. does a QuickMix track name its source station?");
    let playlist = client
        .tuner_call(
            "station.getPlaylist",
            json!({
                "stationToken": quickmix_token,
                "includeTrackLength": true,
                "additionalAudioUrl": "HTTP_128_MP3",
            }),
        )
        .await
        .expect("playlist");

    let items = playlist
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Build a token -> name map so any station reference in a track can be resolved.
    let names: std::collections::HashMap<String, String> = stations
        .iter()
        .filter_map(|s| {
            Some((
                s.get("stationId").and_then(Value::as_str)?.to_string(),
                s.get("stationName").and_then(Value::as_str)?.to_string(),
            ))
        })
        .collect();

    for item in items.iter().take(3) {
        let Some(song) = item.get("songName").and_then(Value::as_str) else {
            continue;
        };
        println!("\n  {song}");
        interesting_keys(item, "    ");

        // Anything that looks like a station id → resolve it to a name.
        for key in ["stationId", "sourceStationId", "originalStationId"] {
            if let Some(id) = item.get(key).and_then(Value::as_str) {
                println!(
                    "    => {key} resolves to: {}",
                    names.get(id).map(String::as_str).unwrap_or("(unknown)")
                );
            }
        }
    }
}
