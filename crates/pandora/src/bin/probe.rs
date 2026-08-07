//! Protocol spike. Answers the questions that gate the native-client architecture:
//!
//!   1. Does tuner `auth.userLogin` still work with a real *paid* account in 2026?
//!   2. What audio quality/encodings does the tuner API hand a paid account?
//!   3. Does a fragment for a paid account carry a `key` field (the XOR-obfuscation path)?
//!   4. **Does the tuner's userAuthToken work as `X-AuthToken` on the modern web REST API?**
//!
//! (4) is the big one: if yes, we get the rich REST surface with no browser and no bot wall.
//! If no, we fall back to lifting a token from a one-time browser login.
//!
//! Credentials come from the environment so they never land in a file or a transcript:
//!
//!   $env:PANDORA_USERNAME = 'you@example.com'
//!   $env:PANDORA_PASSWORD = 'hunter2'
//!   cargo run --bin probe

use pandora::{rest, tuner};
use serde_json::{json, Value};

/// Load `.env` / `.env.local` into the environment, walking up from the working directory.
///
/// Exists because `wenv` exports into *its* shell session, which a separately-launched process
/// does not inherit. Nearest file wins, but empty values never override a real one, so a stale
/// half-filled `.env` in a subdirectory can't shadow the real one at the repo root.
fn load_dotenv() {
    let Ok(start) = std::env::current_dir() else {
        return;
    };

    for directory in start.ancestors() {
        for name in [".env.local", ".env"] {
            let Ok(contents) = std::fs::read_to_string(directory.join(name)) else {
                continue;
            };
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let key = key.trim();
                let value = value.trim().trim_matches(['"', '\'']);
                if value.is_empty() || std::env::var_os(key).is_some_and(|v| !v.is_empty()) {
                    continue;
                }
                // SAFETY: single-threaded startup, before any threads are spawned.
                unsafe { std::env::set_var(key, value) };
            }
        }
    }
}

/// Show enough of a token to correlate across calls, never enough to use.
fn redact(token: &str) -> String {
    let head: String = token.chars().take(6).collect();
    format!("{head}… ({} chars)", token.len())
}

fn heading(text: &str) {
    println!("\n=== {text} ===");
}

