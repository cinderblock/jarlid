//! Actually play music: login → station → 128 kbps → speakers.
//!
//! Plays at reduced volume for a short spell by default, since this is a verification run rather
//! than a listening session.
//!
//! Run: cargo run --example play [seconds] [volume]

use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(10);
    let volume: f32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(0.25);

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
    println!("station:  {name}");
    println!("track:    {}", track.describe());
    println!("encoding: {} · {} s", track.audio_encoding, track.track_length);
    println!("\nplaying {seconds}s at {:.0}% volume …\n", volume * 100.0);

    let started = Instant::now();
    let player = match audio::Player::play(&track.audio_url) {
        Ok(player) => player,
        Err(e) => {
            eprintln!("playback failed: {e}");
            std::process::exit(1);
        }
    };
    player.set_volume(volume);

    let format = player.format();
    println!(
        "output: {} Hz, {} ch (started in {:?})",
        format.sample_rate,
        format.channels,
        started.elapsed()
    );

    // The pause test is opt-in: during a listening check an unexplained gap just sounds like a
    // fault. Pass `pause` as the third argument to exercise it.
    let test_pause = std::env::args().any(|a| a == "pause");
    let mut paused_once = false;

    while player.position() < Duration::from_secs(seconds) && !player.is_finished() {
        std::thread::sleep(Duration::from_millis(500));
        // `drift` must stay ~0. If it climbs, decoded samples are being lost — the track races
        // ahead of real time and the stereo channels can swap. Watch it, don't just listen.
        println!(
            "  position {:>5.1}s   buffered {:>4.1}s   decoded {:>5.1}s   drift {:>5.2}s{}",
            player.position().as_secs_f64(),
            player.buffered().as_secs_f64(),
            player.decoded().as_secs_f64(),
            player.drift().as_secs_f64(),
            if player.is_paused() { "   [paused]" } else { "" }
        );

        if test_pause && !paused_once && player.position() > Duration::from_secs(2) {
            paused_once = true;
            println!("  -- pausing for 1s (deliberate) --");
            player.set_paused(true);
            std::thread::sleep(Duration::from_secs(1));
            player.set_paused(false);
        }
    }

    let played = player.position();
    println!("\nplayed {:.1}s", played.as_secs_f64());

    if played.as_secs_f64() > 1.0 {
        println!("=> Music played through the speakers. Native client is audible.");
    } else {
        println!("!! No audio advanced — position never moved.");
        std::process::exit(1);
    }
}
