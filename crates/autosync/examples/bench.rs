//! Score speech-model output against the corpus's known-good synced lyrics.
//!
//! For each track that has output under `<results>/<config>/`, align the track's *plain* lyrics
//! to the heard words and compare the line times with `synced.lrc`, which the aligner never sees.
//! Writes the generated LRC beside the model output for eyeballing, and prints one row per track
//! and a summary per config.
//!
//! Reads two output formats, both from the step-1 spike (`spike/run-asr.sh`):
//! - whisper-cli `-ojf` JSON. Word times come from the DTW token stamps when present (`t_dtw`),
//!   and from the segment-relative token offsets otherwise; both are scored, as `<config>` and
//!   `<config>+offsets`.
//! - parakeet-cli `-ps` token dump.
//!
//! Run: cargo run --release -p autosync --example bench -- <corpus> <results> <config>…

use std::path::Path;

use autosync::{align, lrc, score, Heard};
use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: bench <corpus> <results> <config>…");
        std::process::exit(2);
    }
    let corpus = Path::new(&args[0]);
    let results = Path::new(&args[1]);

    let mut summaries = Vec::new();
    for config in &args[2..] {
        let variants: &[(&str, Timing)] = if config.starts_with("whisper") {
            &[("", Timing::Dtw), ("+offsets", Timing::Offsets)]
        } else {
            &[("", Timing::Dtw)]
        };
        for (suffix, timing) in variants {
            let name = format!("{config}{suffix}");
            println!("\n== {name}");
            println!(
                "{:<48} {:>5} {:>6} {:>6} {:>7} {:>7} {:>6} {:>6}",
                "track", "lines", "cover", "med", "signed", "p90", "<0.3s", "<1s"
            );
            let mut all = score::Score::default();
            let mut tracks = 0;
            let mut entries: Vec<_> = std::fs::read_dir(corpus)
                .expect("corpus dir")
                .flatten()
                .map(|e| e.path())
                .collect();
            entries.sort();
            for dir in entries {
                let track = dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let Some(heard) = load_heard(&results.join(config), &track, *timing) else {
                    continue;
                };
                let (Ok(plain), Ok(synced)) = (
                    std::fs::read_to_string(dir.join("plain.txt")),
                    std::fs::read_to_string(dir.join("synced.lrc")),
                ) else {
                    continue;
                };
                let lines: Vec<&str> = plain.lines().collect();
                let aligned = align::align(&lines, &heard);
                let times: Vec<Option<f64>> = aligned.lines.iter().map(|l| l.t).collect();
                let s = score::score(&lines, &times, &lrc::parse(&synced));

                let generated: Vec<lrc::Line> = lines
                    .iter()
                    .zip(&times)
                    .filter_map(|(text, t)| {
                        t.map(|t| lrc::Line {
                            t,
                            text: text.to_string(),
                        })
                    })
                    .collect();
                let _ = std::fs::write(
                    results.join(config).join(format!("{track}{suffix}.lrc")),
                    lrc::format(&generated),
                );

                println!(
                    "{:<48} {:>5} {:>5.0}% {:>6} {:>7} {:>7} {:>5.0}% {:>5.0}%",
                    track.chars().take(48).collect::<String>(),
                    s.paired,
                    aligned.coverage() * 100.0,
                    secs(s.median_abs()),
                    secs(s.median_signed()),
                    secs(s.p90_abs()),
                    s.within(0.3) * 100.0,
                    s.within(1.0) * 100.0,
                );
                all.lines += s.lines;
                all.paired += s.paired;
                all.timed += s.timed;
                all.errors.extend(s.errors);
                tracks += 1;
            }
            summaries.push((name, tracks, all));
        }
    }

    println!("\n== summary (all lines of all tracks pooled)");
    println!(
        "{:<28} {:>6} {:>6} {:>6} {:>7} {:>7} {:>6} {:>6} {:>6}",
        "config", "tracks", "lines", "med", "signed", "p90", "<0.3s", "<0.5s", "<1s"
    );
    for (name, tracks, s) in summaries {
        println!(
            "{:<28} {:>6} {:>6} {:>6} {:>7} {:>7} {:>5.0}% {:>5.0}% {:>5.0}%",
            name,
            tracks,
            s.paired,
            secs(s.median_abs()),
            secs(s.median_signed()),
            secs(s.p90_abs()),
            s.within(0.3) * 100.0,
            s.within(0.5) * 100.0,
            s.within(1.0) * 100.0,
        );
    }
}

fn secs(v: Option<f64>) -> String {
    v.map_or("-".into(), |v| format!("{v:.2}"))
}

#[derive(Clone, Copy)]
enum Timing {
    /// Whisper's DTW token stamps; for Parakeet, its own frame stamps.
    Dtw,
    /// Whisper's token offsets.
    Offsets,
}

