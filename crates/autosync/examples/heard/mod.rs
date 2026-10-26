//! Reading the speech models' output saved by the step-1 spike (`spike/run-asr.sh`), for the
//! benches. Shared by `bench` (alignment) and `draft` (lyrics from nothing).
//!
//! - whisper-cli `-ojf` JSON. Word times come from the DTW token stamps when present (`t_dtw`),
//!   or from the segment-relative token offsets ([`Timing`]).
//! - parakeet-cli `-ps` token dump, whole-track or windowed.
//! - Whistle's `needle --audio-word-timestamps` JSON.

// Each bench uses its own subset.
#![allow(dead_code)]

use std::path::Path;

use autosync::Heard;
use serde_json::Value;

#[derive(Clone, Copy)]
pub enum Timing {
    /// Whisper's DTW token stamps; for Parakeet, its own frame stamps.
    Dtw,
    /// Whisper's token offsets.
    Offsets,
}

pub fn load_heard(dir: &Path, track: &str, timing: Timing) -> Option<Vec<Heard>> {
    if let Ok(json) = std::fs::read_to_string(dir.join(format!("{track}.json"))) {
        let v: Value = serde_json::from_str(&json).ok()?;
        if v.get("words").is_some() {
            return Some(whistle_words(&v));
        }
        return Some(whisper_words(&v, timing));
    }
    let dump = std::fs::read_to_string(dir.join(format!("{track}.txt"))).ok()?;
    Some(parakeet_words(&dump))
}

/// Words from Whistle's `needle --audio-word-timestamps` JSON: `{"words":[{"word","start","end",
/// "probability"}]}`, times in seconds.
fn whistle_words(v: &Value) -> Vec<Heard> {
    v["words"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| Heard {
            text: w["word"].as_str().unwrap_or_default().to_string(),
            start: w["start"].as_f64().unwrap_or_default(),
            end: w["end"].as_f64().unwrap_or_default(),
            p: w["probability"].as_f64().unwrap_or_default() as f32,
        })
        .collect()
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
        Some(line[at..].split_whitespace().next()?.to_string())
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
