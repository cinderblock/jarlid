//! Typed models for Pandora's REST responses.
//!
//! Written from observed live responses (`cargo run --example dump-shapes`), not from the public
//! docs, which are 2021-vintage and demonstrably wrong in places.
//!
//! **Every field is `#[serde(default)]` on purpose.** This is an undocumented API we do not
//! control; Pandora can add, rename or drop fields without warning. A missing field should
//! degrade one value, never fail the whole response and take the user's music with it.

use serde::Deserialize;

/// One size of artwork. Pandora returns several per station/track.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Art {
    /// Square edge length in pixels.
    pub size: u32,
    pub url: String,
}

/// Pick the largest artwork available, which is what a full-window hero image wants.
pub fn largest_art(art: &[Art]) -> Option<&Art> {
    art.iter().max_by_key(|a| a.size)
}

/// Pick the smallest artwork at least `min` pixels, falling back to the largest available.
/// Avoids downloading a 1080px image for a list row.
pub fn art_at_least(art: &[Art], min: u32) -> Option<&Art> {
    art.iter()
        .filter(|a| a.size >= min)
        .min_by_key(|a| a.size)
        .or_else(|| largest_art(art))
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Station {
    pub station_id: String,
    /// The `ST:0:…` form, which the newer endpoints prefer over the bare id.
    pub pandora_id: String,
    pub name: String,
    pub art: Vec<Art>,

    /// e.g. "SEEDED_STATION". Thumbprint and shuffle stations behave differently.
    pub station_type: String,
    pub is_shuffle: bool,
    pub is_thumbprint: bool,

    /// Hex RGB (no leading `#`) sampled from the station art — useful for theming the UI to the
    /// current station without us having to analyse the image ourselves.
    pub dominant_color: String,

    pub allow_add_seed: bool,
    pub allow_delete: bool,
    pub allow_rename: bool,
    pub can_shuffle_station: bool,

    /// ISO-8601 timestamps, left as strings: we only ever sort by them.
    pub date_created: String,
    pub last_played: String,
    pub total_play_time: u64,
}

impl Station {
    /// The artwork best suited to a large display.
    pub fn hero_art(&self) -> Option<&Art> {
        largest_art(&self.art)
    }
}

/// What a playlist fragment item actually is. Fragments interleave real music with artist
/// messages and ads, and treating those as songs is a classic way to end up with a player that
/// shows "Now playing: (untitled)".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackKind {
    #[default]
    Track,
    ArtistMessage,
    Advertisement,
    Other,
}

impl TrackKind {
    fn from_str(value: &str) -> Self {
        match value {
            "Track" => Self::Track,
            "ArtistMessage" => Self::ArtistMessage,
            "Advertisement" | "Ad" => Self::Advertisement,
            _ => Self::Other,
        }
    }

    /// Only real music should drive now-playing UI, lyrics lookup and thumbs.
    pub fn is_music(self) -> bool {
        self == Self::Track
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Track {
    pub pandora_id: String,
    /// The tuner API's per-track handle. Feedback endpoints key off this, not `pandora_id`.
    pub track_token: String,
    pub song_title: String,
    pub artist_name: String,
    pub album_title: String,

    /// Signed, expiring URL. Treat as a live credential: never log it in full.
    #[serde(rename = "audioURL")]
    pub audio_url: String,
    /// e.g. "aacplus".
    pub audio_encoding: String,
    /// Seconds.
    pub track_length: u64,

    /// Raw `trackType`; use [`Track::kind`] rather than matching on this directly.
    pub track_type: String,

    pub album_art: Vec<Art>,
    #[serde(rename = "art")]
    pub art_alt: Vec<Art>,

    /// Present only when Pandora XOR-masks the audio. Never seen on anonymous or paid radio; if
    /// this is ever `Some`, the bytes must be un-masked before decoding.
    pub key: Option<String>,

    /// Telemetry endpoints Pandora's own client pings. Not required for playback, but sending
    /// them makes our traffic look like a real client.
    #[serde(rename = "audioReceiptURL")]
    pub audio_receipt_url: String,
    #[serde(rename = "audioSkipUrl")]
    pub audio_skip_url: String,
}

impl Track {
    pub fn kind(&self) -> TrackKind {
        TrackKind::from_str(&self.track_type)
    }

    /// Artwork, from whichever field this response populated.
    pub fn art(&self) -> &[Art] {
        if self.album_art.is_empty() {
            &self.art_alt
        } else {
            &self.album_art
        }
    }

    pub fn hero_art(&self) -> Option<&Art> {
        largest_art(self.art())
    }

    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.track_length)
    }

    /// Safe for logs: identifies the track without leaking the signed audio URL.
    pub fn describe(&self) -> String {
        format!("{} — {}", self.song_title, self.artist_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response missing most fields must still parse. This is the whole point of the
    /// `default` policy — Pandora changing a field name should not break playback.
    #[test]
    fn tolerates_missing_fields() {
        let track: Track = serde_json::from_str(r#"{"songTitle":"x"}"#).unwrap();
        assert_eq!(track.song_title, "x");
        assert_eq!(track.artist_name, "");
        assert_eq!(track.kind(), TrackKind::Other); // absent trackType isn't music
    }

    /// Unknown/new fields must be ignored rather than rejected.
    #[test]
    fn tolerates_unknown_fields() {
        let station: Station =
            serde_json::from_str(r#"{"name":"n","somethingNew":{"a":1}}"#).unwrap();
        assert_eq!(station.name, "n");
    }

    #[test]
    fn classifies_non_music_items() {
        let message = Track {
            track_type: "ArtistMessage".into(),
            ..Default::default()
        };
        assert!(!message.kind().is_music());

        let song = Track {
            track_type: "Track".into(),
            ..Default::default()
        };
        assert!(song.kind().is_music());
    }

    #[test]
    fn picks_appropriate_art() {
        let art = vec![
            Art { size: 90, url: "s".into() },
            Art { size: 640, url: "m".into() },
            Art { size: 1080, url: "l".into() },
        ];
        assert_eq!(largest_art(&art).unwrap().url, "l");
        assert_eq!(art_at_least(&art, 500).unwrap().url, "m");
        // Nothing big enough: fall back to the largest rather than returning nothing.
        assert_eq!(art_at_least(&art, 2000).unwrap().url, "l");
    }

    /// Track art arrives under different keys depending on the endpoint.
    #[test]
    fn falls_back_between_art_fields() {
        let track = Track {
            art_alt: vec![Art { size: 500, url: "alt".into() }],
            ..Default::default()
        };
        assert_eq!(track.hero_art().unwrap().url, "alt");
    }
}
