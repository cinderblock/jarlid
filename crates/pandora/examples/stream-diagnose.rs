//! Why does REST `playlist/getFragment` return STREAM_VIOLATION on this account?
//!
//! Jarlid is closed and no other client is streaming, yet it persists. Hypotheses:
//!
//!   H1. A stale server-side session is still holding the stream and needs time to lapse.
//!   H2. Playback is tied to session *type*: our token comes from a tuner (Android device) login,
//!       and Pandora may refuse REST/web playback on a device token while happily serving
//!       metadata with it. Metadata calls (getStations) demonstrably work.
//!
//! The control that separates them: tuner `station.getPlaylist` with the same session. If tuner
//! playback works while REST playback does not, H2 is the answer and the architecture needs
//! revisiting — REST would give us metadata but not audio.
//!
//! Run: cargo run --example stream-diagnose

use pandora::demo::find_key;
use pandora::{rest, tuner};
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

    heading("fresh tuner login");
    let mut session = tuner::Session::connect(&tuner::ANDROID).await.expect("partner");
    session.login(&username, &password).await.expect("user");
    let token = session.user_auth_token().expect("token").to_string();
    println!("OK");

    // Get a station id over the tuner API so we don't spend a REST call before the test.
    let list = session
        .call("user.getStationList", json!({}))
        .await
        .expect("station list");
    let stations = list
        .get("stations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let station = stations.first().expect("a station");
    let station_token = station
        .get("stationToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let station_id = station
        .get("stationId")
        .and_then(Value::as_str)
        .unwrap_or(&station_token)
        .to_string();
    println!("station: {}", station.get("stationName").and_then(Value::as_str).unwrap_or("?"));

    heading("TEST 1 — REST getFragment, first call of a fresh session");
    let rest_client = rest::Client::connect()
        .await
        .expect("rest")
        .with_auth_token(&token);

    let rest_body = json!({
        "stationId": station_id,
        "isStationStart": true,
        "fragmentRequestReason": "Normal",
        "audioFormat": "aacplus",
    });

    let first = rest_client.call("v1/playlist/getFragment", rest_body.clone()).await;
    match &first {
        Ok(_) => println!("OK — REST playback works. H1 and H2 both refuted."),
        Err(e) => println!("FAILED: {e}"),
    }

    heading("TEST 2 — control: tuner station.getPlaylist, same session");
    let tuner_playlist = session
        .call(
            "station.getPlaylist",
            json!({"stationToken": station_token, "includeTrackLength": true}),
        )
        .await;
    match &tuner_playlist {
        Ok(playlist) => {
            let count = playlist
                .get("items")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            println!("OK — tuner playback works ({count} items).");
        }
        Err(e) => println!("FAILED: {e}"),
    }

    heading("TEST 3 — REST getFragment again, after the tuner playlist");
    match rest_client.call("v1/playlist/getFragment", rest_body).await {
        Ok(_) => println!("OK"),
        Err(e) => println!("FAILED: {e}"),
    }

    heading("TEST 4 — control: anonymous REST session, same endpoint");
    // Proves the endpoint and our request shape are fine, isolating the problem to this account
    // or this session type.
    match rest::Client::connect().await {
        Ok(mut anon) => match anon.anonymous_login().await {
            Ok(_) => match pandora::demo::anonymous_track("Pink Floyd").await {
                Ok((track, _)) => println!(
                    "OK — anonymous REST playback works ({} s, encoding {})",
                    track.length_seconds, track.encoding
                ),
                Err(e) => println!("FAILED: {e}"),
            },
            Err(e) => println!("anonymous login FAILED: {e}"),
        },
        Err(e) => println!("connect FAILED: {e}"),
    }

    heading("VERDICT");
    match (first.is_ok(), tuner_playlist.is_ok()) {
        (true, _) => println!("REST playback works — no problem. Use REST for audio."),
        (false, true) => {
            println!("REST playback REFUSED while tuner playback WORKS on the same session.");
            println!("=> H2: playback is tied to session type. A tuner/device token buys metadata");
            println!("   over REST but not audio. Audio must come from the tuner API (64 kbps),");
            println!("   or the client must hold a genuine web session for playback.");
        }
        (false, false) => {
            println!("Both refused — the account's stream is held by something else, or a stale");
            println!("session has not lapsed. Retry in a few minutes before concluding anything.");
        }
    }

    if let Some(found) = find_key(&list, "stationCount") {
        println!("\n(stationCount: {found})");
    }
}
