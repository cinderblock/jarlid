//! The client the app actually uses.
//!
//! Which API serves which purpose is not a style choice — each was settled by measurement
//! (see `plans/pandora-native-client.md`):
//!
//! | Purpose        | API    | Why |
//! |----------------|--------|-----|
//! | Login          | tuner  | No PerimeterX wall. REST `auth/login` returns 403 to any non-browser. |
//! | Station list   | REST   | Richer: 1080px art and `dominantColor`, which the tuner list lacks. |
//! | **Audio**      | tuner  | REST playback is *refused* on a tuner token (`STREAM_VIOLATION`), and tuner `additionalAudioUrl` yields 128 kbps MP3 — double the 64 kbps default. |
//! | Feedback       | tuner  | Verified end to end against a throwaway station. |
//!
//! Pandora permits exactly **one concurrent stream per account**; see [`Error::is_stream_violation`].

use serde_json::{json, Value};

use crate::models::{Station, Track};
use crate::{demo::find_key, rest, tuner, Error, Result};

/// Stream spec for the best audio this account can get: 128 kbps MP3, measured.
/// `HTTP_192_MP3` is advertised by Pandora but is **not** served to this subscription.
pub const BEST_AUDIO: &str = "HTTP_128_MP3";

/// Fallback if the preferred stream is ever withdrawn: 64 kbps HE-AAC, always present.
pub const FALLBACK_AUDIO: &str = "HTTP_64_AACPLUS_ADTS";

pub struct Client {
    tuner: tuner::Session,
    rest: rest::Client,
    /// Kept so an expired token can be renewed without bouncing the user to a sign-in screen
    /// mid-song. Pandora's tokens do expire.
    credentials: (String, String),
}

impl Client {
    pub async fn login(username: &str, password: &str) -> Result<Self> {
        let (tuner, rest) = Self::authenticate(username, password).await?;
        Ok(Self {
            tuner,
            rest,
            credentials: (username.to_string(), password.to_string()),
        })
    }

    async fn authenticate(
        username: &str,
        password: &str,
    ) -> Result<(tuner::Session, rest::Client)> {
        let mut session = tuner::Session::connect(&tuner::ANDROID).await?;
        session.login(username, password).await?;

        let token = session
            .user_auth_token()
            .ok_or_else(|| Error::Protocol("login succeeded but returned no token".into()))?
            .to_string();

        // The tuner token doubles as the REST `X-AuthToken` — verified 2026-08-07. That is what
        // lets us read the richer REST station list without a browser anywhere.
        let rest = rest::Client::connect().await?.with_auth_token(token);
        Ok((session, rest))
    }

    pub async fn refresh_auth(&mut self) -> Result<()> {
        let (username, password) = self.credentials.clone();
        let (tuner, rest) = Self::authenticate(&username, &password).await?;
        self.tuner = tuner;
        self.rest = rest;
        Ok(())
    }

    /// REST call, transparently re-authenticating once if the token has expired.
    pub async fn rest_call(&mut self, endpoint: &str, body: Value) -> Result<Value> {
        match self.rest.call(endpoint, body.clone()).await {
            Err(e) if e.is_auth_expired() => {
                self.refresh_auth().await?;
                self.rest.call(endpoint, body).await
            }
            other => other,
        }
    }

    /// Tuner call, with the same re-authentication behaviour.
    pub async fn tuner_call(&mut self, method: &str, body: Value) -> Result<Value> {
        match self.tuner.call(method, body.clone()).await {
            Err(e) if e.is_auth_expired() => {
                self.refresh_auth().await?;
                self.tuner.call(method, body).await
            }
            other => other,
        }
    }

    /// The full station collection, from REST — it carries 1080px art and `dominantColor`, which
    /// the tuner station list does not.
    pub async fn stations(&mut self) -> Result<Vec<Station>> {
        let mut all = Vec::new();
        let mut index = 0u64;

        loop {
            let page = self
                .rest_call(
                    "v1/station/getStations",
                    json!({"pageSize": 250, "startIndex": index}),
                )
                .await?;

            let stations = find_key(&page, "stations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if stations.is_empty() {
                break;
            }

            index += stations.len() as u64;
            let total = find_key(&page, "totalStations")
                .and_then(Value::as_u64)
                .unwrap_or(0);

            // Skip malformed entries rather than failing the whole collection.
            all.extend(
                stations
                    .into_iter()
                    .filter_map(|s| serde_json::from_value::<Station>(s).ok()),
            );

            if index >= total {
                break;
            }
        }

        Ok(all)
    }

