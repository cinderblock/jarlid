//! Is `additionalAudioUrl` a *preference list* rather than a positional map?
//!
//! Cameron's hypothesis: Pandora isn't "dropping" specs so much as returning the subset it will
//! grant, in the order asked. If true, requesting a descending preference list and taking the
//! FIRST url yields the best available stream — and would auto-upgrade if 192 kbps were ever
//! granted, instead of being pinned to a hardcoded 128.
//!
//! The earlier finding (request 192,128,64,32 → three urls measuring 128,64,32) is consistent
//! with that, but consistent is not proof: it can't distinguish "request order preserved" from
//! "always descending". So ask for an ASCENDING list too. If the response comes back ascending,
//! order follows the request and taking [0] is only correct when we ask descending.
//!
//! Run: cargo run --example spec-order

use pandora::tuner;
use serde_json::{json, Value};

async fn measure_kbps(http: &reqwest::Client, url: &str, seconds: u64) -> u64 {
    if seconds == 0 || url.is_empty() {
        return 0;
    }
    let Ok(response) = http.get(url).header("Range", "bytes=0-0").send().await else {
        return 0;
    };
    response
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split('/').nth(1)?.parse::<u64>().ok())
        .map(|bytes| bytes * 8 / seconds / 1000)
        .unwrap_or(0)
}

async fn probe(session: &tuner::Session, http: &reqwest::Client, token: &str, specs: &str) {
    let playlist = match session
        .call(
            "station.getPlaylist",
            json!({
                "stationToken": token,
                "includeTrackLength": true,
                "additionalAudioUrl": specs,
            }),
        )
        .await
    {
        Ok(playlist) => playlist,
        Err(e) => {
            println!("  request failed: {e}");
            return;
        }
    };

    let Some(item) = playlist
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find(|i| i.get("songName").is_some()))
    else {
        println!("  no track returned");
        return;
    };

    let seconds = item.get("trackLength").and_then(Value::as_u64).unwrap_or(0);
    let urls: Vec<String> = match item.get("additionalAudioUrl") {
        Some(Value::String(url)) => vec![url.clone()],
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect(),
        _ => Vec::new(),
    };

    println!("  requested {} spec(s), got {} url(s)", specs.split(',').count(), urls.len());
    for (index, url) in urls.iter().enumerate() {
        println!("    [{index}] {} kbps", measure_kbps(http, url, seconds).await);
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
    let token = list
        .get("stations")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(|s| s.get("stationToken"))
        .and_then(Value::as_str)
        .expect("a station")
        .to_string();

    let http = reqwest::Client::new();

    println!("\nDESCENDING request (192,128,64):");
    probe(&session, &http, &token, "HTTP_192_MP3,HTTP_128_MP3,HTTP_64_AACPLUS_ADTS").await;

    println!("\nASCENDING request (64,128,192):");
    probe(&session, &http, &token, "HTTP_64_AACPLUS_ADTS,HTTP_128_MP3,HTTP_192_MP3").await;

    println!("\nIf ASCENDING comes back ascending, the response follows the REQUEST order, so");
    println!("asking descending and taking [0] always yields the best stream on offer.");
    println!("If both come back descending, Pandora sorts by quality and [0] is best regardless.");
}
