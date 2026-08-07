//! End-to-end audio spike: fetch a real Pandora stream (anonymous tier, no account) and decode it
//! with Media Foundation.
//!
//! The decisive check is the negotiated output sample rate. Pandora's AAC core is 22050 Hz and SBR
//! reconstructs it to 44100 Hz, so:
//!   * 44100 Hz out => SBR was applied. Media Foundation is doing the job Symphonia cannot.
//!   * 22050 Hz out => only the core layer decoded, which is exactly the failure we're avoiding.
//!
//! Run: cargo run --example decode-probe

use std::io::Write;

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    println!("=== 1. fetch a real Pandora stream (anonymous tier) ===");
    let (track, _) = match runtime.block_on(pandora::demo::anonymous_track("Pink Floyd")) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("FAILED to get a track: {e}");
            std::process::exit(1);
        }
    };
    println!("track:    {} — {}", track.title, track.artist);
    println!("encoding: {}", track.encoding);
    println!("length:   {} s (as reported by Pandora)", track.length_seconds);
    if track.xor_key.is_some() {
        println!("!! XOR key present — audio is masked and must be un-masked before decoding.");
    }

    println!("\n=== 2. download ===");
    let bytes = match runtime.block_on(async {
        reqwest::get(&track.audio_url).await?.bytes().await
    }) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("FAILED to download: {e}");
            std::process::exit(1);
        }
    };
    println!("{} bytes", bytes.len());

    // Media Foundation wants a URL or path; a temp file is the simplest faithful input. Real
    // playback will feed MF a custom IMFByteStream so nothing has to touch disk.
    let path = std::env::temp_dir().join("pandora-decode-probe.m4a");
    match std::fs::File::create(&path).and_then(|mut f| f.write_all(&bytes)) {
        Ok(()) => println!("wrote {}", path.display()),
        Err(e) => {
            eprintln!("FAILED to write temp file: {e}");
            std::process::exit(1);
        }
    }

    println!("\n=== 3. decode with Media Foundation ===");
    let mut decoder = match audio::Decoder::open(&path.to_string_lossy()) {
        Ok(decoder) => decoder,
        Err(e) => {
            eprintln!("FAILED to open: {e}");
            std::process::exit(1);
        }
    };

    let format = decoder.format();
    println!("negotiated output: {} Hz, {} ch, {}-bit PCM",
        format.sample_rate, format.channels, format.bits_per_sample);

    let pcm = match decoder.decode_all() {
        Ok(pcm) => pcm,
        Err(e) => {
            eprintln!("FAILED to decode: {e}");
            std::process::exit(1);
        }
    };
    let duration = format.duration_of(pcm.len());
    println!("decoded {} bytes = {:.1} s of audio", pcm.len(), duration.as_secs_f64());

    println!("\n=== 4. VERDICT ===");

    // Prove it is real audio and not a buffer of silence, which is how a subtly broken decode
    // configuration usually presents.
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let rms = (samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>()
        / samples.len().max(1) as f64)
        .sqrt();
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    println!("RMS {rms:.0}, peak {peak} (of 32767)");

    let mut ok = true;

    if rms < 100.0 {
        println!("!! Near-silent output — the decode is not producing real audio.");
        ok = false;
    } else {
        println!("OK — real audio, not silence.");
    }

    if format.sample_rate >= 44100 {
        println!("OK — {} Hz output: SBR WAS applied.", format.sample_rate);
    } else {
        println!(
            "!! {} Hz output: only the AAC core decoded, SBR was NOT applied.",
            format.sample_rate
        );
        ok = false;
    }

    // A large shortfall against Pandora's own track length would mean we dropped audio.
    let expected = track.length_seconds as f64;
    if expected > 0.0 {
        let ratio = duration.as_secs_f64() / expected;
        if ratio < 0.95 {
            println!("!! Decoded only {:.0}% of the reported {expected:.0} s — audio was dropped.",
                ratio * 100.0);
            ok = false;
        } else {
            println!("OK — decoded {:.0}% of the reported length.", ratio * 100.0);
        }
    }

    let _ = std::fs::remove_file(&path);

    println!();
    if ok {
        println!("=> Media Foundation decodes Pandora's HE-AAC correctly. Audio path confirmed.");
    } else {
        println!("=> Audio path NOT confirmed. See the failures above.");
        std::process::exit(1);
    }
}
