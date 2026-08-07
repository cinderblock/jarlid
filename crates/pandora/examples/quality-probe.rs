//! What is the best audio Pandora will actually give this account?
//!
//! The tuner API caps at 64 kbps. REST may do better, and `audioFormat` is a request parameter —
//! so try the plausible values and *measure* the result rather than trusting the label. Real
//! bitrate is derived from the file's total size over the track's duration, which cannot lie.
//!
//! Run: cargo run --example quality-probe

use pandora::demo::find_key;
use serde_json::{json, Value};

/// Ask the CDN for one byte; the `Content-Range` total tells us the full size without downloading.
async fn total_size(http: &reqwest::Client, url: &str) -> Option<u64> {
    let response = http.get(url).header("Range", "bytes=0-0").send().await.ok()?;
    response
        .headers()
        .get("content-range")?
        .to_str()
        .ok()?
        .split('/')
        .nth(1)?
        .parse()
        .ok()
}

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD.");
        std::process::exit(2);
    };

    let mut client = pandora::Client::login(&username, &password)
        .await
        .expect("login");
    let stations = client.stations().await.expect("stations");
    let station = stations.first().expect("at least one station");
    println!("using station: {}\n", station.name);

    let http = reqwest::Client::new();

    // Values Pandora's own clients are known to use, plus the tuner API's high-bitrate names.
    let formats = [
        "aacplus",
        "aacplus_adts",
        "mp3",
        "HTTP_128_MP3",
        "HTTP_192_MP3",
        "HTTP_64_AACPLUS_ADTS",
        "flac",
    ];

    println!("{:<24} {:<12} {:>10} {:>12}", "requested", "returned", "reported", "MEASURED");
    println!("{}", "-".repeat(62));

    for format in formats {
        // Superseded by `tuner-quality`: REST playback turned out to be refused on a tuner token
        // (STREAM_VIOLATION), so this probe now only documents that dead end.
        let fragment = client
            .rest_call(
                "v1/playlist/getFragment",
                json!({
                    "stationId": station.station_id,
                    "isStationStart": true,
                    "fragmentRequestReason": "Normal",
                    "audioFormat": format,
                }),
            )
            .await;

        let fragment = match fragment {
            Ok(fragment) => fragment,
            Err(e) => {
                println!("{format:<24} ERROR: {e}");
                continue;
            }
        };

        let Some(track) = find_key(&fragment, "tracks")
            .and_then(Value::as_array)
            .and_then(|tracks| {
                tracks
                    .iter()
                    .find(|t| t.get("audioURL").and_then(Value::as_str).is_some())
                    .cloned()
            })
        else {
            println!("{format:<24} (no playable track returned)");
            continue;
        };

        let encoding = track
            .get("audioEncoding")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let reported = track
            .get("bitrate")
            .map(|b| b.to_string())
            .unwrap_or_else(|| "—".into());
        let length = track.get("trackLength").and_then(Value::as_u64).unwrap_or(0);
        let url = track.get("audioURL").and_then(Value::as_str).unwrap_or("");

        let measured = match (total_size(&http, url).await, length) {
            (Some(bytes), seconds) if seconds > 0 => {
                format!("{} kbps", bytes * 8 / seconds / 1000)
            }
            _ => "?".into(),
        };

        println!("{format:<24} {encoding:<12} {reported:>10} {measured:>12}");
    }

    println!("\nMEASURED is ground truth: total bytes x 8 / track seconds.");
}