    /// Next batch of tracks, **over the tuner API**.
    ///
    /// REST `playlist/getFragment` is refused on a tuner token, and the tuner path yields better
    /// audio anyway. `stationToken` here is the tuner-side token, not the REST `stationId`.
    ///
    /// Fails with [`Error::is_stream_violation`] when another client holds the account's stream.
    pub async fn playlist(&mut self, station_token: &str) -> Result<Vec<Track>> {
        let playlist = self
            .tuner_call(
                "station.getPlaylist",
                json!({
                    "stationToken": station_token,
                    "includeTrackLength": true,
                    "additionalAudioUrl": BEST_AUDIO,
                }),
            )
            .await?;

        let items = playlist
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        Ok(items
            .into_iter()
            .filter_map(|item| {
                let mut track: Track = serde_json::from_value(item.clone()).ok()?;

                // The tuner API names things differently from REST; normalise onto one model so
                // callers never have to care which API a track came from.
                track.song_title = string_at(&item, "songName");
                track.artist_name = string_at(&item, "artistName");
                track.album_title = string_at(&item, "albumName");
                track.track_token = string_at(&item, "trackToken");

                // Prefer the 128 kbps stream; fall back to the standard 64 kbps aacplus.
                track.audio_url = additional_audio_url(&item)
                    .unwrap_or_else(|| audio_url_map_best(&item).unwrap_or_default());
                track.audio_encoding = if additional_audio_url(&item).is_some() {
                    "mp3".into()
                } else {
                    "aacplus".into()
                };

                // The tuner API returns a single `albumArtUrl` string rather than REST's array of
                // sizes, so synthesise the array — otherwise every tuner-sourced track renders
                // with no artwork.
                if track.art().is_empty() {
                    let url = string_at(&item, "albumArtUrl");
                    if !url.is_empty() {
                        track.album_art = art_sizes_from_url(&url);
                    }
                }

                // Ads and artist messages have no songName; drop them here so callers only ever
                // see real music.
                (!track.song_title.is_empty()).then_some(track)
            })
            .collect())
    }

