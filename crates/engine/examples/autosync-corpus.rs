//! Build a test corpus for auto-synced lyrics: real Pandora tracks that LRCLIB already has
//! *synced* lyrics for.
//!
//! Those tracks are free labelled data. The aligner is given only the plain text and the audio,
//! and its line times are scored against the synced copy it was not allowed to see. See
//! `plans/auto-synced-lyrics.md` §4.
//!
//! Each kept track becomes a directory holding `audio.wav` (16 kHz mono, what Whisper-class
//! models take), `synced.lrc`, `plain.txt` and `meta.json`. Tracks whose LRCLIB record is more
//! than two seconds off Pandora's length are skipped: that is a different cut of the song, and
//! its timings would score the aligner against the wrong recording.
//!
//! Anonymous tier, no account. Read-only: it fetches playlists and audio exactly as playback
//! does.
//!
//! Run: cargo run --release -p engine --example autosync-corpus -- <out-dir> [seed-artist …]

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

const USER_AGENT: &str =
    "PandoraDesktop (personal; https://github.com/cinderblock/pandora-desktop)";

/// Spread across genres and vocal styles, because they fail differently: rap is dense, metal is
/// buried in the mix, country and folk are clean.
const SEEDS: &[&str] = &[
    "Taylor Swift",
    "Queen",
    "Adele",
    "Eminem",
    "Johnny Cash",
    "Coldplay",
    "Metallica",
    "Fleetwood Mac",
    "Kendrick Lamar",
    "Dolly Parton",
    "Radiohead",
    "Billie Eilish",
    "The Killers",
    "Amy Winehouse",
    "Linkin Park",
    "Bruno Mars",
];

/// How far LRCLIB's duration may be from Pandora's before it is assumed to be another cut.
const MAX_DURATION_GAP: f64 = 2.0;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(out) = args.next().map(PathBuf::from) else {
        eprintln!("usage: autosync-corpus <out-dir> [seed-artist …]");
        std::process::exit(2);
    };
    let seeds: Vec<String> = args.collect();
    let seeds: Vec<&str> = if seeds.is_empty() {
        SEEDS.to_vec()
    } else {
        seeds.iter().map(String::as_str).collect()
    };
    std::fs::create_dir_all(&out).expect("create corpus dir");

    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .expect("http client");

    let (mut kept, mut skipped) = (0, 0);
    for seed in seeds {
        let fragment = match pandora::demo::anonymous_track(seed).await {
            Ok((_, fragment)) => fragment,
            Err(e) => {
                eprintln!("[{seed}] no playlist: {e}");
                continue;
            }
        };
        let tracks = pandora::demo::find_key(&fragment, "tracks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for raw in tracks {
            let Ok(track) = serde_json::from_value::<pandora::Track>(raw) else {
                continue;
            };
            if track.audio_url.is_empty() || track.track_type == "ArtistMessage" {
                continue;
            }
            let label = format!("{} — {}", track.artist_name, track.song_title);
            let dir = out.join(slug(&format!("{}-{}", track.artist_name, track.song_title)));
            if dir.join("meta.json").exists() {
                println!("[have] {label}");
                continue;
            }

            let Some(record) = lrclib(&http, &track).await else {
                println!("[skip] {label}: no synced lyrics within {MAX_DURATION_GAP}s");
                skipped += 1;
                continue;
            };

            match store(&http, &track, &record, &dir).await {
                Ok(seconds) => {
                    println!("[keep] {label} ({seconds:.0}s, LRCLIB {})", record["id"]);
                    kept += 1;
                }
                Err(e) => {
                    eprintln!("[fail] {label}: {e}");
                    let _ = std::fs::remove_dir_all(&dir);
                }
            }
        }
    }
    println!("\nkept {kept}, skipped {skipped} → {}", out.display());
}

/// The LRCLIB record for `track`, if it has both synced and plain lyrics for this cut.
async fn lrclib(http: &reqwest::Client, track: &pandora::Track) -> Option<Value> {
    let length = track.track_length as f64;
    let usable = |v: &Value| {
        let text = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty())
        };
        let close = v
            .get("duration")
            .and_then(Value::as_f64)
            .is_some_and(|d| (d - length).abs() <= MAX_DURATION_GAP);
        text("syncedLyrics") && text("plainLyrics") && close
    };

    let exact: Option<Value> = get_json(http.get("https://lrclib.net/api/get").query(&[
        ("artist_name", track.artist_name.as_str()),
        ("track_name", track.song_title.as_str()),
        ("album_name", track.album_title.as_str()),
        ("duration", &track.track_length.to_string()),
    ]))
    .await;
    if let Some(v) = exact.filter(usable) {
        return Some(v);
    }

    let found: Option<Value> = get_json(http.get("https://lrclib.net/api/search").query(&[
        ("artist_name", track.artist_name.as_str()),
        ("track_name", track.song_title.as_str()),
    ]))
    .await;
    found?.as_array()?.iter().find(|v| usable(v)).cloned()
}

async fn get_json(request: reqwest::RequestBuilder) -> Option<Value> {
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    serde_json::from_str(&response.text().await.ok()?).ok()
}

/// Download, decode to 16 kHz mono and write the track's directory. Returns its length.
async fn store(
    http: &reqwest::Client,
    track: &pandora::Track,
    record: &Value,
    dir: &Path,
) -> Result<f64, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let bytes = http
        .get(&track.audio_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    // Media Foundation picks its byte-stream handler by extension, and Pandora's aacplus is MP4.
    let compressed = dir.join("source.m4a");
    std::fs::write(&compressed, &bytes).map_err(|e| e.to_string())?;
    let path = compressed.to_string_lossy().to_string();
    let pcm = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let want = audio::Format {
            sample_rate: 16_000,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut decoder = audio::Decoder::open_at(&path, Some(want)).map_err(|e| e.to_string())?;
        let got = decoder.format();
        if got != want {
            return Err(format!("decoder negotiated {got:?}, not 16 kHz mono"));
        }
        decoder.decode_all().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    // Only the decoded copy is needed; the compressed one was just for Media Foundation.
    let _ = std::fs::remove_file(&compressed);

    write_wav(&dir.join("audio.wav"), &pcm, 16_000).map_err(|e| e.to_string())?;

    let text = |k: &str| record.get(k).and_then(Value::as_str).unwrap_or_default();
    std::fs::write(dir.join("synced.lrc"), text("syncedLyrics")).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("plain.txt"), text("plainLyrics")).map_err(|e| e.to_string())?;
    let meta = serde_json::json!({
        "pandoraId": track.pandora_id,
        "title": track.song_title,
        "artist": track.artist_name,
        "album": track.album_title,
        "pandoraLength": track.track_length,
        "lrclibId": record["id"],
        "lrclibDuration": record["duration"],
        "lrclibTitle": record["trackName"],
        "lrclibArtist": record["artistName"],
    });
    std::fs::write(
        dir.join("meta.json"),
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;

    Ok(pcm.len() as f64 / 2.0 / 16_000.0)
}

/// A canonical 44-byte-header PCM WAV, mono 16-bit.
fn write_wav(path: &Path, pcm: &[u8], rate: u32) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let len = pcm.len() as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&1u16.to_le_bytes())?; // mono
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 2).to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?; // block align
    f.write_all(&16u16.to_le_bytes())?; // bits
    f.write_all(b"data")?;
    f.write_all(&len.to_le_bytes())?;
    f.write_all(pcm)?;
    f.flush()
}

/// A filesystem-safe directory name.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(80).collect()
}
