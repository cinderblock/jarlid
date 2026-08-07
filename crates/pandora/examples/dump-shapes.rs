//! Print the *shape* of Pandora's REST responses — key names and value types — so the typed
//! models in `models.rs` can be written from observation rather than from 2021-vintage docs.
//!
//! Deliberately prints structure, not content: no station names, no track titles, no tokens.
//! Only short enum-like strings (under 24 chars, no spaces) are shown as samples, because field
//! names alone don't reveal that e.g. `trackType` is "Track" | "ArtistMessage".
//!
//! Run: cargo run --example dump-shapes

use std::collections::BTreeMap;

use pandora::{demo::find_key, rest, tuner};
use serde_json::{json, Value};

/// Describe a value by its type, so we learn the schema without printing the data.
fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(n) if n.is_i64() || n.is_u64() => "int".into(),
        Value::Number(_) => "float".into(),
        Value::String(s) => {
            // Short, space-free strings are almost always enum tags or IDs worth knowing.
            if s.len() < 24 && !s.contains(' ') && !s.starts_with("http") {
                format!("string ({s:?})")
            } else {
                format!("string [{} chars]", s.len())
            }
        }
        Value::Array(items) => match items.first() {
            Some(first) => format!("array[{}] of {}", items.len(), describe(first)),
            None => "array[0]".into(),
        },
        Value::Object(map) => format!("object {{{}}}", map.len()),
    }
}

/// Print one level of an object's keys with their types.
fn print_shape(label: &str, value: &Value) {
    println!("\n--- {label} ---");
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map.iter().collect();
            for (key, child) in sorted {
                println!("  {key}: {}", describe(child));
            }
        }
        other => println!("  (not an object: {})", describe(other)),
    }
}

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD (see .env.example at the repo root).");
        std::process::exit(2);
    };

    let mut session = tuner::Session::connect(&tuner::ANDROID)
        .await
        .expect("partner login");
    session.login(&username, &password).await.expect("user login");
    let token = session.user_auth_token().expect("token").to_string();

    let client = rest::Client::connect()
        .await
        .expect("rest")
        .with_auth_token(&token);

    // getStations is safe to call regardless of what else is streaming — only getFragment is
    // gated by the one-stream-per-account rule.
    let stations = client
        .call("v1/station/getStations", json!({"pageSize": 3}))
        .await
        .expect("getStations");
    print_shape("getStations (top level)", &stations);

    if let Some(first) = find_key(&stations, "stations")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        print_shape("a Station", first);
        if let Some(art) = first.get("art").and_then(Value::as_array).and_then(|a| a.first()) {
            print_shape("a Station's art entry", art);
        }
    }

    let profile = client.call("v1/listener/getProfile", json!({})).await;
    match profile {
        Ok(profile) => print_shape("listener/getProfile", &profile),
        Err(e) => println!("\n--- listener/getProfile --- unavailable: {e}"),
    }

    // Fragments are the important shape, but need the account's stream to be free.
    let station_id = find_key(&stations, "stationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

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
            print_shape("getFragment (top level)", &fragment);
            if let Some(track) = find_key(&fragment, "tracks")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
            {
                print_shape("a Track", track);
                println!("\n>>> AUDIO QUALITY <<<");
                for key in ["audioEncoding", "bitrate", "audioURL"] {
                    if let Some(value) = track.get(key) {
                        let shown = if key == "audioURL" {
                            json!(value.as_str().unwrap_or("").split('/').nth(2).unwrap_or("?"))
                        } else {
                            value.clone()
                        };
                        println!("  {key}: {shown}");
                    }
                }
            }
        }
        Err(e) => {
            println!("\n--- getFragment --- unavailable: {e}");
            println!("    (STREAM_VIOLATION means another client holds the account's one stream —");
            println!("     quit Jarlid and re-run to capture the Track shape and audio bitrate.)");
        }
    }
}
