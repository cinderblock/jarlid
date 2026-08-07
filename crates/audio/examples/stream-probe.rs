//! Can Media Foundation open Pandora's signed HTTPS audio URL directly?
//!
//! decode-probe downloads to a temp file first. If MF's own scheme handlers can open the URL, the
//! whole "custom IMFByteStream" work item disappears and playback can start before the download
//! finishes. This is a cheap test of an expensive assumption.
//!
//! Run: cargo run --example stream-probe

use std::time::Instant;

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    println!("=== fetching a track URL (anonymous tier) ===");
    let (track, _) = match runtime.block_on(pandora::demo::anonymous_track("Pink Floyd")) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("FAILED: {e}");
            std::process::exit(1);
        }
    };
    println!("{} — {}", track.title, track.artist);
    println!("host: {}", track.audio_url.split('/').nth(2).unwrap_or("?"));

    println!("\n=== opening the HTTPS URL directly with Media Foundation ===");
    let started = Instant::now();
    match audio::Decoder::open(&track.audio_url) {
        Ok(mut decoder) => {
            let format = decoder.format();
            println!(
                "OPENED in {:?} — {} Hz, {} ch, {}-bit",
                started.elapsed(),
                format.sample_rate,
                format.channels,
                format.bits_per_sample
            );

            // Time to *first audio* is the number that matters for playback latency: it shows MF
            // is streaming progressively rather than buffering the whole file before decoding.
            let first = Instant::now();
            match decoder.next_chunk() {
                Ok(Some(chunk)) => {
                    println!(
                        "first PCM chunk: {} bytes after {:?}",
                        chunk.len(),
                        first.elapsed()
                    );
                    println!("\n=> MF streams the URL directly. No custom IMFByteStream needed.");
                }
                Ok(None) => println!("\n!! Opened, but produced no audio."),
                Err(e) => println!("\n!! Opened, but decoding failed: {e}"),
            }
        }
        Err(e) => {
            println!("FAILED after {:?}: {e}", started.elapsed());
            println!("\n=> MF will not open the URL. A custom IMFByteStream fed from the network");
            println!("   is required for streaming playback. Keep that work item.");
        }
    }
}
