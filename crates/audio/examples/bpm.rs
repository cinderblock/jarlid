//! Measure the tempo of local audio files, the same way playback does.
//!
//! The unit tests prove the detector against synthetic click tracks, where the answer is known
//! exactly. This is the other half: real music, where the answer is only knowable by listening.
//! It decodes through the same [`Decoder`] the player uses, at the same 48 kHz stereo the
//! player asks for, and feeds the same [`TempoTracker`] — so a number printed here is the number
//! the app's technical readout would show.
//!
//! Prints the estimate as it firms up, because a tempo that wanders is a tempo that is wrong:
//! a correct reading settles within the first few windows and then stops moving.
//!
//! ```text
//! cargo run -p audio --example bpm -- "C:\path\to\track.mp3" [more.mp3 ...]
//! ```

use audio::{Decoder, Format, TempoTracker};

/// What the player asks for, so this measures what playback would measure.
const OUTPUT: Format = Format {
    sample_rate: 48_000,
    channels: 2,
    bits_per_sample: 16,
};

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: cargo run -p audio --example bpm -- <file> [file ...]");
        std::process::exit(2);
    }

    for path in &files {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());

        match measure(path) {
            Ok(report) => println!("{name}\n{report}"),
            Err(e) => println!("{name}\n  failed: {e}\n"),
        }
    }
}

fn measure(path: &str) -> audio::Result<String> {
    let mut decoder = Decoder::open_at(path, Some(OUTPUT))?;
    let format = decoder.format();
    let source = decoder.source();

    let mut tracker = TempoTracker::new(format.sample_rate, format.channels);
    let mut samples = 0u64;
    let mut next_report = 0u64;
    let mut trail = String::new();

    // Report every five seconds of audio, so a wandering estimate is visible rather than
    // averaged away by only ever printing the last one.
    let step = format.sample_rate as u64 * format.channels as u64 * 5;

    while let Some(chunk) = decoder.next_chunk()? {
        let pcm: Vec<i16> = chunk
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        samples += pcm.len() as u64;
        tracker.push(&pcm);

        if samples >= next_report {
            next_report = samples + step;
            let at = samples / (format.sample_rate as u64 * format.channels as u64);
            match tracker.tempo() {
                Some(t) => trail.push_str(&format!(
                    "  {at:>3}s  {:>6.1} BPM  ({:.2})\n",
                    t.bpm, t.confidence
                )),
                None => trail.push_str(&format!("  {at:>3}s     —\n")),
            }
        }
    }

    let heard = samples as f64 / (format.sample_rate as f64 * format.channels as f64);
    let mut report = String::new();
    if let Some(s) = source {
        report.push_str(&format!(
            "  source: {} {} kbit/s, {} Hz, {} ch  ->  decoded at {} Hz\n",
            s.codec, s.bitrate_kbps, s.sample_rate, s.channels, format.sample_rate
        ));
    }
    report.push_str(&format!("  length: {heard:.0}s\n"));
    report.push_str(&trail);
    match tracker.tempo() {
        Some(t) => report.push_str(&format!(
            "  FINAL:  {:.1} BPM  (confidence {:.2})\n",
            t.bpm, t.confidence
        )),
        None => report.push_str("  FINAL:  no tempo found\n"),
    }
    Ok(report)
}
