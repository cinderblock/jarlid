//! Audio-pipeline spike. Deliberately uses the **anonymous** listener tier so it needs no
//! account and never touches the user's subscription — the codec is the same either way.
//!
//! Answers:
//!   1. Can we get a playable audio URL with a pure HTTP client and no browser?
//!   2. Is the stream AAC-LC or HE-AAC/HE-AACv2? (Decides whether Symphonia is viable at all.)
//!   3. Are any encryption boxes present? (Confirms there is no DRM to contend with.)
//!
//! Run: cargo run --bin audio-probe

use pandora::json::find_key;
use pandora::{mp4, rest};
use serde_json::{json, Value};

fn heading(text: &str) {
    println!("\n=== {text} ===");
}


#[tokio::main]
async fn main() {
    heading("1. anonymous login (no account touched)");
    let mut client = match rest::Client::connect().await {
        Ok(client) => client,
        Err(e) => {
            println!("FAILED to reach pandora.com: {e}");
            std::process::exit(1);
        }
    };

    match client.anonymous_login().await {
        Ok(result) => {
            println!("OK — anonymous listener created.");
            if let Some(config) = result.get("config") {
                for key in ["dailySkipLimit", "stationSkipLimit", "inactivityTimeout"] {
                    if let Some(value) = config.get(key) {
                        println!("  {key}: {value}");
                    }
                }
            }
        }
        Err(e) => {
            println!("FAILED: {e}");
            println!("=> If this is errorCode 1215, PerimeterX has widened beyond auth/login.");
            std::process::exit(1);
        }
    }

    heading("2. find something to play");
    let search = match client
        .call(
            "v1/search/fullSearch",
            json!({"query": "Pink Floyd", "types": ["AR"], "count": 3}),
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            println!("FAILED: {e}");
            std::process::exit(1);
        }
    };

    // The server ignores the `types` filter, so pick the artist ourselves rather than taking
    // whatever happens to sort first (which is often a composer or genre, and won't seed a station).
    let items = find_key(&search, "items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(artist) = items
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("artist"))
    else {
        println!("No artist in search results. Types present: {:?}",
            items.iter().filter_map(|i| i.get("type")).collect::<Vec<_>>());
        std::process::exit(1);
    };

    let seed = artist.get("musicId").and_then(Value::as_str).unwrap_or_default();
    let pandora_id = artist.get("pandoraId").and_then(Value::as_str).unwrap_or_default();
    println!(
        "OK — seed: {} (musicId {seed}, pandoraId {pandora_id})",
        artist.get("name").and_then(Value::as_str).unwrap_or("?")
    );

    // The public docs are 2021-vintage and the field names have drifted, so try the documented
    // shape first and fall back — reporting which one the server actually accepted.
    let candidates = [
        ("stationCode=musicId", json!({"stationCode": seed, "stationName": "", "searchQuery": ""})),
        ("pandoraId", json!({"pandoraId": pandora_id, "stationName": ""})),
        ("stationCode=pandoraId", json!({"stationCode": pandora_id, "stationName": ""})),
        ("musicId", json!({"musicId": seed})),
    ];

    let mut station = None;
    for (label, body) in candidates {
        match client.call("v1/station/createStation", body).await {
            Ok(result) => {
                println!("OK — createStation accepted `{label}`.");
                station = Some(result);
                break;
            }
            Err(e) => println!("  `{label}` rejected: {e}"),
        }
    }

    let Some(station) = station else {
        println!("\nEvery createStation shape was rejected. Search response shape:");
        println!("{}", serde_json::to_string_pretty(&search).unwrap_or_default());
        std::process::exit(1);
    };

    let Some(station_id) = find_key(&station, "stationId").and_then(Value::as_str) else {
        println!("No stationId returned. Response:");
        println!("{}", serde_json::to_string_pretty(&station).unwrap_or_default());
        std::process::exit(1);
    };
    println!("OK — station {station_id}");

    heading("3. playlist/getFragment");
    let fragment = match client
        .call(
            "v1/playlist/getFragment",
            json!({
                "stationId": station_id,
                "isStationStart": true,
                "fragmentRequestReason": "Normal",
                "audioFormat": "aacplus",
            }),
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            println!("FAILED: {e}");
            std::process::exit(1);
        }
    };

    let tracks = find_key(&fragment, "tracks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    println!("OK — {} items in fragment.", tracks.len());

    // The XOR-obfuscation path in Pandora's web player only activates when a `key` is supplied.
    let xor_key = find_key(&fragment, "key").is_some();
    println!("XOR `key` field present: {xor_key}");
    if xor_key {
        println!("  !! Audio is XOR-masked on this tier — the decode path must un-mask first.");
    }

    let Some(audio_url) = find_key(&fragment, "audioURL").and_then(Value::as_str) else {
        println!("No audioURL found. Track shape:");
        println!(
            "{}",
            serde_json::to_string_pretty(tracks.first().unwrap_or(&Value::Null)).unwrap_or_default()
        );
        std::process::exit(1);
    };

    if let Some(track) = tracks.first() {
        println!(
            "  first track: {} — {}",
            track.get("songTitle").and_then(Value::as_str).unwrap_or("?"),
            track.get("artistName").and_then(Value::as_str).unwrap_or("?"),
        );
        for key in ["audioEncoding", "trackLength", "trackType"] {
            if let Some(value) = track.get(key) {
                println!("  {key}: {value}");
            }
        }
    }
    // Host only — the token in the query string is a live credential.
    println!(
        "  audio host: {}",
        audio_url.split('/').nth(2).unwrap_or("?")
    );

    heading("4. fetch the container header");
    let http = reqwest::Client::new();
    let response = match http
        .get(audio_url)
        .header("Range", "bytes=0-32767")
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            println!("FAILED: {e}");
            std::process::exit(1);
        }
    };
    println!("status: {}", response.status());
    let header_value = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("?")
            .to_string()
    };
    println!("content-type: {}", header_value("content-type"));

    // "bytes 0-32767/1550611" — the total tells us where to look if moov isn't at the front.
    let total: Option<u64> = header_value("content-range")
        .split('/')
        .nth(1)
        .and_then(|s| s.parse().ok());
    let mut bytes = response.bytes().await.unwrap_or_default().to_vec();
    println!("got {} bytes of {}", bytes.len(), total.map(|t| t.to_string()).unwrap_or("?".into()));

    // Files not optimised for streaming put `moov` at the end. Fetch the tail if it's not up front.
    if !mp4::box_types(&bytes).iter().any(|b| b == "moov") {
        if let Some(total) = total {
            let start = total.saturating_sub(256 * 1024);
            println!("`moov` not in the head — fetching the tail from byte {start}…");
            if let Ok(tail) = http
                .get(audio_url)
                .header("Range", format!("bytes={start}-{}", total - 1))
                .send()
                .await
            {
                if let Ok(tail) = tail.bytes().await {
                    println!("got {} more bytes", tail.len());
                    bytes = tail.to_vec();
                }
            }
        }
    }

    heading("5. VERDICT — codec and encryption");
    let boxes = mp4::box_types(&bytes);
    println!("boxes: {}", boxes.join(" "));

    let encrypted: Vec<_> = mp4::ENCRYPTION_BOXES
        .iter()
        .filter(|name| boxes.iter().any(|b| b == *name))
        .collect();
    if encrypted.is_empty() {
        println!("\nDRM: none. No encryption boxes present — plain, playable MP4/AAC.");
    } else {
        println!("\nDRM: !! encryption boxes present: {encrypted:?}");
    }

    match mp4::audio_config(&bytes) {
        Some(config) => {
            println!("\ncodec:        {}", config.object_type);
            println!("output rate:  {} Hz", config.sample_rate);
            println!("channels:     {}", config.channels);
            if let Some(extension) = config.extension_sample_rate {
                println!("core rate:    {} Hz (SBR doubles it to {extension})", extension / 2);
            }

            println!();
            if config.object_type.needs_sbr() {
                println!("=> SBR IS REQUIRED. Symphonia would decode the core layer only:");
                println!("   {} Hz instead of {} Hz, no high band, audibly dull.",
                    config.sample_rate / 2, config.sample_rate);
                println!("   Confirms the decision: use Windows Media Foundation.");
            } else {
                println!("=> Plain AAC-LC — no SBR needed. Symphonia would be viable after all,");
                println!("   which would keep the audio path pure Rust and portable.");
                println!("   Worth revisiting the Media Foundation decision in the plan.");
            }
        }
        None => {
            println!("\nCould not parse the AudioSpecificConfig — the header may be truncated.");
            println!("Re-run requesting a larger Range if `moov` is missing from the box list.");
        }
    }
}
