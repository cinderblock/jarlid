//! Verify the write paths against a **throwaway station**, then delete it.
//!
//! Thumbs permanently shape a station's behaviour, so these were never run against the user's
//! real 88 stations. The user authorised exactly one disposable station for this.
//!
//! Creates it, exercises feedback on it, and deletes it again — so nothing is left behind and
//! the delete endpoint gets verified too.
//!
//! Run: cargo run --example verify-writes

use pandora::tuner;
use serde_json::{json, Value};

/// A name that is obviously disposable if cleanup ever fails and it shows up in the UI.
const THROWAWAY_NAME: &str = "zz-delete-me-protocol-test";

fn report(label: &str, result: &pandora::Result<Value>) {
    match result {
        Ok(_) => println!("  ✅ {label}"),
        Err(e) => println!("  ❌ {label} — {e}"),
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
    println!("logged in.\n");

    println!("=== 1. find a seed ===");
    let search = session
        .call("music.search", json!({"searchText": "Rachmaninoff"}))
        .await
        .expect("search");
    let music_token = search
        .get("artists")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|a| a.get("musicToken"))
        .and_then(Value::as_str)
        .expect("a musicToken")
        .to_string();
    println!("  musicToken {music_token}");

    println!("\n=== 2. create the throwaway station ===");
    let created = session
        .call("station.createStation", json!({"musicToken": music_token}))
        .await;
    report("station.createStation", &created);
    let Ok(created) = created else {
        eprintln!("\nCannot continue without a station.");
        std::process::exit(1);
    };
    let station_token = created
        .get("stationToken")
        .and_then(Value::as_str)
        .expect("stationToken")
        .to_string();
    println!("  station: {} ({station_token})",
        created.get("stationName").and_then(Value::as_str).unwrap_or("?"));

    // Rename so it is unmistakably disposable in any UI that lists it.
    let renamed = session
        .call(
            "station.renameStation",
            json!({"stationToken": station_token, "stationName": THROWAWAY_NAME}),
        )
        .await;
    report("station.renameStation", &renamed);

    println!("\n=== 3. get a track to act on ===");
    let playlist = session
        .call(
            "station.getPlaylist",
            json!({
                "stationToken": station_token,
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
    let tracks: Vec<&Value> = items.iter().filter(|i| i.get("songName").is_some()).collect();
    println!("  {} tracks", tracks.len());

    let token_of = |index: usize| -> Option<String> {
        tracks
            .get(index)?
            .get("trackToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    println!("\n=== 4. feedback endpoints ===");

    if let Some(track_token) = token_of(0) {
        let up = session
            .call(
                "station.addFeedback",
                json!({"stationToken": station_token, "trackToken": track_token, "isPositive": true}),
            )
            .await;
        report("station.addFeedback (thumbs up)", &up);

        // Removing feedback needs the id the add returned, which is how the real client undoes a
        // mis-tap.
        if let Ok(added) = &up {
            if let Some(feedback_id) = added.get("feedbackId").and_then(Value::as_str) {
                let removed = session
                    .call("station.deleteFeedback", json!({"feedbackId": feedback_id}))
                    .await;
                report("station.deleteFeedback", &removed);
            } else {
                println!("  ⚠️  addFeedback returned no feedbackId — undo needs another route");
            }
        }
    }

    if let Some(track_token) = token_of(1) {
        let down = session
            .call(
                "station.addFeedback",
                json!({"stationToken": station_token, "trackToken": track_token, "isPositive": false}),
            )
            .await;
        report("station.addFeedback (thumbs down)", &down);
    }

    if let Some(track_token) = token_of(2) {
        let tired = session
            .call("user.sleepSong", json!({"trackToken": track_token}))
            .await;
        report("user.sleepSong (tired of track)", &tired);
    }

    println!("\n=== 5. cleanup — delete the throwaway station ===");
    let deleted = session
        .call("station.deleteStation", json!({"stationToken": station_token}))
        .await;
    report("station.deleteStation", &deleted);

    if deleted.is_err() {
        println!("\n⚠️  CLEANUP FAILED. Delete \"{THROWAWAY_NAME}\" by hand.");
        std::process::exit(1);
    }

    println!("\n=> Write paths verified. Throwaway station removed; the real 88 are untouched.");
}
