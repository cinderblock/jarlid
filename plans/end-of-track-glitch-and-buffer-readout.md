# End-of-track glitch, device-buffer readout, history strip over the text

## Goal

Three reports from the user on 2026-10-13, against the installed v1.12.1:

1. "I just had an audio glitch at the end of the last song. See it in logs? Did our new automatic
   buffer work?"
2. Show the device buffer (30 ms, growing under load; see `plans/audio-dropouts-under-load.md`)
   in the bottom-left technical readout.
3. "Certain aspect ratios of the display show the past albums' art over the text sometimes": the
   recently-played strip (`#history`) drawn over the title/artist/lyrics text.

## Environment / context

- Installed build: `C:\Users\camer\AppData\Local\Jarlid\jarlid.exe`, v1.12.1, running since
  14:22 on 2026-10-13. Settings: blend **off**, song cache **off**, default output device.
- Readout: Rust emits `engine://technical` at 1 Hz from `app/src-tauri/src/native.rs`;
  `renderTechnical` in `app/src/main.ts` draws `#tech`.
- Adaptive buffer: `DeviceBuffer` in `crates/engine/src/audio_thread.rs`; per-player detection
  is `DeviceQueue` in `crates/audio/src/player.rs`.

## Findings

### There are no logs to look at

- The release build is a `windows_subsystem = "windows"` process. Every `eprintln!` (including
  "device underruns are recurring; the next player gets a N ms buffer") goes nowhere. There is no
  log file under `%APPDATA%\com.camer.pandora-desktop` or `%LOCALAPPDATA%`.
- The underrun count and current buffer size live only in memory and only reach a bug report,
  and the Report button only appears on an error toast. So whether the buffer grew during the
  glitch **cannot be determined after the fact**. The readout (item 2) fixes that going forward.
- The bug-report format string has a broken line continuation: `"... starved {:.2}s,              underruns ..."`
  embeds ~14 spaces in the report.

### A real end-of-track defect, made worse by the bigger buffer

`Player::is_finished()` is "decoder finished && ring empty". It ignores the audio already handed
to Windows (`device_audio`), and the audio thread drops the player, and with it the stream, as
soon as it sees `is_finished()`. Dropping a cpal stream stops the WASAPI client immediately, so
whatever is still in the device buffer is discarded mid-waveform.

- At the old ~22 ms default and the new 30 ms, with a 50 ms poll, the tail is often already out.
- Once the buffer has grown to 120 or 240 ms, up to that much of the song's last moment is cut,
  and cut abruptly, which is a click on any track that does not end in digital silence.

This is a plausible cause of "a glitch at the end of the song", specifically on a session where
the buffer had grown. It is not proven to be *the* cause (see above: no evidence survives).

## Plan

- [x] `is_finished()` also waits for the device buffer to play out (`device_audio == 0`), with a
      deadline on the audio thread (`TAIL_GRACE`, buffer + 250 ms after `drained()`) so a device
      that stops calling back cannot hold the track forever. `750f567`.
- [x] Readout: `out 30 ms` after `buf 4.9 s`, `out 30 → 60 ms` while a new size is pending;
      `underruns N` in the fault group when non-zero. `711afc1`.
- [x] Fix the report format string. `9998bce`.
- [x] History strip over the text. `cff76a6`. See the findings below.
- [x] `bun run build`, `cargo test -p audio -p engine`, `cargo check` in `app/src-tauri`. All pass;
      clippy and rustfmt warnings remaining are pre-existing, in lines not touched here.
- [x] README.
- [x] Readout seen rendering in the browser harness (`… buf 14.2 s · out 30 ms`, faked payload).
      Not yet seen against a real engine; the first look at that will be the next build.

### Verification of the tail fix (real device, muted)

`cargo run --example device-buffer -- <ms> 15` on the Dell P2715Q (Intel Display Audio, 48 kHz),
which now times the gap between the last sample being handed over and `is_finished()`:

| asked | tail played out | device held at drain (from the playhead) |
| --- | --- | --- |
| 30 ms | 30.2 ms | — |
| 60 ms | 59.7 ms | 50 ms |
| 120 ms | 121.1 ms | 110 ms |
| 240 ms | 99.8 ms | 90 ms |
| 480 ms | 400.6 ms | 390 ms |

At 240 and 480 ms Windows calls back less often than every 10 ms, so the device is not full at
the moment the ring empties. In every case the playhead and the wall clock agree, which is what
says the model of the device queue is right. Before the fix, whatever of that was still unplayed
when the 50 ms poll noticed the drain was discarded.

