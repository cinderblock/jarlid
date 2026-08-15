//! Will Pandora serve a second audio stream while the first is still playing?
//!
//! This gates the whole DJ-blend idea. A crossfade needs both tracks producing audio at once, and
//! `pandora::Client` warns that Pandora allows only one concurrent stream per account — the
//! `STREAM_VIOLATION` the app already has recovery code for. If that limit is enforced on the
//! signed CDN URLs, blending is a non-starter in its obvious form and has to become "pre-decode
//! the next track into RAM, then close that connection".
//!
//! So the probe mirrors the real scenario rather than testing something easier:
//!
//! 1. Open track A and read it at roughly **playback speed**, as the player would.
//! 2. Five seconds in — while A is still open and being consumed — open track B and pull the
//!    first chunk of it as fast as it will come, which is the pre-buffer we would actually do.
//! 3. Keep reading A afterwards, because the failure that matters most is not "B was refused"
//!    but "opening B silently killed A".
//! 4. Then ask the tuner API for a playlist, to see whether the *account* was flagged even if
//!    the bytes flowed.
//!
//! Read-only: it fetches audio and asks for a playlist, exactly as normal playback does. It
//! never writes anything to the account.
//!
//! Run: cargo run -p engine --example concurrent-streams

use std::time::{Duration, Instant};

use futures_util::StreamExt;

/// How long to hold the first stream open, in total.
const HOLD_A: Duration = Duration::from_secs(20);

/// How far into A to start B. Long enough that A is unambiguously established.
const OPEN_B_AT: Duration = Duration::from_secs(5);

/// What the pre-buffer would actually grab: ~30 s of 128 kbit/s MP3 is about 480 KB. Ask for a
/// bit more so the read is a realistic burst rather than a token request.
const B_TARGET_BYTES: usize = 768 * 1024;

/// 128 kbit/s is 16 KB/s. Reading A at about that rate keeps it looking like playback rather
/// than a download, which is the behaviour any server-side limit would be watching for.
const A_BYTES_PER_SECOND: usize = 16 * 1024;

