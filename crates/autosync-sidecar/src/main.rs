//! Jarlid's speech sidecar: hears a track and reports timed words, over a JSON-lines pipe.
//!
//! Downloaded on demand, never shipped in the installer, and run as its own process so nothing the
//! speech engine or a GPU driver does can take playback down with it. See
//! `plans/auto-synced-lyrics.md` §5 and [`protocol`] for the messages.
//!
//! The engine DLLs are loaded from the directory this executable sits in, or from
//! `--engine-dir <dir>`.
//!
//! Two threads: this one reads commands, and a worker owns the engine and runs them in order. The
//! split is what lets a cancel interrupt a transcription mid-window instead of waiting behind it.

mod engine;
mod protocol;
mod wav;

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use autosync::window;
use protocol::{Command, Event, Word, PROTOCOL};

/// What the reader thread and the worker share: which job is running, and which have been
/// cancelled before or while they run.
#[derive(Default)]
struct Shared {
    current: Mutex<Option<String>>,
    cancelled: Mutex<HashSet<String>>,
    abort: AtomicBool,
}

impl Shared {
    fn cancel(&self, id: &str) {
        self.cancelled.lock().unwrap().insert(id.to_string());
        if self.current.lock().unwrap().as_deref() == Some(id) {
            self.abort.store(true, Ordering::Relaxed);
        }
    }
}

fn main() {
    let dir = engine_dir();
    emit(&Event::Ready {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION"),
    });

    let shared = Arc::new(Shared::default());
    let (tx, rx) = mpsc::channel::<Command>();
    {
        let shared = shared.clone();
        std::thread::spawn(move || work(&dir, rx, &shared));
    }

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Command>(&line) {
            Ok(Command::Cancel { id }) => shared.cancel(&id),
            Ok(Command::Quit) => break,
            Ok(command) => {
                if tx.send(command).is_err() {
                    break;
                }
            }
            Err(e) => emit(&Event::Error {
                id: None,
                message: format!("bad command: {e}"),
            }),
        }
    }
    // Quit, or stdin closed because the app went away: stop whatever is running and leave now.
    // Nothing the worker could still produce has anyone to read it.
    shared.abort.store(true, Ordering::Relaxed);
    std::process::exit(0);
}

/// `--engine-dir <dir>`, else the directory holding this executable.
fn engine_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--engine-dir" {
            if let Some(dir) = args.next() {
                return PathBuf::from(dir);
            }
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

fn work(dir: &Path, rx: mpsc::Receiver<Command>, shared: &Shared) {
    let mut engine: Option<engine::Engine> = None;
    for command in rx {
        match command {
            Command::Load {
                model,
                gpu,
                threads,
            } => {
                // Free the old model first: two at once is a second gigabyte for no reason.
                engine = None;
                let began = Instant::now();
                match engine::Engine::load(dir, Path::new(&model), gpu, threads) {
                    Ok(e) => {
                        emit(&Event::Loaded {
                            ms: began.elapsed().as_millis() as u64,
                            engine: e.version(),
                        });
                        engine = Some(e);
                    }
                    Err(message) => emit(&Event::Error { id: None, message }),
                }
            }
            Command::Transcribe { id, wav, from } => {
                *shared.current.lock().unwrap() = Some(id.clone());
                shared.abort.store(false, Ordering::Relaxed);
                // Checked after `current` is set, so a cancel can't slip between the two.
                let event = if shared.cancelled.lock().unwrap().contains(&id) {
                    Event::Cancelled { id: id.clone() }
                } else {
                    match engine.as_mut() {
                        Some(e) => transcribe(e, &id, Path::new(&wav), from, shared),
                        None => Event::Error {
                            id: Some(id.clone()),
                            message: "no model loaded".into(),
                        },
                    }
                };
                emit(&event);
                *shared.current.lock().unwrap() = None;
                shared.cancelled.lock().unwrap().remove(&id);
            }
            // Handled on the reader thread; never sent here.
            Command::Cancel { .. } | Command::Quit => {}
        }
    }
}

/// Hear `wav` window by window, reporting each as it finishes. Returns the closing event.
fn transcribe(
    engine: &mut engine::Engine,
    id: &str,
    wav: &Path,
    from: f64,
    shared: &Shared,
) -> Event {
    let began = Instant::now();
    let fail = |message: String| Event::Error {
        id: Some(id.to_string()),
        message,
    };
    let samples = match std::fs::read(wav)
        .map_err(|e| e.to_string())
        .and_then(|b| wav::read(&b))
    {
        Ok(s) => s,
        Err(e) => return fail(format!("{}: {e}", wav.display())),
    };

    let windows = window::plan(samples.len(), window::SIZE, window::OVERLAP);
    let order = window::order_from(&windows, from);
    for (done, &k) in order.iter().enumerate() {
        if shared.abort.load(Ordering::Relaxed) {
            return Event::Cancelled { id: id.to_string() };
        }
        let w = windows[k];
        let heard = match engine.hear(&samples[w.start..w.end], &shared.abort) {
            Ok(heard) => heard,
            Err(_) if shared.abort.load(Ordering::Relaxed) => {
                return Event::Cancelled { id: id.to_string() }
            }
            Err(e) => return fail(e),
        };
        let words = window::place(&windows, k, heard)
            .into_iter()
            .map(|h| Word {
                text: h.text,
                start: h.start,
                end: h.end,
                p: h.p,
            })
            .collect();
        emit(&Event::Words {
            id: id.to_string(),
            start: w.start_seconds(),
            end: w.end as f64 / window::RATE as f64,
            done: done + 1,
            of: order.len(),
            words,
        });
    }
    Event::Done {
        id: id.to_string(),
        ms: began.elapsed().as_millis() as u64,
    }
}

/// Write one event as one line. Locked and flushed per event, so lines never interleave and the
/// app sees each as soon as it exists.
fn emit(event: &Event) {
    let line = serde_json::to_string(event).unwrap_or_default();
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
