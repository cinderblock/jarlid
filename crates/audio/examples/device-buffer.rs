//! What device buffer does Windows actually grant, and does the callback keep up with it?
//!
//! Plays a local file **muted** on the default output, at the requested buffer size, and prints
//! the measured buffer, device underruns, and how far the playhead trails the audio delivered.
//! Muting still runs the whole callback, so this measures the real path without making a sound.
//!
//! To see underruns, load the machine while it runs, from somewhere else (not by building on it
//! at the same time; see `plans/audio-dropouts-under-load.md`).
//!
//! Run: cargo run --example device-buffer [buffer-ms] [seconds] [file]

use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let buffer_ms: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(30);
    let seconds: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(5);
    let file = args
        .next()
        .unwrap_or_else(|| r"C:\Windows\Media\Ring05.wav".into());

    let player = audio::Player::play_on(
        &file,
        Duration::ZERO,
        &audio::Output::Default,
        Duration::from_millis(buffer_ms),
    )
    .expect("open");
    player.set_volume(0.0);

    println!(
        "asked for {buffer_ms} ms on {:?} at {} Hz",
        player.device_name(),
        player.format().sample_rate
    );
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) && !player.is_finished() {
        std::thread::sleep(Duration::from_millis(500));
        println!(
            "{:>5.1}s  buffer {:>6.1} ms  underruns {:>3}  position {:>6.3}s  decoded-ahead {:>5.2}s  drift {:.3}s",
            start.elapsed().as_secs_f64(),
            player.device_buffer().as_secs_f64() * 1000.0,
            player.device_underruns(),
            player.position().as_secs_f64(),
            player.buffered().as_secs_f64(),
            player.drift().as_secs_f64(),
        );
    }
}