#[tokio::main]
async fn main() {
    let Some(creds) = engine::credentials::load().ok().flatten() else {
        eprintln!("No stored credentials. Sign in through the app first, or run the");
        eprintln!("`seed-credentials` example.");
        std::process::exit(2);
    };
    println!("account: {}", creds.username);

    let mut client = match pandora::Client::login(&creds.username, &creds.password).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("login failed: {e}");
            std::process::exit(1);
        }
    };

    let stations = client.station_list().await.expect("stations");
    let station = stations.first().expect("a station");
    let token = station.station_token.clone();
    println!("station: {}\n", station.station_name);

    let tracks = match client.playlist(&token).await {
        Ok(t) => t,
        Err(e) if e.is_stream_violation() => {
            eprintln!("STREAM_VIOLATION before we even started — something else is playing.");
            eprintln!("Close other Pandora clients and retry; this probe needs a clear field.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("playlist failed: {e}");
            std::process::exit(1);
        }
    };

    // Two *different* tracks, because a blend is always between two songs. Reusing one URL twice
    // would test connection pooling rather than Pandora's stream accounting.
    let playable: Vec<_> = tracks.iter().filter(|t| !t.audio_url.is_empty()).collect();
    let (Some(a), Some(b)) = (playable.first(), playable.get(1)) else {
        eprintln!("need two playable tracks; got {}", playable.len());
        std::process::exit(1);
    };
    println!("A: {}", a.describe());
    println!("B: {}\n", b.describe());

    let http = reqwest::Client::new();
    let started = Instant::now();

    // --- stream A at playback speed -------------------------------------------------------
    let a_url = a.audio_url.clone();
    let a_http = http.clone();
    let a_task = tokio::spawn(async move {
        let response = match a_http.get(&a_url).send().await {
            Ok(r) => r,
            Err(e) => return Err(format!("A: request failed: {e}")),
        };
        let status = response.status();
        println!(
            "[{:>5.1}s] A opened: HTTP {status}",
            started.elapsed().as_secs_f64()
        );
        if !status.is_success() {
            return Err(format!("A: HTTP {status}"));
        }

        let mut stream = response.bytes_stream();
        let mut total = 0usize;
        let mut last_report = Instant::now();
        while started.elapsed() < HOLD_A {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    total += chunk.len();
                    // Throttle to roughly playback rate.
                    let should_have_taken =
                        Duration::from_secs_f64(total as f64 / A_BYTES_PER_SECOND as f64);
                    if let Some(sleep) = should_have_taken.checked_sub(started.elapsed()) {
                        tokio::time::sleep(sleep).await;
                    }
                    if last_report.elapsed() > Duration::from_secs(5) {
                        last_report = Instant::now();
                        println!(
                            "[{:>5.1}s] A still reading: {} KB",
                            started.elapsed().as_secs_f64(),
                            total / 1024
                        );
                    }
                }
                // The critical failure: A dies partway through, which in the app would be a
                // dropout mid-song rather than an error anyone sees.
                Some(Err(e)) => {
                    return Err(format!(
                        "A: READ FAILED after {} KB at {:.1}s: {e}",
                        total / 1024,
                        started.elapsed().as_secs_f64()
                    ))
                }
                None => {
                    println!(
                        "[{:>5.1}s] A: stream ended after {} KB",
                        started.elapsed().as_secs_f64(),
                        total / 1024
                    );
                    break;
                }
            }
        }
        Ok(total)
    });

    // --- open B partway through -----------------------------------------------------------
    tokio::time::sleep(OPEN_B_AT).await;
    println!(
        "[{:>5.1}s] --- opening B while A is still streaming ---",
        started.elapsed().as_secs_f64()
    );

    let b_result: Result<usize, String> = async {
        let response = http
            .get(&b.audio_url)
            .send()
            .await
            .map_err(|e| format!("B: request failed: {e}"))?;
        let status = response.status();
        println!(
            "[{:>5.1}s] B opened: HTTP {status}",
            started.elapsed().as_secs_f64()
        );
        if !status.is_success() {
            return Err(format!("B: HTTP {status}"));
        }
        let mut stream = response.bytes_stream();
        let mut total = 0usize;
        while total < B_TARGET_BYTES {
            match stream.next().await {
                Some(Ok(chunk)) => total += chunk.len(),
                Some(Err(e)) => {
                    return Err(format!("B: read failed after {} KB: {e}", total / 1024))
                }
                None => break,
            }
        }
        Ok(total)
    }
    .await;

    let b_elapsed = started.elapsed().as_secs_f64();
    match &b_result {
        Ok(n) => println!("[{b_elapsed:>5.1}s] B pre-buffered {} KB", n / 1024),
        Err(e) => println!("[{b_elapsed:>5.1}s] B FAILED: {e}"),
    }

    let a_result = a_task.await.expect("A task");

    // --- did the account get flagged? -----------------------------------------------------
    println!("\n--- asking the tuner API for another playlist ---");
    let api_after = match client.playlist(&token).await {
        Ok(t) => format!("OK, {} tracks", t.len()),
        Err(e) if e.is_stream_violation() => "STREAM_VIOLATION".to_string(),
        Err(e) => format!("failed: {e}"),
    };
    println!("{api_after}");

    // --- verdict --------------------------------------------------------------------------
    println!("\n================ VERDICT ================");
    match (&a_result, &b_result) {
        (Ok(a_bytes), Ok(b_bytes)) => {
            println!("A survived: {} KB read at playback speed", a_bytes / 1024);
            println!("B pre-buffered concurrently: {} KB", b_bytes / 1024);
            println!("Tuner API afterwards: {api_after}");
            if api_after == "STREAM_VIOLATION" {
                println!("\nBytes flowed, but the ACCOUNT was flagged. Two concurrent reads are");
                println!("tolerated by the CDN and punished by the API. Pre-buffering is still");
                println!("possible, but the next getPlaylist has to expect this and recover.");
            } else {
                println!("\n✅ Two concurrent audio streams are served, the first is unharmed,");
                println!("and the account is not flagged. The pre-buffer design is viable.");
            }
        }
        (Err(a_err), _) => {
            println!("❌ {a_err}");
            println!("\nOpening B broke A. This is the worst outcome: in the app it would be a");
            println!("dropout mid-song. Blending would need the incoming track fully buffered");
            println!("BEFORE the outgoing one is anywhere near its end, with only one connection");
            println!("ever open at a time.");
        }
        (Ok(a_bytes), Err(b_err)) => {
            println!("A survived: {} KB", a_bytes / 1024);
            println!("❌ {b_err}");
            println!("\nThe second stream was refused while the first was open. Pre-buffering has");
            println!("to happen in a gap — between tracks, or by pausing the outgoing stream's");
            println!("reads long enough to fetch, which the 5 s ring buffer may not cover.");
        }
    }
}
