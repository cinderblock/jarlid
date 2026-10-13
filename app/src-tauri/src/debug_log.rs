//! An opt-in debug log: everything the app writes to stderr, timestamped, in a file.
//!
//! A release build has no console, so every `eprintln!` in every crate — the audio thread's
//! "device underruns are recurring", a stall being recovered, a panic — goes nowhere, and "is it
//! in the logs?" after a glitch has no answer. Off by default, because nobody needs a file growing
//! on their disk to listen to music; switched on from Settings when something needs explaining.
//!
//! **Nothing that logs has to know about this.** Rust's stderr looks the handle up with
//! `GetStdHandle` on every write rather than caching it, so pointing `STD_ERROR_HANDLE` at a
//! pipe captures every existing line in every crate, panics included. A thread on the far end of
//! the pipe stamps each line with the local time and appends it to the file, and in a dev build
//! also copies it to the console stderr used to be, so turning this on never hides anything.
//!
//! The pipe is made once and never closed. A writer can be part-way through a line on another
//! thread at any moment, and closing the handle under it would fail the write — and
//! `eprintln!` panics when a write fails. Turning the log off points stderr back where it was
//! instead, and leaves the pipe idle.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

/// The file, in the app's log directory (`%LOCALAPPDATA%\com.camer.pandora-desktop\logs`).
const FILE: &str = "jarlid.log";
/// Where the file goes when it reaches [`MAX_BYTES`], replacing the previous one. Two files'
/// worth is the most this can ever use.
const OLD_FILE: &str = "jarlid.old.log";
/// Days of ordinary listening, and minutes of a machine underrunning on every poll.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

pub fn dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_log_dir().ok()
}

/// Start or stop logging to the file. Safe to call repeatedly with the same answer.
pub fn apply(app: &AppHandle, enabled: bool) {
    let Some(dir) = dir(app) else { return };
    imp::apply(dir, enabled, &app.package_info().version.to_string());
}

/// Where the log is, so Settings can say so rather than leave it to be hunted for.
#[tauri::command]
pub fn debug_log_path(app: AppHandle) -> Option<String> {
    dir(&app).map(|d| d.join(FILE).display().to_string())
}

/// Show the log in Explorer: the file selected if there is one yet, otherwise its folder.
#[tauri::command]
pub fn debug_log_reveal(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let dir = dir(&app).ok_or("no log directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(FILE);
    let opener = app.opener();
    if file.exists() {
        opener.reveal_item_in_dir(file)
    } else {
        opener.open_path(dir.display().to_string(), None::<&str>)
    }
    .map_err(|e| e.to_string())
}

/// `2026-10-13 14:46:03.127`, in local time: this is read next to "it glitched at about quarter
/// to three", not next to another machine's log.
fn timestamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!(
        "{}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond()
    )
}

/// Append one stamped line, moving the file aside first if it has reached its cap.
///
/// Opens the file for every line rather than holding it: a debug log is a few lines a minute,
/// and not holding it means it can be deleted, renamed or opened in an editor at any time
/// without this side noticing or caring.
fn append(dir: &std::path::Path, line: &str) {
    use std::io::Write;
    let path = dir.join(FILE);
    if std::fs::metadata(&path)
        .map(|m| m.len() >= MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join(OLD_FILE));
    }
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{} {line}", timestamp());
    }
}

