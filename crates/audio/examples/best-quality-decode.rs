//! Full path, real account: login → station → 128 kbps MP3 → decoded PCM.
//!
//! Verifies the pipeline the app will actually use, at the best quality this subscription can get.
//! Media Foundation handles MP3 as readily as HE-AAC, so switching the preferred stream costs us
//! nothing on the decode side.
//!
//! Run: cargo run --example best-quality-decode

use std::time::Instant;

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD.");
        std::process::exit(2);
    };

    let mut client = pandora::Client::login(&username, &password)
        .await
        .expect("login");

    let stations = client.station_list().await.expect("stations");
    let station = stations.first().expect("a station");
    let (name, token) = (station.station_name.clone(), station.station_token.clone());
    println!("station: {name}");

    let tracks = match client.playlist(&token).await {
        Ok(tracks) => tracks,
        Err(e) if e.is_stream_violation() => {
            eprintln!("Pandora is playing on another device — close it and retry.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("playlist failed: {e}");
            std::process::exit(1);
        }
    };

    let track = tracks.first().expect("a track");
    println!("track:    {}", track.describe());
    println!("album:    {}", track.album_title);
    println!("encoding: {}", track.audio_encoding);
    println!("length:   {} s", track.track_length);
    // The tuner API gives one art URL; we synthesise the other sizes by rewriting its dimensions.
    // That is an assumption about Pandora's CDN, so verify it actually resolves.
    match track.hero_art() {
        Some(art) => {
            let status = reqwest::Client::new()
                .get(&art.url)
                .header("Range", "bytes=0-0")
                .send()
                .await
                .map(|r| r.status().as_u16())
                .unwrap_or(0);
            println!(
                "art:      {}px — {} ({})",
                art.size,
                if (200..300).contains(&status) { "resolves" } else { "BROKEN" },
                status
            );
        }
        None => println!("art:      none"),
    }
    println!("host:     {}", track.audio_url.split('/').nth(2).unwrap_or("?"));

    println!("\ndecoding …");
    let started = Instant::now();
    let mut decoder = match audio::Decoder::open(&track.audio_url) {
        Ok(decoder) => decoder,
        Err(e) => {
            eprintln!("open failed: {e}");
            std::process::exit(1);
        }
    };
    let opened = started.elapsed();
    let format = decoder.format();
    println!(
        "opened in {opened:?} — {} Hz, {} ch, {}-bit",
        format.sample_rate, format.channels, format.bits_per_sample
    );

    // Decode ~5 seconds: enough to prove sustained real audio without pulling the whole track.
    let mut pcm = Vec::new();
    let target = format.sample_rate as usize * format.frame_size() * 5;
    let first_chunk = Instant::now();
    let mut latency = None;
    while pcm.len() < target {
        match decoder.next_chunk() {
            Ok(Some(chunk)) => {
                latency.get_or_insert_with(|| first_chunk.elapsed());
                pcm.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("decode failed: {e}");
                std::process::exit(1);
            }
        }
    }

    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let rms = (samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>()
        / samples.len().max(1) as f64)
        .sqrt();

    println!(
        "first audio after {:?}; decoded {:.1} s, RMS {rms:.0}",
        latency.unwrap_or_default(),
        format.duration_of(pcm.len()).as_secs_f64()
    );

    println!();
    if format.sample_rate >= 44100 && rms > 100.0 {
        println!("=> Full pipeline verified at the best quality this account can get.");
    } else {
        println!("!! Unexpected output — {} Hz, RMS {rms:.0}", format.sample_rate);
        std::process::exit(1);
    }
}
