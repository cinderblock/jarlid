//! The JSON-lines protocol between Jarlid and this sidecar.
//!
//! One JSON object per line each way: [`Command`]s on stdin, [`Event`]s on stdout. stderr carries
//! the engine's own log, for the app to keep or drop.
//!
//! The sidecar only hears. It returns timed words, one window at a time; lining them up with the
//! lyrics is the app's job, so aligner improvements ship with the app instead of needing this
//! sidecar re-released and re-pinned.
//!
//! ```text
//! ← {"ev":"ready","protocol":1,"version":"0.1.0"}
//! → {"op":"load","model":"C:\\…\\ggml-parakeet-tdt-0.6b-v3-q8_0.bin","gpu":true,"threads":3}
//! ← {"ev":"loaded","ms":2100,"engine":"1.9.5"}
//! → {"op":"transcribe","id":"TR:123","wav":"C:\\…\\TR123.wav","from":0.0}
//! ← {"ev":"words","id":"TR:123","start":0.0,"end":30.0,"done":1,"of":11,"words":[…]}
//! ← {"ev":"done","id":"TR:123","ms":95000}
//! → {"op":"cancel","id":"TR:123"}
//! ← {"ev":"cancelled","id":"TR:123"}
//! ```
//!
//! Since 0.2.0, the optional forced aligner (its own download; see `aligner.rs`): the emission
//! of each span of the track, for the app to align known words on. Added messages only, so the
//! protocol number stands; the app only sends these to an engine whose manifest lists the
//! aligner.
//!
//! ```text
//! → {"op":"loadAligner","model":"C:\\…\\mms-fa-int8.onnx","runtime":"C:\\…\\onnxruntime.dll","threads":3}
//! ← {"ev":"alignerLoaded","ms":900}
//! → {"op":"emit","id":"TR:123","wav":"C:\\…\\TR123.wav","spans":[[11.2,14.9],…]}
//! ← {"ev":"emission","id":"TR:123","k":0,"t0":11.2,"ratio":0.02,"frames":185,"data":"…"}
//! ← {"ev":"done","id":"TR:123","ms":21000}
//! ```

use serde::{Deserialize, Serialize};

/// Bumped whenever a message changes shape. The app refuses a sidecar speaking another version.
pub const PROTOCOL: u32 = 1;

/// What the app asks for.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Command {
    /// Load (or replace) the speech model.
    Load {
        model: String,
        #[serde(default = "yes")]
        gpu: bool,
        #[serde(default = "default_threads")]
        threads: u32,
    },
    /// Hear a whole track. `wav` is 16 kHz mono 16-bit PCM; `from` (seconds) is where to start,
    /// for a song already playing — the rest of the track follows, then what came before.
    Transcribe {
        id: String,
        wav: String,
        #[serde(default)]
        from: f64,
    },
    /// Load (or replace) the forced-alignment model. `runtime` is the path of `onnxruntime.dll`
    /// (default: the one in the engine dir). The runtime loads once per process: a later
    /// `loadAligner` replaces only the model.
    LoadAligner {
        model: String,
        #[serde(default)]
        runtime: Option<String>,
        #[serde(default = "default_threads")]
        threads: u32,
    },
    /// The aligner's emission over each of `spans` (seconds) of `wav` (as for `transcribe`), in
    /// order, then `done`.
    Emit {
        id: String,
        wav: String,
        spans: Vec<(f64, f64)>,
    },
    /// Stop a transcription or emission, queued or running.
    Cancel { id: String },
    /// Exit. Closing stdin does the same.
    Quit,
}

fn yes() -> bool {
    true
}

fn default_threads() -> u32 {
    3
}

/// One word heard, in seconds from the start of the track.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub p: f32,
}

/// What the sidecar reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "ev", rename_all = "camelCase")]
pub enum Event {
    Ready {
        protocol: u32,
        version: &'static str,
    },
    Loaded {
        ms: u64,
        engine: String,
    },
    /// The words of one window, already on the track's clock and with the overlap shared out,
    /// so the app can simply collect them. Windows arrive in processing order, not track order.
    Words {
        id: String,
        start: f64,
        end: f64,
        done: usize,
        of: usize,
        words: Vec<Word>,
    },
    AlignerLoaded {
        ms: u64,
    },
    /// The emission over span `k` of an `emit`: `frames` rows of `autosync::ctc::CLASSES`
    /// log-probabilities (star column included), row-major little-endian f32, base64. Frame `f`
    /// starts at `t0 + f × ratio` seconds. `data` is absent when the span is too short to align.
    Emission {
        id: String,
        k: usize,
        t0: f64,
        ratio: f64,
        frames: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    Done {
        id: String,
        ms: u64,
    },
    Cancelled {
        id: String,
    },
    /// `id` is the job it concerns, if any.
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands_with_defaults() {
        let c: Command = serde_json::from_str(r#"{"op":"load","model":"m.bin"}"#).unwrap();
        assert_eq!(
            c,
            Command::Load {
                model: "m.bin".into(),
                gpu: true,
                threads: 3
            }
        );
        let c: Command =
            serde_json::from_str(r#"{"op":"transcribe","id":"a","wav":"a.wav"}"#).unwrap();
        assert_eq!(
            c,
            Command::Transcribe {
                id: "a".into(),
                wav: "a.wav".into(),
                from: 0.0
            }
        );
        let c: Command = serde_json::from_str(
            r#"{"op":"emit","id":"a","wav":"a.wav","spans":[[1.0,2.5],[3,4]]}"#,
        )
        .unwrap();
        assert_eq!(
            c,
            Command::Emit {
                id: "a".into(),
                wav: "a.wav".into(),
                spans: vec![(1.0, 2.5), (3.0, 4.0)]
            }
        );
        let c: Command = serde_json::from_str(r#"{"op":"loadAligner","model":"m.onnx"}"#).unwrap();
        assert_eq!(
            c,
            Command::LoadAligner {
                model: "m.onnx".into(),
                runtime: None,
                threads: 3
            }
        );
        let c: Command = serde_json::from_str(r#"{"op":"quit"}"#).unwrap();
        assert_eq!(c, Command::Quit);
    }

    #[test]
    fn events_are_tagged() {
        let e = Event::Cancelled { id: "a".into() };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"ev":"cancelled","id":"a"}"#
        );
        let e = Event::Error {
            id: None,
            message: "x".into(),
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"ev":"error","message":"x"}"#
        );
    }
}
