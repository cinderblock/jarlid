//! What the lyric editor's timeline draws besides the lyrics: how loud the song is, moment by
//! moment, and the words the speech engine heard in it. Lines are timed by eye against both — a
//! line starts where the singing does, and a verse ends where the loudness drops away.
//! See `plans/lyrics-timeline.md`.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Manager};

/// Loudness values per second. Fine enough to place a word by eye at full zoom.
pub const RATE: u32 = 100;

/// The quietest level drawn, in dB below full scale. Anything quieter is silence on screen.
const FLOOR_DB: f64 = 60.0;

/// The last envelope worked out, so reopening the editor on the same song is instant.
#[derive(Default)]
pub struct TimelineCtl {
    last: Mutex<Option<(String, Arc<Envelope>)>>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    /// The track it belongs to, so the editor can tell it is still the one it is editing.
    pub title: String,
    pub artist: String,
    /// Values per second.
    pub rate: u32,
    /// Loudness, 0 = the floor or below, 255 = full scale, on a decibel scale.
    pub levels: Vec<u8>,
}

/// The loudness of the song playing now. Decoding a whole song takes a second or two, from
/// the song cache when it is there and from its stream otherwise.
#[tauri::command]
pub async fn track_envelope(app: AppHandle) -> Result<Arc<Envelope>, String> {
    let engine = app
        .state::<crate::native::NativeEngine>()
        .try_engine()
        .ok_or("Nothing is playing")?;
    let track = engine.now_playing().await.ok_or("Nothing is playing")?;
    let ctl = app.state::<TimelineCtl>();
    if let Some((id, env)) = ctl.last.lock().unwrap().as_ref() {
        if *id == track.pandora_id {
            return Ok(env.clone());
        }
    }
    let src = engine
        .cached_file(&track.pandora_id)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| track.audio_url.clone());
    let levels = tokio::task::spawn_blocking(move || {
        crate::autosync::decode_16k(&src).map(|pcm| envelope(&pcm, 16_000))
    })
    .await
    .map_err(|e| e.to_string())??;
    let env = Arc::new(Envelope {
        title: track.song_title.clone(),
        artist: track.artist_name.clone(),
        rate: RATE,
        levels,
    });
    *ctl.last.lock().unwrap() = Some((track.pandora_id.clone(), env.clone()));
    Ok(env)
}

/// One word the speech engine heard.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HeardWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// Its confidence, 0–1; 1 where it wasn't kept.
    pub p: f32,
}

/// What the speech engine heard in a track, or nothing if it hasn't heard it.
#[tauri::command]
pub fn heard_words(
    app: AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
) -> Vec<HeardWord> {
    let key = crate::lyrics::key_for(&artist, &track, album.as_deref());
    crate::autosync::heard(&app, &key)
        .unwrap_or_default()
        .into_iter()
        .map(|(text, start, end, p)| HeardWord {
            text,
            start,
            end,
            p,
        })
        .collect()
}

/// The RMS loudness of 16-bit little-endian mono `pcm`, [`RATE`] values per second, each mapped
/// from `-FLOOR_DB`..0 dBFS onto 0..255.
pub fn envelope(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let per = (sample_rate / RATE).max(1) as usize;
    pcm.as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b) as f64 / 32768.0)
        .collect::<Vec<_>>()
        .chunks(per)
        .map(|w| {
            let rms = (w.iter().map(|s| s * s).sum::<f64>() / w.len() as f64).sqrt();
            let db = 20.0 * rms.max(1e-9).log10();
            (((db + FLOOR_DB) / FLOOR_DB).clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(samples: impl IntoIterator<Item = i16>) -> Vec<u8> {
        samples.into_iter().flat_map(i16::to_le_bytes).collect()
    }

    #[test]
    fn silence_is_the_floor_and_full_scale_is_the_top() {
        let quiet = pcm(std::iter::repeat_n(0, 160));
        assert_eq!(envelope(&quiet, 16_000), [0]);
        // A full-scale square wave has an RMS of full scale.
        let loud = pcm((0..160).map(|i| if i % 2 == 0 { i16::MAX } else { -i16::MAX }));
        assert_eq!(envelope(&loud, 16_000), [255]);
    }

    /// A real song through Media Foundation: `JARLID_AUTOSYNC_TEST_AUDIO=<file.m4a> cargo test
    /// -- --ignored real_song`. Its quiet intro and loud chorus should both show.
    #[test]
    #[ignore = "needs an audio file; see the doc comment"]
    fn a_real_song_has_quiet_and_loud_parts() {
        let src =
            std::env::var("JARLID_AUTOSYNC_TEST_AUDIO").expect("set JARLID_AUTOSYNC_TEST_AUDIO");
        let levels = envelope(&crate::autosync::decode_16k(&src).unwrap(), 16_000);
        let mut sorted = levels.clone();
        sorted.sort();
        let (p10, p90) = (sorted[sorted.len() / 10], sorted[sorted.len() * 9 / 10]);
        eprintln!("{} values, p10 {p10}, p90 {p90}", levels.len());
        assert!(levels.len() > 60 * RATE as usize);
        assert!(p90 > 150 && p90 - p10 > 30, "p10 {p10}, p90 {p90}");
    }

    #[test]
    fn one_value_per_hundredth_of_a_second() {
        let second = pcm(std::iter::repeat_n(1000, 16_000));
        let levels = envelope(&second, 16_000);
        assert_eq!(levels.len(), RATE as usize);
        // 1000/32768 is about -30 dBFS: halfway up a 60 dB scale.
        assert!((levels[0] as i32 - 128).abs() <= 2, "{}", levels[0]);
    }
}
