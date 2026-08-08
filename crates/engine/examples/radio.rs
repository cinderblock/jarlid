//! A working radio: pick a station, play it continuously, auto-advance between tracks.
//!
//! This is the whole native client end to end — no browser, no webview, no DOM scraping.
//!
//! Run: cargo run --release --example radio [minutes] [volume] [station-name-substring]

use std::time::Duration;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let minutes: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(2);
    let volume: f32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(0.3);
    let wanted = args.next();

    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD.");
        std::process::exit(2);
    };

    println!("logging in …");
    let (engine, mut events) = match engine::Engine::start(&username, &password).await {
        Ok(started) => started,
        Err(e) => {
            eprintln!("login failed: {e}");
            std::process::exit(1);
        }
    };

    let stations = engine.tuner_stations().await.expect("stations");
    let (name, token) = match &wanted {
        Some(want) => stations
            .iter()
            .find(|(name, _)| name.to_lowercase().contains(&want.to_lowercase()))
            .unwrap_or_else(|| {
                eprintln!("no station matching {want:?}; using the first");
                &stations[0]
            })
            .clone(),
        None => stations.first().expect("a station").clone(),
    };

    engine.set_volume(volume);
    if let Err(e) = engine.play_station(&name, &token).await {
        eprintln!("could not start: {e}");
        std::process::exit(1);
    }

    // Report events as they happen — this is the stream a UI would subscribe to.
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                engine::Event::TrackStarted(track) => {
                    println!(
                        "\n▶  {}\n   {} · {} · {}s · {}px art",
                        track.describe(),
                        track.album_title,
                        track.audio_encoding,
                        track.track_length,
                        track.hero_art().map(|a| a.size).unwrap_or(0),
                    );
                }
                engine::Event::TrackEnded => println!("   (ended)"),
                engine::Event::StationChanged(name) => println!("📻 station: {name}"),
                engine::Event::ModeChanged(name) => println!("🎛  mode: {name}"),
                engine::Event::Paused(paused) => {
                    println!("   {}", if paused { "paused" } else { "resumed" })
                }
                engine::Event::StreamTaken => {
                    eprintln!("\n!! Pandora is playing on another device — stop it and retry.");
                }
                engine::Event::Error(message) => eprintln!("   error: {message}"),
            }
        }
    });

    // Drive auto-advance in the background; the loop below just observes.
    let radio = async { engine.run().await };
    let watch = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(minutes * 60);
        let mut ticks = 0u32;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_secs(5)).await;
            ticks += 1;

            let drift = engine.drift();
            println!(
                "   {:>5.1}s   buffered {:>4.1}s   drift {:>4.2}s",
                engine.position().as_secs_f64(),
                engine.buffered().as_secs_f64(),
                drift.as_secs_f64(),
            );
            if drift > Duration::from_millis(500) {
                eprintln!("   !! drift climbing — audio is being dropped");
            }

            // The skip test is opt-in: during a listening check an unannounced track change just
            // sounds like a fault. Pass `skip` to exercise it.
            if ticks == 3 && std::env::args().any(|a| a == "skip") {
                println!("   -- skip (deliberate) --");
                if let Err(e) = engine.skip().await {
                    eprintln!("   skip failed: {e}");
                }
            }
        }
    };

    tokio::select! {
        _ = radio => {}
        _ = watch => {}
    }

    println!("\ndone.");
}