    /// The tuner station list — needed because [`Self::playlist`] takes a `stationToken`, which
    /// the REST list does not provide.
    pub async fn tuner_stations(&mut self) -> Result<Vec<(String, String)>> {
        let list = self.tuner_call("user.getStationList", json!({})).await?;
        Ok(list
            .get("stations")
            .and_then(Value::as_array)
            .map(|stations| {
                stations
                    .iter()
                    .map(|s| (string_at(s, "stationName"), string_at(s, "stationToken")))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn search(&mut self, query: &str) -> Result<Value> {
        self.tuner_call("music.search", json!({"searchText": query})).await
    }

    /// Everything a station knows about the listener's taste: its seeds and every thumb.
    ///
    /// `includeExtendedAttributes` is what makes this worth having — without it the response is
    /// just the station header. With it, `music.songs`/`music.artists` (the seeds) and
    /// `feedback.thumbsUp`/`feedback.thumbsDown` arrive in **one** call, already carrying display
    /// names. The REST API has no equivalent: there, seeds and each thumb polarity are separate
    /// paginated endpoints, and seeds come back as bare ids needing a second lookup to name them.
    /// Six-ish requests per station there, one here — which matters when walking a collection.
    ///
    /// Note a QuickMix station returns neither `music` nor `feedback`: it is a shuffle *over*
    /// other stations and has no seeds or thumbs of its own.
    pub async fn station_details(&mut self, station_token: &str) -> Result<Value> {
        let body = json!({"stationToken": station_token, "includeExtendedAttributes": true});
        self.tuner_call("station.getStation", body).await
    }

    // ---------------------------------------------------------------------------------------
    // Write paths — all VERIFIED 2026-08-07 against a throwaway station that was then deleted.
    // ---------------------------------------------------------------------------------------

    pub async fn thumb_up(&mut self, station_token: &str, track_token: &str) -> Result<Value> {
        self.feedback(station_token, track_token, true).await
    }

    pub async fn thumb_down(&mut self, station_token: &str, track_token: &str) -> Result<Value> {
        self.feedback(station_token, track_token, false).await
    }

    async fn feedback(
        &mut self,
        station_token: &str,
        track_token: &str,
        is_positive: bool,
    ) -> Result<Value> {
        self.tuner_call(
            "station.addFeedback",
            json!({
                "stationToken": station_token,
                "trackToken": track_token,
                "isPositive": is_positive,
            }),
        )
        .await
    }

    /// Undo a thumb. `feedback_id` comes from the `feedbackId` in [`Self::thumb_up`]'s response.
    pub async fn remove_feedback(&mut self, feedback_id: &str) -> Result<Value> {
        self.tuner_call("station.deleteFeedback", json!({"feedbackId": feedback_id}))
            .await
    }

    /// Rest a track for ~30 days.
    pub async fn tired_of_track(&mut self, track_token: &str) -> Result<Value> {
        self.tuner_call("user.sleepSong", json!({"trackToken": track_token}))
            .await
    }

    pub async fn create_station(&mut self, music_token: &str) -> Result<Value> {
        self.tuner_call("station.createStation", json!({"musicToken": music_token}))
            .await
    }

    pub async fn rename_station(&mut self, station_token: &str, name: &str) -> Result<Value> {
        self.tuner_call(
            "station.renameStation",
            json!({"stationToken": station_token, "stationName": name}),
        )
        .await
    }

    pub async fn delete_station(&mut self, station_token: &str) -> Result<Value> {
        self.tuner_call("station.deleteStation", json!({"stationToken": station_token}))
            .await
    }
}

fn string_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `additionalAudioUrl` we asked for. Pandora returns a bare string for a single spec, or an
/// array — and **drops** unavailable specs rather than returning empty slots, which is why we only
/// ever request one at a time.
fn additional_audio_url(item: &Value) -> Option<String> {
    match item.get("additionalAudioUrl")? {
        Value::String(url) if !url.is_empty() => Some(url.clone()),
        Value::Array(urls) => urls
            .first()
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// Turn the tuner API's single `albumArtUrl` into the size-tagged list the models expect.
///
/// The URL encodes its dimensions (`…/500W_500H.jpg`), and Pandora's CDN serves other sizes from
/// the same path — including 1080px, which is what a full-window hero image wants. Substituting
/// the dimensions is how we reach the larger art without a second API call.
fn art_sizes_from_url(url: &str) -> Vec<crate::models::Art> {
    let Some((prefix, suffix)) = split_at_dimensions(url) else {
        // Unrecognised shape: keep the original rather than inventing URLs that may 404.
        return vec![crate::models::Art { size: 500, url: url.to_string() }];
    };

    [130u32, 500, 640, 1080]
        .into_iter()
        .map(|size| crate::models::Art {
            size,
            url: format!("{prefix}{size}W_{size}H{suffix}"),
        })
        .collect()
}

/// Split `…/_500W_500H.jpg` around its dimension segment.
fn split_at_dimensions(url: &str) -> Option<(&str, &str)> {
    let start = url.rfind('/')? + 1;
    let (head, tail) = url.split_at(start);
    let marker = tail.find("W_")?;
    let width_start = tail[..marker].rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    // Everything after the "…H" of "500W_500H".
    let height_end = tail[marker..].find('H')? + marker + 1;
    Some((&url[..head.len() + width_start], &tail[height_end..]))
}

/// Highest-bitrate entry of the standard `audioUrlMap`.
fn audio_url_map_best(item: &Value) -> Option<String> {
    let map = item.get("audioUrlMap")?.as_object()?;
    map.values()
        .max_by_key(|detail| {
            detail
                .get("bitrate")
                .and_then(Value::as_str)
                .and_then(|b| b.parse::<u32>().ok())
                .unwrap_or(0)
        })?
        .get("audioUrl")
        .and_then(Value::as_str)
        .map(str::to_string)
}

impl Error {
    /// Another client holds this account's single permitted stream.
    ///
    /// Worth distinguishing: the honest message is "Pandora is playing on another device", which
    /// is actionable, rather than a generic failure.
    pub fn is_stream_violation(&self) -> bool {
        matches!(self, Error::Api { message, .. } if message.contains("STREAM_VIOLATION"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_stream_violation() {
        let error = Error::Api { code: 0, message: "STREAM_VIOLATION".into() };
        assert!(error.is_stream_violation());
        assert!(!error.is_auth_expired());
    }

    #[test]
    fn recognises_expired_token() {
        let error = Error::Api { code: 1001, message: "invalid auth token".into() };
        assert!(error.is_auth_expired());
        assert!(!error.is_stream_violation());
    }

    #[test]
    fn reads_additional_audio_url_in_both_shapes() {
        assert_eq!(
            additional_audio_url(&json!({"additionalAudioUrl": "http://a"})),
            Some("http://a".into())
        );
        assert_eq!(
            additional_audio_url(&json!({"additionalAudioUrl": ["http://b"]})),
            Some("http://b".into())
        );
        // Absent or empty must fall through to the audioUrlMap rather than yielding "".
        assert_eq!(additional_audio_url(&json!({"additionalAudioUrl": ""})), None);
        assert_eq!(additional_audio_url(&json!({})), None);
    }

    #[test]
    fn expands_tuner_album_art_to_sizes() {
        let art = art_sizes_from_url("https://cont-1.p-cdn.us/images/ab/cd/_500W_500H.jpg");
        assert_eq!(art.len(), 4);
        let largest = crate::models::largest_art(&art).unwrap();
        assert_eq!(largest.size, 1080);
        assert!(
            largest.url.ends_with("_1080W_1080H.jpg"),
            "got {}",
            largest.url
        );
        // The directory part must survive untouched.
        assert!(largest.url.starts_with("https://cont-1.p-cdn.us/images/ab/cd/_"));
    }

    /// An unfamiliar URL shape must degrade to the original rather than fabricate 404s.
    #[test]
    fn keeps_unrecognised_art_url() {
        let art = art_sizes_from_url("https://example.com/cover.jpg");
        assert_eq!(art.len(), 1);
        assert_eq!(art[0].url, "https://example.com/cover.jpg");
    }

    #[test]
    fn picks_highest_bitrate_from_map() {
        let item = json!({"audioUrlMap": {
            "lowQuality":    {"bitrate": "32", "audioUrl": "low"},
            "highQuality":   {"bitrate": "64", "audioUrl": "high"},
            "mediumQuality": {"bitrate": "64", "audioUrl": "medium"},
        }});
        assert!(matches!(audio_url_map_best(&item).as_deref(), Some("high" | "medium")));
    }
}
