//! Running `jarlid-autosync.exe` and talking to it.
//!
//! One process, started on the first job and stopped after a spell of idleness, since the
//! loaded model holds about a gigabyte. It runs at below-normal priority with no console window:
//! the decode thread feeding the audio device must always win the CPU over it
//! (`plans/audio-dropouts-under-load.md`).
//!
//! The protocol is `crates/autosync-sidecar/src/protocol.rs`. Each job's events are routed to
//! its own channel by id, so several jobs can be queued with the sidecar at once and each caller
//! sees only its own.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use autosync::Heard;
use serde::Deserialize;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use super::component::{Installed, PROTOCOL};

/// How long to wait for the model to load before giving up on the sidecar.
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// What a job reports.
#[derive(Debug)]
pub enum JobEvent {
    /// One window's words, on the track's clock. `done` of `of` windows are finished.
    Words {
        done: usize,
        of: usize,
        words: Vec<Heard>,
    },
    Done,
    Cancelled,
    Error(String),
}

#[derive(Deserialize)]
#[serde(tag = "ev", rename_all = "camelCase")]
enum Event {
    Ready {
        protocol: u32,
    },
    Loaded {
        ms: u64,
        engine: String,
    },
    Words {
        id: String,
        done: usize,
        of: usize,
        words: Vec<Word>,
    },
    Done {
        id: String,
    },
    Cancelled {
        id: String,
    },
    Error {
        id: Option<String>,
        message: String,
    },
}

#[derive(Deserialize)]
struct Word {
    text: String,
    start: f64,
    end: f64,
    p: f32,
}

type Routes = Arc<Mutex<HashMap<String, UnboundedSender<JobEvent>>>>;

pub struct Sidecar {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    routes: Routes,
    alive: Arc<AtomicBool>,
}

impl Sidecar {
    /// Start the sidecar and load the model. Blocking: call from a blocking thread.
    pub fn start(installed: &Installed, gpu: bool, threads: u32) -> Result<Self, String> {
        installed.verify_code()?;

        let mut command = Command::new(installed.exe());
        command
            .arg("--engine-dir")
            .arg(&installed.engine_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("starting the speech engine: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;

        // The engine's own log, kept with the app's (and so in the debug log when that is on).
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    eprintln!("[autosync engine] {line}");
                }
            }
        });

        let routes: Routes = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let (started_tx, started_rx) = mpsc::channel::<Result<(), String>>();
        {
            let routes = routes.clone();
            let alive = alive.clone();
            std::thread::spawn(move || read(stdout, routes, alive, started_tx));
        }

        let sidecar = Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            routes,
            alive,
        };
        sidecar.send(&serde_json::json!({
            "op": "load",
            "model": installed.model,
            "gpu": gpu,
            "threads": threads,
        }))?;
        match started_rx.recv_timeout(LOAD_TIMEOUT) {
            Ok(Ok(())) => Ok(sidecar),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("the speech engine did not start in time".into()),
        }
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Queue a transcription. Its events arrive on the returned channel, ending with `Done`,
    /// `Cancelled` or `Error`.
    pub fn transcribe(
        &self,
        id: &str,
        wav: &std::path::Path,
        from: f64,
    ) -> Result<UnboundedReceiver<JobEvent>, String> {
        let (tx, rx) = unbounded_channel();
        self.routes.lock().unwrap().insert(id.to_string(), tx);
        let sent = self.send(&serde_json::json!({
            "op": "transcribe",
            "id": id,
            "wav": wav,
            "from": from,
        }));
        if let Err(e) = sent {
            self.routes.lock().unwrap().remove(id);
            return Err(e);
        }
        Ok(rx)
    }

    pub fn cancel(&self, id: &str) {
        let _ = self.send(&serde_json::json!({ "op": "cancel", "id": id }));
    }

    fn send(&self, message: &serde_json::Value) -> Result<(), String> {
        let mut stdin = self.stdin.lock().unwrap();
        writeln!(stdin, "{message}")
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("the speech engine stopped: {e}"))
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        // Closing stdin alone would also end it, but only once its current window finishes.
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Read events until the sidecar exits: the start-up handshake first, then each job's events to
/// its channel. Every job still waiting is told when the process goes away.
fn read(
    stdout: std::process::ChildStdout,
    routes: Routes,
    alive: Arc<AtomicBool>,
    started: mpsc::Sender<Result<(), String>>,
) {
    let mut started = Some(started);
    let mut start = |result: Result<(), String>| {
        if let Some(tx) = started.take() {
            let _ = tx.send(result);
        }
    };
    let route = |id: &str, event: JobEvent, last: bool| {
        let mut routes = routes.lock().unwrap();
        if let Some(tx) = routes.get(id) {
            let _ = tx.send(event);
        }
        if last {
            routes.remove(id);
        }
    };

    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<Event>(&line) else {
            eprintln!("[autosync] unreadable event: {line}");
            continue;
        };
        match event {
            Event::Ready { protocol } if protocol != PROTOCOL => {
                start(Err(format!(
                    "the speech engine speaks protocol {protocol}, this version needs {PROTOCOL}"
                )));
            }
            Event::Ready { .. } => {}
            Event::Loaded { ms, engine } => {
                eprintln!("[autosync] model loaded in {ms} ms (engine {engine})");
                start(Ok(()));
            }
            Event::Words {
                id,
                done,
                of,
                words,
            } => {
                let words = words
                    .into_iter()
                    .map(|w| Heard {
                        text: w.text,
                        start: w.start,
                        end: w.end,
                        p: w.p,
                    })
                    .collect();
                route(&id, JobEvent::Words { done, of, words }, false);
            }
            Event::Done { id } => route(&id, JobEvent::Done, true),
            Event::Cancelled { id } => route(&id, JobEvent::Cancelled, true),
            Event::Error {
                id: Some(id),
                message,
            } => route(&id, JobEvent::Error(message), true),
            Event::Error { id: None, message } => {
                eprintln!("[autosync] {message}");
                start(Err(message));
            }
        }
    }

    alive.store(false, Ordering::Relaxed);
    start(Err("the speech engine exited while starting".into()));
    for (_, tx) in routes.lock().unwrap().drain() {
        let _ = tx.send(JobEvent::Error("the speech engine exited".into()));
    }
}