fn load_heard(dir: &Path, track: &str, timing: Timing) -> Option<Vec<Heard>> {
    if let Ok(json) = std::fs::read_to_string(dir.join(format!("{track}.json"))) {
        return Some(whisper_words(&serde_json::from_str(&json).ok()?, timing));
    }
    let dump = std::fs::read_to_string(dir.join(format!("{track}.txt"))).ok()?;
    Some(parakeet_words(&dump))
}

/// Words from whisper-cli's full JSON. A token beginning with a space starts a new word; special
/// tokens (`[_BEG_]`, `[_TT_…]`) are skipped.
fn whisper_words(v: &Value, timing: Timing) -> Vec<Heard> {
    let mut out: Vec<Heard> = Vec::new();
    for segment in v["transcription"].as_array().into_iter().flatten() {
        for token in segment["tokens"].as_array().into_iter().flatten() {
            let text = token["text"].as_str().unwrap_or_default();
            if text.starts_with("[_") || text.is_empty() {
                continue;
            }
            let from = token["offsets"]["from"].as_f64().unwrap_or_default() / 1000.0;
            let to = token["offsets"]["to"].as_f64().unwrap_or_default() / 1000.0;
            let start = match (timing, token["t_dtw"].as_f64()) {
                (Timing::Dtw, Some(cs)) if cs >= 0.0 => cs / 100.0,
                _ => from,
            };
            let p = token["p"].as_f64().unwrap_or_default() as f32;
            match out.last_mut() {
                Some(word) if !text.starts_with(' ') => {
                    word.text.push_str(text);
                    word.end = to;
                }
                _ => out.push(Heard {
                    text: text.trim().to_string(),
                    start,
                    end: to,
                    p,
                }),
            }
        }
    }
    out
}

/// Length of the windows `spike/chunk.py` cuts, seconds. Must match `run-asr.sh`.
const WINDOW: f64 = 30.0;

/// Words from parakeet-cli's `-ps` token dump.
///
/// A whole-track run is one file. A windowed run is many, each announced by a
/// `read_audio_data: reading audio data from '…/w0003_t72.000.wav'` line whose name carries the
/// window's start. Each window's words are moved onto the track's clock, and where windows
/// overlap, each keeps only the half of the overlap nearest its own middle, so no word is
/// counted twice and none is taken from the very edge of a window, where it may be cut off.
fn parakeet_words(dump: &str) -> Vec<Heard> {
    let mut windows: Vec<(f64, String)> = Vec::new();
    for line in dump.lines() {
        if line.contains("reading audio data from") {
            let start = line
                .rsplit_once("_t")
                .and_then(|(_, rest)| rest.split(".wav").next()?.parse::<f64>().ok())
                .unwrap_or(0.0);
            windows.push((start, String::new()));
        } else if let Some((_, body)) = windows.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if windows.len() <= 1 {
        return parakeet_tokens(dump);
    }

    let mut out = Vec::new();
    for (k, (start, body)) in windows.iter().enumerate() {
        let lo = match k {
            0 => f64::NEG_INFINITY,
            _ => (start + windows[k - 1].0 + WINDOW) / 2.0,
        };
        let hi = match windows.get(k + 1) {
            Some((next, _)) => (next + start + WINDOW) / 2.0,
            None => f64::INFINITY,
        };
        for mut word in parakeet_tokens(body) {
            word.start += start;
            word.end += start;
            if word.start >= lo && word.start < hi {
                out.push(word);
            }
        }
    }
    out
}

/// Words from one file's token lines:
/// `[ 3] id= 1103 frame= 13 … p=0.9996 … t0= 104 t1= 128 word_start=true "▁my"`, times in
/// hundredths of a second.
fn parakeet_tokens(dump: &str) -> Vec<Heard> {
    let field = |line: &str, key: &str| -> Option<String> {
        let at = line.find(key)? + key.len();
        Some(
            line[at..]
                .trim_start()
                .split_whitespace()
                .next()?
                .to_string(),
        )
    };
    let mut out: Vec<Heard> = Vec::new();
    for line in dump.lines().filter(|l| l.contains("word_start=")) {
        let (Some(t0), Some(t1), Some(start_word), Some(p)) = (
            field(line, "t0=").and_then(|s| s.parse::<f64>().ok()),
            field(line, "t1=").and_then(|s| s.parse::<f64>().ok()),
            field(line, "word_start="),
            field(line, " p=").and_then(|s| s.parse::<f32>().ok()),
        ) else {
            continue;
        };
        let Some(text) = line.rfind('"').and_then(|end| {
            let open = line[..end].rfind('"')?;
            Some(line[open + 1..end].to_string())
        }) else {
            continue;
        };
        let piece = text.trim_start_matches('▁').to_string();
        match out.last_mut() {
            Some(word) if start_word != "true" => {
                word.text.push_str(&piece);
                word.end = t1 / 100.0;
            }
            _ => out.push(Heard {
                text: piece,
                start: t0 / 100.0,
                end: t1 / 100.0,
                p,
            }),
        }
    }
    out
}
