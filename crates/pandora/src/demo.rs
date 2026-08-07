//! Spike support: obtain a real, playable Pandora track without any account.
//!
//! Deliberately uses the **anonymous** listener tier. It serves the same `aacplus` encoding as a
//! logged-in session, so it is a faithful sample for exercising the audio pipeline, while never
//! touching the user's paid subscription.
//!
//! This module exists for probes and tests. The real client does not use it.

use serde_json::{json, Value};

use crate::{rest, Result};

/// Load `.env` / `.env.local` into the environment, walking up from the working directory.
///
/// Exists because `wenv` exports into *its* shell session, which a separately-launched process
/// does not inherit. Nearest file wins, but empty values never override a real one, so a stale
/// half-filled `.env` in a subdirectory can't shadow the real one at the repo root.
///
/// Probe-only. The real client will use the OS credential store.
pub fn load_dotenv() {
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

/// Credentials from the environment, after [`load_dotenv`].
pub fn credentials() -> Option<(String, String)> {
    load_dotenv();
    let username = std::env::var("PANDORA_USERNAME").ok().filter(|v| !v.is_empty())?;
    let password = std::env::var("PANDORA_PASSWORD").ok().filter(|v| !v.is_empty())?;
    Some((username, password))
}

#[derive(Debug, Clone)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub audio_url: String,
    pub encoding: String,
    pub length_seconds: u64,
    /// Present when Pandora XOR-masks the audio. Dormant on this tier; unverified on paid tiers.
    pub xor_key: Option<String>,
}

/// Pull the first value found under `key`, at any depth. Pandora nests responses inconsistently
/// across endpoints, and we care about the value rather than the path.
pub fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .or_else(|| map.values().find_map(|v| find_key(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}

/// Anonymous login → search → create a station → fetch a playlist fragment → first real track.
pub async fn anonymous_track(seed_query: &str) -> Result<(Track, Value)> {
    let mut client = rest::Client::connect().await?;
    client.anonymous_login().await?;

    let search = client
        .call(
            "v1/search/fullSearch",
            json!({"query": seed_query, "types": ["AR"], "count": 5}),
        )
        .await?;

    // The server ignores the `types` filter, so pick the artist ourselves — otherwise a composer
    // or genre sorts first and will not seed a station.
    let items = find_key(&search, "items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let artist = items
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("artist"))
        .ok_or_else(|| crate::Error::Protocol(format!("no artist found for {seed_query:?}")))?;

    let pandora_id = artist
        .get("pandoraId")
        .and_then(Value::as_str)
        .ok_or_else(|| crate::Error::Protocol("artist has no pandoraId".into()))?;

    // NB: createStation wants `pandoraId` (e.g. AR:105740). The publicly documented `stationCode`
    // field is rejected with GENERIC — those docs are 2021-vintage and have drifted.
    let station = client
        .call(
            "v1/station/createStation",
            json!({"pandoraId": pandora_id, "stationName": ""}),
        )
        .await?;

    let station_id = find_key(&station, "stationId")
        .and_then(Value::as_str)
        .ok_or_else(|| crate::Error::Protocol("createStation returned no stationId".into()))?;

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
        .await?;

    let tracks = find_key(&fragment, "tracks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Fragments interleave non-song items (ArtistMessage, ads); take the first real track.
    let track = tracks
        .iter()
        .find(|t| {
            t.get("audioURL").and_then(Value::as_str).is_some()
                && t.get("trackType").and_then(Value::as_str) != Some("ArtistMessage")
        })
        .ok_or_else(|| crate::Error::Protocol("fragment contained no playable track".into()))?;

    let field = |name: &str| {
        track
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    Ok((
        Track {
            title: field("songTitle"),
            artist: field("artistName"),
            audio_url: field("audioURL"),
            encoding: field("audioEncoding"),
            length_seconds: track
                .get("trackLength")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            xor_key: find_key(track, "key")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        fragment,
    ))
}
