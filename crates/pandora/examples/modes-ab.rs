//! Does the **tuner** playlist actually respect a mode set over **REST**?
//!
//! Modes are an "interactive radio" REST concept; our audio comes from the tuner API. If the two
//! don't talk, a mode picker in the UI would silently do nothing — worse than not shipping it.
//!
//! Method: for each mode, set it, pull several playlists, and collect the artists. Playlist
//! generation is stochastic, so no single sample proves anything; what we're looking for is a
//! clear difference in *character* between modes — Discovery should surface artists My Station
//! doesn't, and Deep Cuts should avoid the obvious hits.
//!
//! Run: cargo run --example modes-ab

use std::collections::BTreeSet;

use pandora::demo::find_key;
use serde_json::{json, Value};

/// Playlists per mode. More samples is a better signal, but each one consumes skips against the
/// account, so keep it modest.
const SAMPLES: usize = 3;

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD.");
        std::process::exit(2);
    };

    let mut client = pandora::Client::login(&username, &password).await.expect("login");

    let stations = client.stations().await.expect("stations");
    let station = stations.first().expect("a station").clone();
    println!("station: {} ({})\n", station.name, station.station_id);

    let modes = client
        .rest_call(
            "v1/interactiveradio/getAvailableModesSimple",
            json!({ "stationId": station.station_id }),
        )
        .await
        .expect("modes");

    let available = find_key(&modes, "availableModes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut collected: Vec<(String, BTreeSet<String>, usize)> = Vec::new();

    for mode in &available {
        let Some(name) = mode.get("modeName").and_then(Value::as_str) else {
            continue;
        };
        if mode.get("isModeAvailable").and_then(Value::as_bool) != Some(true) {
            println!("skipping {name:?} — not available to this account");
            continue;
        }
        let mode_id = mode.get("modeId").cloned().unwrap_or(Value::Null);

        if let Err(e) = client
            .rest_call(
                "v1/interactiveradio/setAndGetAvailableModes",
                json!({ "stationId": station.station_id, "modeId": mode_id }),
            )
            .await
        {
            println!("could not set {name:?}: {e}");
            continue;
        }

        let mut artists = BTreeSet::new();
        let mut tracks = 0usize;
        for _ in 0..SAMPLES {
            match client.playlist(&station.station_id).await {
                Ok(batch) => {
                    tracks += batch.len();
                    for track in batch {
                        artists.insert(track.artist_name);
                    }
                }
                Err(e) if e.is_stream_violation() => {
                    println!("STREAM_VIOLATION — close other Pandora players and retry.");
                    std::process::exit(1);
                }
                Err(e) => println!("  playlist failed: {e}"),
            }
        }

        println!("{name:<14} {tracks:>2} tracks, {:>2} distinct artists", artists.len());
        collected.push((name.to_string(), artists, tracks));
    }

    println!("\n=== artists per mode ===");
    for (name, artists, _) in &collected {
        let mut sample: Vec<&str> = artists.iter().map(String::as_str).collect();
        sample.truncate(8);
        println!("\n{name}:\n  {}", sample.join(", "));
    }

    // The clearest evidence: artists that appear in one mode and in none of the others.
    println!("\n=== artists unique to each mode ===");
    for (name, artists, _) in &collected {
        let others: BTreeSet<&String> = collected
            .iter()
            .filter(|(other, _, _)| other != name)
            .flat_map(|(_, set, _)| set.iter())
            .collect();
        let unique: Vec<&str> = artists
            .iter()
            .filter(|a| !others.contains(a))
            .map(String::as_str)
            .collect();
        println!("{name:<14} {:>2} unique  {}", unique.len(), {
            let mut s = unique.clone();
            s.truncate(5);
            s.join(", ")
        });
    }

    println!("\n=== verdict ===");
    let total_unique: usize = collected
        .iter()
        .map(|(name, artists, _)| {
            let others: BTreeSet<&String> = collected
                .iter()
                .filter(|(other, _, _)| other != name)
                .flat_map(|(_, set, _)| set.iter())
                .collect();
            artists.iter().filter(|a| !others.contains(a)).count()
        })
        .sum();

    if total_unique == 0 {
        println!("Every mode returned the same artists. Strong evidence the tuner playlist");
        println!("IGNORES the REST mode — do NOT ship a mode picker on this path.");
    } else {
        println!("{total_unique} artists appeared in only one mode. Consistent with the tuner");
        println!("playlist honouring the REST mode — though playlists are stochastic, so treat");
        println!("this as evidence rather than proof, and sanity-check by ear.");
    }

    // Always leave the station as we found it.
    if let Some(default) = available
        .iter()
        .find(|m| m.get("isInitialMode").and_then(Value::as_bool) == Some(true))
    {
        let id = default.get("modeId").cloned().unwrap_or(Value::Null);
        match client
            .rest_call(
                "v1/interactiveradio/setAndGetAvailableModes",
                json!({ "stationId": station.station_id, "modeId": id }),
            )
            .await
        {
            Ok(_) => println!("\nrestored {:?}", default.get("modeName").and_then(Value::as_str).unwrap_or("?")),
            Err(e) => println!("\n⚠️  could not restore the default mode: {e}"),
        }
    }
}
