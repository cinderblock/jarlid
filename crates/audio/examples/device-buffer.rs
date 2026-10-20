//! What device buffer does Windows actually grant, and does the callback keep up with it?
//!
//! Plays a local file **muted** on the default output, at the requested buffer size, and prints
//! the measured buffer, device underruns, how far the playhead trails the audio delivered, and how
//! the callback kept time: its period, its longest wait, and its slowest run.
//! Muting still runs the whole callback, so this measures the real path without making a sound.
//!
//! If the file ends inside the run, it also prints how long the device took to play out the end
//! of it after the last sample was handed over. That should be about the buffer size: the player
//! is not finished until it has, so a caller dropping it on `is_finished` cuts nothing off.
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
    let mut last_report = Instant::now();
    let mut drained_at: Option<Instant> = None;
    while start.elapsed() < Duration::from_secs(seconds) && !player.is_finished() {
        // Polled finely so the tail can be timed; reported at the old half-second pace.
        std::thread::sleep(Duration::from_millis(2));
        if player.drained() && drained_at.is_none() {
            drained_at = Some(Instant::now());
        }
        if last_report.elapsed() < Duration::from_millis(500) {
            continue;
        }
        last_report = Instant::now();
        let timing = player.take_callback_timing();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        println!(
            "{:>5.1}s  buffer {:>6.1} ms  underruns {:>3}  position {:>6.3}s  decoded-ahead {:>5.2}s  drift {:.3}s  period {:>5.1} ms  longest wait {:>6.1} ms  slowest {:.2} ms",
            start.elapsed().as_secs_f64(),
            ms(player.device_buffer()),
            player.device_underruns(),
            player.position().as_secs_f64(),
            player.buffered().as_secs_f64(),
            player.drift().as_secs_f64(),
            timing.shortest_gap.map_or(0.0, ms),
            ms(timing.longest_gap),
            ms(timing.longest_callback),
        );
    }
    if let Some(at) = drained_at.filter(|_| player.is_finished()) {
        println!(
            "end of file: the device played out its last {:.1} ms after the last sample was handed over",
            at.elapsed().as_secs_f64() * 1000.0
        );
    }
}
