//! Can we support Pandora's station Modes (My Station / Crowd Faves / Discovery / Deep Cuts …)?
//!
//! Endpoints extracted from today's shipping bundle:
//!   v1/interactiveradio/getAvailableModesSimple
//!   v1/interactiveradio/setAndGetAvailableModes    (note: their constant has a TRAILING SPACE)
//!   v1/action/mode
//!
//! Two questions, and the second is the one that decides the feature:
//!   1. Can we list the modes for a station with our tuner-derived token?
//!   2. **Does setting a mode change what tuner `station.getPlaylist` returns?**
//!      Modes are an "interactive radio" REST concept; our audio comes from the tuner API. If
//!      the two don't talk, mode selection would silently do nothing — worse than not shipping it.
//!
//! Run: cargo run --example modes-probe

use pandora::demo::find_key;
use serde_json::{json, Value};

fn heading(text: &str) {
    println!("\n=== {text} ===");
}

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD.");
        std::process::exit(2);
    };

    let mut client = pandora::Client::login(&username, &password).await.expect("login");

    // The REST station list carries `stationId`; the tuner list carries `stationToken`. Modes are
    // a REST concept, so we need both to test whether they interact.
    let rest_stations = client.stations().await.expect("rest stations");
    let station = rest_stations.first().expect("a station");
    println!("station: {} (id {})", station.name, station.station_id);

    let tuner_stations = client.tuner_stations().await.expect("tuner stations");
    let tuner_token = tuner_stations
        .iter()
        .find(|(name, _)| *name == station.name)
        .map(|(_, token)| token.clone());
    println!("matching tuner token: {}", tuner_token.as_deref().unwrap_or("NOT FOUND"));

    heading("1. list available modes");
    // Field names unknown; try the plausible shapes and report which the server accepts.
    let candidates = [
        ("stationId", json!({ "stationId": station.station_id })),
        ("pandoraId", json!({ "pandoraId": station.station_id })),
        (
            "stationId+listener",
            json!({ "stationId": station.station_id, "includeExtendedAttributes": true }),
        ),
    ];

    let mut modes_response = None;
    for (label, body) in candidates {
        match client
            .rest_call("v1/interactiveradio/getAvailableModesSimple", body)
            .await
        {
            Ok(result) => {
                println!("  ✅ accepted `{label}`");
                modes_response = Some(result);
                break;
            }
            Err(e) => println!("  ❌ `{label}`: {e}"),
        }
    }

    let Some(modes) = modes_response else {
        println!("\nCould not list modes. Modes may be unavailable to this account or need a");
        println!("web session rather than a tuner-derived token.");
        std::process::exit(1);
    };

    println!("\n{}", serde_json::to_string_pretty(&modes).unwrap_or_default());

    // Pull out (modeId, modeName) pairs however they happen to be nested.
    let list = find_key(&modes, "availableModes")
        .or_else(|| find_key(&modes, "modes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    println!("\nmodes found: {}", list.len());
    for mode in &list {
        println!(
            "  [{}] {}",
            mode.get("modeId").map(|v| v.to_string()).unwrap_or("?".into()),
            mode.get("modeName").and_then(Value::as_str).unwrap_or("?"),
        );
    }

    let Some(target) = list
        .iter()
        .find(|m| m.get("modeName").and_then(Value::as_str).is_some_and(|n| n != "My Station"))
    else {
        println!("\nNo non-default mode to switch to; stopping.");
        return;
    };
    let mode_id = target.get("modeId").cloned().unwrap_or(Value::Null);
    let mode_name = target.get("modeName").and_then(Value::as_str).unwrap_or("?");

    heading(&format!("2. switch to {mode_name:?} (modeId {mode_id})"));
    let set_candidates = [
        (
            "setAndGetAvailableModes",
            "v1/interactiveradio/setAndGetAvailableModes",
            json!({ "stationId": station.station_id, "modeId": mode_id }),
        ),
        (
            "action/mode",
            "v1/action/mode",
            json!({ "stationId": station.station_id, "modeId": mode_id }),
        ),
    ];

    let mut switched = false;
    for (label, endpoint, body) in set_candidates {
        match client.rest_call(endpoint, body).await {
            Ok(_) => {
                println!("  ✅ `{label}` accepted");
                switched = true;
                break;
            }
            Err(e) => println!("  ❌ `{label}`: {e}"),
        }
    }

    if !switched {
        println!("\nCould not set a mode. Feature would need REST playback, which is refused on");
        println!("a tuner token — see plans/pandora-native-client.md.");
        return;
    }

    heading("3. THE DECIDER — does the tuner playlist respect the mode?");
    let Some(token) = tuner_token else {
        println!("No tuner token for this station; cannot test.");
        return;
    };

    match client.playlist(&token).await {
        Ok(tracks) => {
            println!("tuner playlist after switching to {mode_name:?}:");
            for track in tracks.iter().take(4) {
                println!("  {}", track.describe());
            }
            println!("\nCompare against the same station in My Station mode. If the mode is");
            println!("honoured, the mix should visibly differ (Deep Cuts especially).");
        }
        Err(e) if e.is_stream_violation() => {
            println!("STREAM_VIOLATION — close other Pandora players and retry.");
        }
        Err(e) => println!("playlist failed: {e}"),
    }

    heading("4. restore My Station");
    if let Some(default) = list
        .iter()
        .find(|m| m.get("modeName").and_then(Value::as_str) == Some("My Station"))
    {
        let default_id = default.get("modeId").cloned().unwrap_or(Value::Null);
        match client
            .rest_call(
                "v1/interactiveradio/setAndGetAvailableModes",
                json!({ "stationId": station.station_id, "modeId": default_id }),
            )
            .await
        {
            Ok(_) => println!("  ✅ restored — the station is back how it was"),
            Err(e) => println!("  ⚠️  could not restore: {e} (set it back in the app)"),
        }
    }
}
