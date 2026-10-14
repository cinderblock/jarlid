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
    /// Stop a transcription, queued or running.
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