#[cfg(windows)]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::mem::ManuallyDrop;
    use std::os::windows::io::FromRawHandle;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};
    use windows::Win32::System::Pipes::CreatePipe;

    /// Handles as integers, because `HANDLE` is a raw pointer and so is neither `Send` nor
    /// `Sync`. Both live for the rest of the process.
    struct Pipe {
        write: isize,
        /// Whatever stderr was before: a console in a dev build, nothing at all in a release.
        /// Restored when the log is turned off, and the console copy is written to it.
        original: Option<isize>,
    }

    static PIPE: OnceLock<Option<Pipe>> = OnceLock::new();
    /// Where the reader writes. Settable after the thread has started, since the directory is
    /// only known once there is an `AppHandle`, and re-applying must not start a second reader.
    static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
    static ON: Mutex<bool> = Mutex::new(false);

    pub fn apply(dir: PathBuf, enabled: bool, version: &str) {
        *DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
        let mut on = ON.lock().unwrap_or_else(|e| e.into_inner());
        if enabled == *on {
            return;
        }
        let Some(pipe) = PIPE.get_or_init(open) else {
            return;
        };
        if enabled {
            // SAFETY: the write end stays open for the life of the process; see the module docs.
            if unsafe { SetStdHandle(STD_ERROR_HANDLE, HANDLE(pipe.write as _)) }.is_ok() {
                *on = true;
                eprintln!("--- debug log on: Jarlid {version} ---");
            }
        } else {
            eprintln!("--- debug log off ---");
            let original = HANDLE(pipe.original.unwrap_or(0) as _);
            // SAFETY: `original` is the handle the process started with, or null, which Rust's
            // stderr treats as "discard" — exactly how a release build behaves without this.
            let _ = unsafe { SetStdHandle(STD_ERROR_HANDLE, original) };
            *on = false;
        }
    }

    /// Make the pipe and start its reader. `None` if Windows would not give us a pipe, in which
    /// case the switch does nothing rather than anything worse.
    fn open() -> Option<Pipe> {
        // SAFETY: plain Win32 calls with out-pointers to locals.
        let original = unsafe { GetStdHandle(STD_ERROR_HANDLE) }
            .ok()
            .map(|h| h.0 as isize);
        let mut read = HANDLE::default();
        let mut write = HANDLE::default();
        // 64 KiB: room for a burst of lines while the reader is busy with a file, so a writer
        // (the audio thread among them) never has to wait on the disk.
        unsafe { CreatePipe(&mut read, &mut write, None, 64 * 1024) }.ok()?;
        let read = read.0 as isize;

        std::thread::Builder::new()
            .name("debug-log".into())
            .spawn(move || {
                // SAFETY: the read end is ours alone, and is never closed.
                let pipe = unsafe { std::fs::File::from_raw_handle(read as _) };
                // Not ours to close: it belongs to whoever gave the process its stderr.
                let mut console = original
                    .map(|h| ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(h as _) }));
                let mut reader = BufReader::new(pipe);
                let mut buf = Vec::new();
                // Must never stop reading, whatever happens to the file: a full pipe would
                // block every thread that logs, the audio thread included.
                loop {
                    buf.clear();
                    match reader.read_until(b'\n', &mut buf) {
                        Ok(n) if n > 0 => {}
                        // End of file cannot happen while the write end is open, and it always
                        // is; an error is not expected either. Either way, don't spin on it.
                        _ => {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            continue;
                        }
                    }
                    if let Some(console) = console.as_mut() {
                        let _ = console.write_all(&buf);
                    }
                    let line = String::from_utf8_lossy(&buf);
                    let dir = DIR.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let Some(dir) = dir {
                        super::append(&dir, line.trim_end_matches(['\r', '\n']));
                    }
                }
            })
            .ok()?;

        Some(Pipe {
            write: write.0 as isize,
            original,
        })
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn apply(_dir: std::path::PathBuf, _enabled: bool, _version: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jarlid-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The whole mechanism: a write to the process's stderr, from code that knows nothing
    /// about logging, arrives in the file with a timestamp in front of it.
    ///
    /// Written with `writeln!(stderr())` rather than `eprintln!`, because the test harness
    /// captures the macro in test threads, and outside a test nothing does.
    #[cfg(windows)]
    #[test]
    fn stderr_lands_in_the_file() {
        use std::io::Write;
        let dir = scratch("debug-log");
        imp::apply(dir.clone(), true, "test");
        writeln!(std::io::stderr(), "a line from somewhere else").unwrap();

        let path = dir.join(FILE);
        let mut text = String::new();
        for _ in 0..100 {
            text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.contains("a line from somewhere else") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        imp::apply(dir.clone(), false, "test");

        let line = text
            .lines()
            .find(|l| l.ends_with("a line from somewhere else"))
            .unwrap_or_else(|| panic!("not in the log: {text:?}"));
        // `2026-10-13 14:46:03.127 a line…`: a date, a time to the millisecond, then the line.
        let stamp = &line[..23];
        assert!(
            stamp.as_bytes()[4] == b'-'
                && stamp.as_bytes()[10] == b' '
                && stamp.as_bytes()[19] == b'.',
            "no timestamp on {line:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_file_is_moved_aside_rather_than_grown() {
        let dir = scratch("debug-log-rotate");
        let full = std::fs::File::create(dir.join(FILE)).unwrap();
        full.set_len(MAX_BYTES).unwrap();
        drop(full);

        append(&dir, "the first line of a fresh file");

        assert_eq!(
            std::fs::metadata(dir.join(OLD_FILE)).unwrap().len(),
            MAX_BYTES
        );
        let fresh = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(
            fresh.ends_with("the first line of a fresh file\n"),
            "{fresh:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
