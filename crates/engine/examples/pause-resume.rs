//! Reproduce the "left it idle, now play does nothing" bug, and prove it is fixed.
//!
//! Pauses a real track for longer than the audio thread's `RELEASE_AFTER_PAUSE`, so the player is
//! torn down and the network connection dropped exactly as it would be after a long idle, then
//! presses play and checks that the *same track* resumes from the *same position* — rather than
//! sitting silent until you hit next.
//!
//! Silent by default (volume 0) so it can run without taking over the speakers; pass a volume to
//! actually listen to the seam.
//!
//! Run: cargo run --release --example pause-resume [pause-seconds] [volume] [station-substring]

use std::time::Duration;

/// Must exceed `audio_thread::RELEASE_AFTER_PAUSE` (45 s) or the player is never released and the
/// test proves nothing.
const DEFAULT_PAUSE: u64 = 60;

/// How much the resumed position may differ from where we paused. Media Foundation seeks to a
/// nearby frame boundary, so this is not zero — but it is nowhere near "restarted from the top".
const TOLERANCE: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let pause_for = Duration::from_secs(
        args.next()
            .and_then(|a| a.parse().ok())
            .unwrap_or(DEFAULT_PAUSE),
    );
    let volume: f32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(0.0);
    let wanted = args.next();

    // Prefer the credentials the app itself uses, so this runs with no environment set up.
    let started = match engine::Engine::start_from_saved().await {
        Ok(started) => Some(started),
        Err(e) => {
            eprintln!("no saved credentials ({e}); falling back to the environment");
            match pandora::demo::credentials() {
                Some((user, pass)) => engine::Engine::start(&user, &pass).await.ok(),
                None => None,
            }
        }
    };
    let Some((engine, mut events)) = started else {
        eprintln!("could not log in — sign in to the app, or set PANDORA_USERNAME/PASSWORD.");
        std::process::exit(2);
    };

    let stations = engine.station_list().await.expect("stations");
    let station = match &wanted {
        Some(want) => stations
            .iter()
            .find(|s| s.station_name.to_lowercase().contains(&want.to_lowercase()))
            .unwrap_or(&stations[0]),
        None => stations.first().expect("a station"),
    };

    engine.set_volume(volume);
    engine
        .play_station(&station.station_name, &station.station_token)
        .await
        .expect("start playback");

    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                engine::Event::TrackStarted(track) => println!("▶  {}", track.describe()),
                engine::Event::Error(message) => eprintln!("!! {message}"),
                _ => {}
            }
        }
    });

    let test = async {
        // Let it get properly going first, so a resume failure can't be confused with a slow start.
        let playing = wait_for(&engine, Duration::from_secs(4), Duration::from_secs(30)).await;
        if !playing {
            eprintln!("FAIL: never reached 4 s of playback at all");
            std::process::exit(1);
        }
        let before = engine.position();
        let track = engine.now_playing().await.map(|t| t.describe());
        println!("paused at {:.1}s", before.as_secs_f64());
        engine.set_paused(true);

        println!(
            "waiting {}s — long enough for the player to be released …",
            pause_for.as_secs()
        );
        tokio::time::sleep(pause_for).await;

        // The moment of truth: the old build did nothing here.
        println!("play");
        engine.set_paused(false);

        let resumed = wait_for(
            &engine,
            before + Duration::from_millis(500),
            Duration::from_secs(20),
        )
        .await;
        let after = engine.position();
        let same_track = engine.now_playing().await.map(|t| t.describe()) == track;

        println!(
            "resumed at {:.1}s (was {:.1}s), same track: {same_track}",
            after.as_secs_f64(),
            before.as_secs_f64()
        );

        let mut ok = true;
        if !resumed {
            eprintln!("FAIL: playback did not resume — position never advanced past the pause");
            ok = false;
        }
        if !same_track {
            eprintln!("FAIL: resumed onto a different track instead of continuing this one");
            ok = false;
        }
        if after.abs_diff(before) > TOLERANCE {
            eprintln!(
                "FAIL: resumed {:.1}s away from where it paused",
                after.abs_diff(before).as_secs_f64()
            );
            ok = false;
        }
        if engine.drift() > Duration::from_millis(500) {
            eprintln!(
                "FAIL: drift {:.2}s — audio is being dropped",
                engine.drift().as_secs_f64()
            );
            ok = false;
        }

        println!("\n{}", if ok { "PASS" } else { "FAILED" });
        std::process::exit(if ok { 0 } else { 1 });
    };

    // Run the radio alongside, so a track ending mid-test behaves as it would in the app.
    tokio::select! {
        _ = engine.run() => {}
        _ = test => {}
    }
}

/// Wait until playback passes `target`, or give up. Returns whether it got there.
async fn wait_for(engine: &engine::Engine, target: Duration, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if engine.position() >= target {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}