#[tokio::main]
async fn main() {
    load_dotenv();

    // Step 1 needs no account, so run it first: it independently proves the tuner API is alive
    // and that our Blowfish codec round-trips against the real server (syncTime decrypts).
    heading("1. tuner auth.partnerLogin");
    let mut session = match tuner::Session::connect(&tuner::ANDROID).await {
        Ok(session) => {
            println!("OK — partner login accepted, clock synced.");
            session
        }
        Err(e) => {
            println!("FAILED: {e}");
            println!("=> The tuner API is down or has changed. Fall back to browser-login.");
            std::process::exit(1);
        }
    };

    let (Ok(username), Ok(password)) = (
        std::env::var("PANDORA_USERNAME"),
        std::env::var("PANDORA_PASSWORD"),
    ) else {
        println!("\nStopping here: no credentials in the environment, so the account-specific");
        println!("steps are skipped. The protocol and our crypto are confirmed working.");
        println!("To run the rest:");
        println!("  $env:PANDORA_USERNAME = 'you@example.com'");
        println!("  $env:PANDORA_PASSWORD = '<your password>'");
        println!("  cargo run --bin probe");
        return;
    };

    heading("2. tuner auth.userLogin (real account)");
    if let Err(e) = session.login(&username, &password).await {
        println!("FAILED: {e}");
        if e.is_bad_credentials() {
            println!("=> Credentials rejected. Check PANDORA_USERNAME / PANDORA_PASSWORD.");
        } else {
            println!("=> Login is walled or broken. Fall back to browser-login.");
        }
        std::process::exit(1);
    }
    let token = session.user_auth_token().unwrap_or_default().to_string();
    println!("OK — logged in. userAuthToken = {}", redact(&token));

    heading("3. account tier");
    match session.call("user.canSubscribe", json!({})).await {
        Ok(v) => println!("canSubscribe: {v}"),
        Err(e) => println!("(couldn't read: {e})"),
    }

    heading("4. tuner user.getStationList");
    let station = match session
        .call("user.getStationList", json!({"includeStationArtUrl": true}))
        .await
    {
        Ok(list) => {
            let stations = list
                .get("stations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            println!("OK — {} stations.", stations.len());
            for s in stations.iter().take(5) {
                println!(
                    "  - {} ({})",
                    s.get("stationName").and_then(Value::as_str).unwrap_or("?"),
                    s.get("stationToken").and_then(Value::as_str).unwrap_or("?")
                );
            }
            stations.first().cloned()
        }
        Err(e) => {
            println!("FAILED: {e}");
            None
        }
    };

    // Pandora allows only one concurrent stream per account. Requesting a tuner playlist and then
    // a REST fragment counts as two, and the second fails with STREAM_VIOLATION — so the probe
    // can exercise one path or the other, never both in a single run.
    let rest_only = std::env::args().any(|a| a == "--rest-only");

    heading("5. tuner station.getPlaylist — audio quality + XOR key check");
    if rest_only {
        println!("SKIPPED (--rest-only) so step 7 doesn't trip STREAM_VIOLATION.");
    } else if let Some(token_value) = station
        .as_ref()
        .and_then(|s| s.get("stationToken"))
        .and_then(Value::as_str)
    {
        let request = json!({
            "stationToken": token_value,
            "includeTrackLength": true,
            "additionalAudioUrl": "HTTP_128_MP3,HTTP_192_MP3,HTTP_64_AACPLUS_ADTS",
        });
        match session.call("station.getPlaylist", request).await {
            Ok(playlist) => {
                let items = playlist
                    .get("items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                println!("OK — {} items.", items.len());
                for item in items.iter().take(3) {
                    let Some(song) = item.get("songName").and_then(Value::as_str) else {
                        println!("  - (non-song item: {:?})", item.get("adToken").is_some());
                        continue;
                    };
                    println!(
                        "  - {song} — {}",
                        item.get("artistName").and_then(Value::as_str).unwrap_or("?")
                    );
                    if let Some(map) = item.get("audioUrlMap") {
                        for (quality, detail) in map.as_object().into_iter().flatten() {
                            println!(
                                "      {quality}: {} kbps {}",
                                detail.get("bitrate").and_then(Value::as_str).unwrap_or("?"),
                                detail.get("encoding").and_then(Value::as_str).unwrap_or("?")
                            );
                        }
                    }
                    // The XOR-obfuscation path only activates when the server supplies a key.
                    println!(
                        "      XOR key present: {}",
                        item.get("key").is_some() || item.get("audioKey").is_some()
                    );
                }
            }
            Err(e) => println!("FAILED: {e}"),
        }
    } else {
        println!("SKIPPED — no station to play.");
    }

    heading("6. THE BIG ONE — tuner token against the web REST API");
    match rest::Client::connect().await {
        Ok(client) => {
            let client = client.with_auth_token(&token);
            match client
                .call("v1/station/getStations", json!({"pageSize": 5}))
                .await
            {
                Ok(result) => {
                    let count = result
                        .get("stations")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(0);
                    println!("*** ACCEPTED *** — REST returned {count} stations.");
                    println!("=> Option A confirmed: fully native, no browser, rich REST surface.");
                }
                Err(e) if e.is_auth_expired() => {
                    println!("REJECTED (1001 invalid auth token) — tokens are not interchangeable.");
                    println!("=> Fall back to option C: one-time browser login to lift a token.");
                }
                Err(e) => {
                    println!("REJECTED: {e}");
                    println!("=> Fall back to option C: one-time browser login to lift a token.");
                }
            }
        }
        Err(e) => println!("Could not reach the REST API at all: {e}"),
    }

    heading("7. audio quality via REST (tuner caps at 64 kbps — does REST do better?)");
    match rest::Client::connect().await {
        Ok(client) => {
            let client = client.with_auth_token(&token);
            let stations = client
                .call("v1/station/getStations", json!({"pageSize": 5}))
                .await
                .unwrap_or(Value::Null);

            match pandora::demo::find_key(&stations, "stationId").and_then(Value::as_str) {
                Some(station_id) => {
                    let fragment = client
                        .call(
                            "v1/playlist/getFragment",
                            json!({
                                "stationId": station_id,
                                "isStationStart": true,
                                "fragmentRequestReason": "Normal",
                                "audioFormat": "aacplus",
                            }),
                        )
                        .await;

                    match fragment {
                        Ok(fragment) => {
                            let tracks = pandora::demo::find_key(&fragment, "tracks")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default();
                            println!("OK — {} items.", tracks.len());
                            for track in tracks.iter().take(3) {
                                let Some(title) =
                                    track.get("songTitle").and_then(Value::as_str)
                                else {
                                    continue;
                                };
                                println!(
                                    "  - {title}: encoding={} bitrate={} host={}",
                                    track
                                        .get("audioEncoding")
                                        .and_then(Value::as_str)
                                        .unwrap_or("?"),
                                    track.get("bitrate").map(|b| b.to_string()).unwrap_or("?".into()),
                                    track
                                        .get("audioURL")
                                        .and_then(Value::as_str)
                                        .and_then(|u| u.split('/').nth(2))
                                        .unwrap_or("?")
                                );
                            }
                            println!(
                                "  XOR `key` present anywhere in fragment: {}",
                                pandora::demo::find_key(&fragment, "key").is_some()
                            );
                        }
                        Err(e) => println!("getFragment FAILED: {e}"),
                    }
                }
                None => println!("No stationId in the REST station list."),
            }
        }
        Err(e) => println!("Could not reach REST: {e}"),
    }

    heading("done");
    println!("Record these results in plans/pandora-native-client.md.");
}