### History strip over the text (layout)

- **What was covered:** the `#tech` readout in the bottom-left corner (two-column layout), and
  the "LYRICS" label and first lines (stacked layout, under 820px wide).
- **Cause:** `--stage-reserve` counted only the stack every session has. The source pill (41px,
  permanent once a WiiM is on the LAN), the mode pill (28), the QuickMix "from" line (18) and the
  mini player (70) were not in it. Nothing in the stage can shrink, so the stage grew past 92vh
  and, being centred, spilled off both ends. The old comment's claim that the `min()` on
  `.art-wrap` caught an under-estimate was false: that `min()` limits width, not height.
- **Fix:** each optional row adds its height to the reserve via `:has()` while it is shown;
  remote mode adds -61px (drops the 96px strip, adds the 34px volume slider). Base corrected
  from 272 + 1.3×artist to 273 + 1.35×artist.
- **Measured:** no overlaps at any window 500px tall or more, across ~150 sizes with each row on
  and off. Art at 1000×620 with source + mode pills: 238 → 167, the room those rows take.
- **Left alone:** windows under 500px tall with the source pill (the stack does not fit even with
  no art); the tiny art in the stacked layout on narrow windows (`89vw - 420px` term, already
  noted in `plans/stage-art-room.md`).
- **Fragility to remember:** the per-row heights are hand-measured constants, like the rest of the
  reserve. Adding a row to the stage without a `--row-*` entry brings this bug back.

## Round 2 (user answers, 2026-10-13)

### Decisions (don't re-ask)

- **Logging:** a debug flag that turns logging on; otherwise off. Built as a persisted Settings
  switch (Troubleshooting → Keep a debug log), because a glitch can't be predicted, so the log
  has to survive restarts and auto-updates. A launch flag would not.
- **Art size must not change with the mode** (local vs speaker). `ad0b817` removed the remote-mode
  reserve rule.
- **Pills on one line:** the user asked to see a comparison before deciding. Mockups are in
  `plans/mockups/`.

### Debug log design

- `app/src-tauri/src/debug_log.rs`. Rust's stderr calls `GetStdHandle` on every write (verified
  in `library/std/src/sys/stdio/windows.rs`), so `SetStdHandle(STD_ERROR_HANDLE, pipe)` captures
  every `eprintln!` in every crate, panics included, with no changes at the call sites. A reader
  thread stamps each line with local time to the ms and appends it to
  `%LOCALAPPDATA%\com.camer.pandora-desktop\logs\jarlid.log`. At 8 MB the file is moved to
  `jarlid.old.log`. In a dev build the reader also copies each line to the original console.
- The pipe is never closed. Closing it under a concurrent writer fails the write, and
  `eprintln!` panics on a failed write. Off means restoring the original handle (null in a
  release build, which std treats as a quiet sink).
- The reader never stops reading, because a full pipe would block every thread that logs,
  including the audio thread.
- The engine now logs track start, player open (device, offset, buffer), each underrun batch,
  each starved stretch, and track end (`dda9230`).
- Tested: `stderr_lands_in_the_file` (a plain stderr write lands in the file with a timestamp)
  and `a_full_file_is_moved_aside_rather_than_grown`.

### Pills on one line: what the mockups showed

The source pill (309px with "Living Room · Idle") + 8 + the mode pill (90) needs 407px. The stage
column is 340px at 1000×620 and 300px at 1600×900, so the row wraps and nothing changes. It only
fits from about 1440×900 up, and there it gives the art +28px. Reserving 0 for the mode row when
it *might* wrap brings the overlap bug back, and CSS can't tell whether it wrapped. A no-wrap
variant clips "THIS COMPUT" / "LIVING ROOM", and the mode dropdown overhangs the lyrics.

### Gotcha

- A Python edit turned a Rust `\`-newline string continuation into a run of spaces inside the
  string, which is the same defect `9998bce` fixed. Caught in review and rewritten as one line.
  After scripted edits, grep for long runs of spaces inside string literals.

## Open questions for the user

1. ~~Rolling log file?~~ Answered: an opt-in flag. Built.
2. ~~Art size in remote mode?~~ Answered: don't change it. Done in `ad0b817`.
3. Pills on one line: comparison shown. Recommendation: don't, because at ordinary window sizes
   it either does nothing or clips the speaker's name.

## Things not to do

- Don't bump the version (project rule).
- Don't build a release while listening to judge audio (see `audio-dropouts-under-load.md`).
