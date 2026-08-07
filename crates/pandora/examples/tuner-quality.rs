//! Can the tuner API give this account better than 64 kbps?
//!
//! `station.getPlaylist` accepts an `additionalAudioUrl` parameter — a comma-separated list of
//! stream specs. Pandora One subscribers were historically served 192 kbps MP3 through it, and
//! this account reports `pandoraBrandingType: "p1"`, so it is worth testing before concluding
//! that 64 kbps is the ceiling.
//!
//! Bitrate is measured from the file, not read off a label.
//!
//! Run: cargo run --example tuner-quality

use pandora::tuner;
use serde_json::{json, Value};

/// Ask the CDN for one byte; `Content-Range` reveals the full size without downloading it.
async fn measure_kbps(http: &reqwest::Client, url: &str, seconds: u64) -> String {
    if seconds == 0 {
        return "?".into();
    }
    let Ok(response) = http.get(url).header("Range", "bytes=0-0").send().await else {
        return "unreachable".into();
    };
    let total = response
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split('/').nth(1)?.parse::<u64>().ok());

    match total {
        Some(bytes) => format!("{} kbps", bytes * 8 / seconds / 1000),
        None => format!("no size (status {})", response.status()),
    }
}

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD.");
        std::process::exit(2);
    };

    let mut session = tuner::Session::connect(&tuner::ANDROID).await.expect("partner");
    session.login(&username, &password).await.expect("user");

    let list = session
        .call("user.getStationList", json!({}))
        .await
        .expect("stations");
    let station_token = list
        .get("stations")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(|s| s.get("stationToken"))
        .and_then(Value::as_str)
        .expect("a station")
        .to_string();

    // The tuner API's documented stream specs, highest first.
    let specs = "HTTP_192_MP3,HTTP_128_MP3,HTTP_64_AACPLUS_ADTS,HTTP_32_AACPLUS_ADTS";

    let playlist = session
        .call(
            "station.getPlaylist",
            json!({
                "stationToken": station_token,
                "includeTrackLength": true,
                "additionalAudioUrl": specs,
            }),
        )
        .await
        .expect("playlist");

    let items = playlist
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let http = reqwest::Client::new();

    for item in items.iter().take(2) {
        let Some(song) = item.get("songName").and_then(Value::as_str) else {
            continue;
        };
        let seconds = item
            .get("trackLength")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        println!("\n=== {song} ({seconds} s) ===");

        println!("\n  audioUrlMap (the standard set):");
        if let Some(map) = item.get("audioUrlMap").and_then(Value::as_object) {
            for (quality, detail) in map {
                let url = detail.get("audioUrl").and_then(Value::as_str).unwrap_or("");
                println!(
                    "    {quality:<16} claims {:>4} kbps {:<10} measured {}",
                    detail.get("bitrate").and_then(Value::as_str).unwrap_or("?"),
                    detail.get("encoding").and_then(Value::as_str).unwrap_or("?"),
                    measure_kbps(&http, url, seconds).await
                );
            }
        }

        // additionalAudioUrl comes back as a bare string for one spec, or an array for several,
        // positionally matching the request order.
        println!("\n  additionalAudioUrl (requested {specs}):");
        match item.get("additionalAudioUrl") {
            Some(Value::String(url)) => {
                println!("    [0] measured {}", measure_kbps(&http, url, seconds).await);
            }
            Some(Value::Array(urls)) => {
                for (index, url) in urls.iter().enumerate() {
                    let spec = specs.split(',').nth(index).unwrap_or("?");
                    match url.as_str().filter(|u| !u.is_empty()) {
                        Some(url) => println!(
                            "    {spec:<24} measured {}",
                            measure_kbps(&http, url, seconds).await
                        ),
                        None => println!("    {spec:<24} (empty — not available to this account)"),
                    }
                }
            }
            _ => println!("    (field absent — the account gets no additional streams)"),
        }
    }

    // Requesting several specs at once is ambiguous: Pandora appears to DROP unavailable entries
    // rather than returning empty slots, so the array no longer lines up with the request order
    // and every reading looks shifted. Ask for exactly one spec at a time to remove all doubt.
    println!("\n\n=== unambiguous: one spec per request ===\n");
    for spec in [
        "HTTP_192_MP3",
        "HTTP_128_MP3",
        "HTTP_64_AACPLUS_ADTS",
        "HTTP_24_AACPLUS_ADTS",
    ] {
        let playlist = session
            .call(
                "station.getPlaylist",
                json!({
                    "stationToken": station_token,
                    "includeTrackLength": true,
                    "additionalAudioUrl": spec,
                }),
            )
            .await;

        let Ok(playlist) = playlist else {
            println!("  {spec:<24} request failed: {:?}", playlist.err());
            continue;
        };

        let Some(item) = playlist
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.iter().find(|i| i.get("songName").is_some()))
        else {
            println!("  {spec:<24} no track returned");
            continue;
        };

        let seconds = item.get("trackLength").and_then(Value::as_u64).unwrap_or(0);
        match item.get("additionalAudioUrl") {
            Some(Value::String(url)) if !url.is_empty() => {
                println!(
                    "  {spec:<24} AVAILABLE — measured {}",
                    measure_kbps(&http, url, seconds).await
                );
            }
            Some(Value::Array(urls)) => match urls.first().and_then(Value::as_str) {
                Some(url) if !url.is_empty() => println!(
                    "  {spec:<24} AVAILABLE — measured {}",
                    measure_kbps(&http, url, seconds).await
                ),
                _ => println!("  {spec:<24} NOT AVAILABLE (empty array)"),
            },
            _ => println!("  {spec:<24} NOT AVAILABLE (field absent)"),
        }
    }

    println!("\nMeasured = total bytes x 8 / track seconds. Ground truth, not a label.");
}
