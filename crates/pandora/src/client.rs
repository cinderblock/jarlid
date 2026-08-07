//! The client the app actually uses.
//!
//! Combines the two APIs the way the protocol research concluded is correct:
//!
//! - **Login over the tuner API.** It has no bot wall, and its `userAuthToken` is accepted by the
//!   REST API (verified 2026-08-07).
//! - **Everything else over REST.** The tuner API caps audio at 64 kbps even for a paid
//!   subscriber, so playing its URLs would sound worse than the web player.

use serde_json::{json, Value};

use crate::models::{Station, Track};
use crate::{demo::find_key, rest, tuner, Error, Result};

pub struct Client {
    rest: rest::Client,
    /// Kept so an expired token can be renewed without bothering the user. Pandora tokens do
    /// expire, and a silent re-login is far better than dumping the user back at a sign-in screen
    /// mid-song.
    credentials: (String, String),
}

impl Client {
    /// Log in and prepare a REST client. This is the only call that touches the tuner API.
    pub async fn login(username: &str, password: &str) -> Result<Self> {
        let token = Self::fetch_token(username, password).await?;
        let rest = rest::Client::connect().await?.with_auth_token(token);
        Ok(Self {
            rest,
            credentials: (username.to_string(), password.to_string()),
        })
    }

    async fn fetch_token(username: &str, password: &str) -> Result<String> {
        let mut session = tuner::Session::connect(&tuner::ANDROID).await?;
        session.login(username, password).await?;
        session
            .user_auth_token()
            .map(str::to_string)
            .ok_or_else(|| Error::Protocol("login succeeded but returned no token".into()))
    }

    /// Re-authenticate in place after the token expires.
    pub async fn refresh_auth(&mut self) -> Result<()> {
        let (username, password) = self.credentials.clone();
        let token = Self::fetch_token(&username, &password).await?;
        self.rest = rest::Client::connect().await?.with_auth_token(token);
        Ok(())
    }

    /// Call an endpoint, transparently re-logging in once if the token has expired.
    pub async fn call(&mut self, endpoint: &str, body: Value) -> Result<Value> {
        match self.rest.call(endpoint, body.clone()).await {
            Err(e) if e.is_auth_expired() => {
                self.refresh_auth().await?;
                self.rest.call(endpoint, body).await
            }
            other => other,
        }
    }

    /// Every station in the collection. Pandora pages this; 250 covers all but extreme accounts,
    /// and `totalStations` in the response tells us if we need to go again.
    pub async fn stations(&mut self) -> Result<Vec<Station>> {
        let mut all = Vec::new();
        let mut index = 0u64;

        loop {
            let page = self
                .call(
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

            for station in stations {
                // Skip malformed entries rather than failing the whole list.
                if let Ok(station) = serde_json::from_value::<Station>(station) {
                    all.push(station);
                }
            }

            if index >= total {
                break;
            }
        }

        Ok(all)
    }

    /// Next batch of tracks for a station.
    ///
    /// Fails with [`Error::is_stream_violation`] when another client is streaming this account —
    /// Pandora permits exactly one concurrent stream.
    pub async fn fragment(&mut self, station_id: &str, is_start: bool) -> Result<Vec<Track>> {
        let fragment = self
            .call(
                "v1/playlist/getFragment",
                json!({
                    "stationId": station_id,
                    "isStationStart": is_start,
                    "fragmentRequestReason": if is_start { "Normal" } else { "ContinueStation" },
                    "audioFormat": "aacplus",
                }),
            )
            .await?;

        let tracks = find_key(&fragment, "tracks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        Ok(tracks
            .into_iter()
            .filter_map(|t| serde_json::from_value::<Track>(t).ok())
            .collect())
    }

    pub async fn search(&mut self, query: &str) -> Result<Value> {
        // NB: the `types` filter is ignored server-side — callers must filter results themselves.
        self.call(
            "v1/search/fullSearch",
            json!({"query": query, "count": 20}),
        )
        .await
    }

    // ---------------------------------------------------------------------------------------
    // Write paths.
    //
    // ⚠️ UNVERIFIED. These mutate the user's real account (thumbs shape their stations forever),
    // so they have deliberately NOT been executed against a live account. The endpoint names come
    // from the shipping web bundle; the request bodies are inferred. Verify each one against a
    // throwaway station before trusting it, and expect the field names to need correcting.
    // ---------------------------------------------------------------------------------------

    /// Thumbs up the current track. **Unverified — see the warning above.**
    pub async fn thumb_up(&mut self, station_id: &str, track_pandora_id: &str) -> Result<Value> {
        self.feedback(station_id, track_pandora_id, true).await
    }

    /// Thumbs down the current track. **Unverified — see the warning above.**
    pub async fn thumb_down(&mut self, station_id: &str, track_pandora_id: &str) -> Result<Value> {
        self.feedback(station_id, track_pandora_id, false).await
    }

    async fn feedback(
        &mut self,
        station_id: &str,
        track_pandora_id: &str,
        is_positive: bool,
    ) -> Result<Value> {
        self.call(
            "v1/station/addFeedback",
            json!({
                "stationId": station_id,
                "trackToken": track_pandora_id,
                "isPositive": is_positive,
            }),
        )
        .await
    }

    /// Tell Pandora we're tired of a track (rests it ~30 days). **Unverified.**
    pub async fn tired_of_track(&mut self, track_pandora_id: &str) -> Result<Value> {
        self.call("v3/station/addTiredSong", json!({"trackToken": track_pandora_id}))
            .await
    }

    /// Report that playback started. Optional telemetry — Pandora does not require it, but
    /// sending it makes our traffic resemble a real client. **Unverified.**
    pub async fn report_track_started(&mut self, track_pandora_id: &str) -> Result<Value> {
        self.call("v1/station/trackStarted", json!({"trackToken": track_pandora_id}))
            .await
    }
}

impl Error {
    /// Another client holds this account's single permitted stream.
    ///
    /// Worth surfacing specifically: the honest message is "Pandora is playing on another device",
    /// which is actionable, rather than a generic failure.
    pub fn is_stream_violation(&self) -> bool {
        matches!(self, Error::Api { message, .. } if message.contains("STREAM_VIOLATION"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_stream_violation() {
        let error = Error::Api {
            code: 0,
            message: "STREAM_VIOLATION".into(),
        };
        assert!(error.is_stream_violation());
        assert!(!error.is_auth_expired());
    }

    #[test]
    fn recognises_expired_token() {
        let error = Error::Api {
            code: 1001,
            message: "invalid auth token".into(),
        };
        assert!(error.is_auth_expired());
        assert!(!error.is_stream_violation());
    }
}
