# Seamless Updates — Living Plan

Plan path: `plans/seamless-updates.md`

## Goal

An update should land **in the gap between songs**, not through the middle of one.
Download and verify everything ahead of time, then the instant a track ends, install and
restart — so the user experiences a slightly longer pause between songs rather than an
interruption.

User's framing: *"it downloads and does as much as possible, and then the instant a song
ends/fades out, triggers the restart/install, and when the app comes back a new song
starts and it almost feels like nothing happened."*

## Decisions already made (don't re-ask)

- **Fully automatic.** No prompt. Download silently, restart at the next song boundary,
  show a brief "updating…" note as it happens. No opt-out setting for now.
- **Simple version first.** Ship staged-download + fire-at-boundary + quiet install, and
  live with the resulting gap before optimising further.
- **Pre-buffered resume is DEFERRED** (see "Deferred" below) — the user chose to feel the
  simple version's gap first rather than build the optimisation up front.
- **Do NOT assume the "next track" is deterministic.** The user flagged this and it is
  unmeasured; see the open question under "Deferred" before building anything that relies
  on it. There is an alternative design that avoids the question completely.
- **Window close stays immediate.** Clicking X means "go away now"; only app-initiated
  restarts wait for a boundary.

## Mechanics, verified against the crate source (tauri-plugin-updater 2.10.1)

Read from `~/.cargo/registry/.../tauri-plugin-updater-2.10.1/src/updater.rs` rather than
assumed, because the whole design depends on it:

- `Update::download(on_chunk, on_finish) -> Vec<u8>` and `Update::install(bytes)` are
  **separate** (`updater.rs:652` and `:718`). `download_and_install` is just the two
  chained. So the download can happen long before the install.
- **Signature verification happens inside `download`** (`verify_signature` at the end of
  the download body), so staged bytes are already verified — the install trigger cannot
  fail signature checks at the last moment.
- On Windows, `install` builds NSIS args `/UPDATE /ARGS <current exe args>`, launches the
  installer with `ShellExecuteW`, then calls `std::process::exit(0)` (`updater.rs:865`).
  The `/UPDATE` flag is what makes the installer **relaunch the app** afterwards.
- There is an `on_before_exit` hook (`updater.rs:288`) that runs immediately before the
  installer launch + exit. That is the right place to stop audio cleanly.
- `installMode` is **not set** in `tauri.conf.json`, so it defaults to `passive` — which
  shows an NSIS progress dialog. `quiet` is silent. Jarlid installs per-user
  (`AppData\Local\Jarlid`), so `quiet` needs no elevation.

## Where the time goes

| stage | cost | on critical path? |
|---|---|---|
| download | seconds–minutes | **no** — staged in advance |
| NSIS quiet install | ~3–8s | yes |
| process start + WebView2 | ~1–3s | yes |
| tuner login | ~1–2s | yes (removable — see Deferred) |
| `getPlaylist` | ~1–2s | yes (removable — see Deferred) |
| buffer to first sound | ~1–2s | yes |

Realistic gap for the simple version: **~7–17s**. "A longer gap between songs" is
achievable; "nothing happened at all" is not, because the exe is swapped and the process
genuinely restarts. Setting that expectation honestly matters more than overselling it.

## Plan / steps

1. [x] `installMode: "quiet"` — **note the key**: it is `plugins.updater.windows.installMode`,
   NOT `bundle.windows.nsis.installMode`. The latter is install *scope*
   (`currentUser`/`perMachine`/`both`) and setting `quiet` there is a hard config error:
   `unknown variant 'quiet'`.
2. [x] Stage the download in the background (`updates::stage`), holding bytes **and the
   `Update` handle** so firing needs no network at all.
3. [x] Fire on `Event::TrackEnded` (`native.rs` → `updates::on_track_boundary`).
4. [x] Backstop while playing, `MAX_WAIT` = 6 min.
5. [x] Guards as a pure `decide()` function with unit tests.
6. [x] UI: `app://update-staged` sets the badge; `app://update-installing` paints
   "updating to vX…" as the last thing before the process exits; clicking the badge only
   skips the wait (`install_staged` reuses the staged bytes).
7. [x] `on_before_exit` stops the audio device before the handover. The hook lives on
   `UpdaterBuilder` and is copied into the resulting `Update`, so it must be attached in
   `stage()` — before the handle is staged — not at install time. Two wrinkles worth
   remembering: the hook is a plain `Fn()` with nowhere to await, so reaching the engine
   uses `NativeEngine::try_engine` (`tokio::Mutex::try_lock`, skipping the stop rather than
   blocking the handover if momentarily contended); and this matters more than it first
   appears, because `advance()` has already started the *next* track by the time
   `TrackEnded` reaches us — so there really is audio playing at the moment we install.
8. [ ] Verify end-to-end — see the note below on why that is awkward.

## ⚠️ How this can (and cannot) be verified

The whole flow is **release-only** — `updates::spawn` is behind `#[cfg(not(debug_assertions))]`
precisely so a dev build can never download a release over itself. So:

- A debug build exercises none of it. `cargo check --release` is mandatory after touching
  this file, since debug does not compile the call site that ships.
- Observing a real seamless update needs a *newer* release to exist than the one running.
  **The version that adds this feature cannot demonstrate it.** Shipping it as vX means the
  vX → vX+1 update is the first one that can be seamless; the update *into* vX still uses
  the old abrupt path.
- What is tested now: `decide()` — the guard logic, where a mistake means restarting at a
  bad moment — via 7 unit tests covering each hold and their precedence.

## The waiting policy (user's rule)

> *"If we have to, interrupting and resuming a running song is OK. But always prefer
> waiting a couple of minutes to the end of the song, to avoid abrupt audio
> interruptions."*

So the track boundary is the **preference**, and interrupting is a bounded **fallback** —
not a design choice to agonise over. Concretely:

- Armed and playing → wait for `Event::TrackEnded`, then install. This is the normal path
  and costs at most one track (typically 2–5 minutes).
- **Backstop:** if no boundary arrives within `MAX_WAIT` (6 minutes — longer than almost
  any track, so it only fires when something is wrong, e.g. a position that has stopped
  advancing), install anyway. Interrupting is explicitly acceptable rather than waiting
  forever.
- The backstop timer **only runs while playing**. A paused app is never interrupted and
  never restarted out from under the listener — it simply updates the next time it is
  playing, or on the next launch check.

## Guards (must not restart at a bad moment)

- **Never while an export is running.** `ExportCtl.running` already exists; check it.
- **Never while paused.** Nothing is being interrupted, but a restart would come back
  *playing* and start music at someone who deliberately stopped it. Trade-off accepted: an
  app left paused indefinitely does not auto-update; the startup check catches it later.
- **Never in remote mode** — the WiiM owns playback, there is no local track boundary to
  ride, and restarting would just drop the display.
- Only fire once; disarm after triggering so a failed install cannot loop.

## Deferred (not now, by user's choice)

**Pre-buffered resume**, which would cut login and `getPlaylist` off the critical path and
bring the gap to roughly 5s: persist a track and its signed audio URL before exiting, then
on launch start decoding it immediately, in parallel with the tuner login. Pandora's audio
URLs are signed and expiring rather than auth-gated, so this works without a session; an
expired URL falls back to the normal path.

### ⚠️ OPEN QUESTION: is the "next track" deterministic?

Raised by the user, and it is the right question — the original sketch above assumed we
could persist *the next queued track* and have it be the track the app would have played
anyway. **That assumption is probably false, and nothing here has been measured yet.**

What we know for certain:

- The engine keeps a local `queue: Vec<Track>` refilled from `station.getPlaylist`
  (`crates/engine/src/lib.rs`, `MIN_QUEUED = 2`). That queue is **in memory only** — the
  sole persisted state is `last-station.json`, which holds `{name, token}` and nothing else
  (`app/src-tauri/src/native.rs`). So today a restart discards the queue entirely and asks
  Pandora for a fresh fragment.
- Pandora generates a playlist fragment **per request**. Two calls almost certainly return
  different tracks, which means "the next track" is a property of *our* queue, not a
  property of the station that survives a restart.

What that implies: pre-buffering the persisted next track does **not** reproduce what
would otherwise have played — it *changes* what plays. That is probably fine (it is a
track Pandora legitimately served us), but it is a behaviour choice, not a transparent
optimisation, and it must not be described as the latter.

**Experiments to settle it.** All of these need the app CLOSED first: `getPlaylist` is the
call that trips `STREAM_VIOLATION`, so running them while Jarlid is playing will either
fail or steal the stream and interrupt the listener.

1. **Determinism.** Call `station.getPlaylist` twice in a row on the same station and diff
   the `trackToken`/`songName` lists. Same list ⇒ fragments are stable and the whole
   question dissolves. Different ⇒ confirmed non-deterministic. *Write it as
   `crates/pandora/examples/playlist-determinism.rs`; it is read-only.*
2. **Audio-URL lifetime.** Take a fragment, then `HEAD` the *second* track's audio URL
   immediately, at 1 min, 5 min, 30 min. Establishes how long a staged URL stays usable and
   therefore whether pre-buffering can survive a slow install.
3. **Stale `trackToken` validity.** Does `station.addFeedback` still accept a `trackToken`
   from an abandoned fragment? Decides whether thumbs work on a pre-buffered track. This is
   a **write** — needs the throwaway station (`examples/verify-writes.rs` precedent).
4. **Accounting.** Does Pandora treat an un-played fragment as played? Relevant to skip
   limits and to station learning; hardest to measure, lowest priority.

### The alternative that sidesteps the question entirely

Instead of persisting the *next* track, persist the **currently playing** track and its
position, and on relaunch re-open that same signed URL and seek back. Determinism never
enters into it — it is the same track, the same fragment, the same second.

This is more seamless than the boundary approach *and* the machinery already exists as of
v1.2.0: `Decoder::seek()` (`SetCurrentPosition`, VT_I8 PROPVARIANT in 100 ns units) plus
the stall/pause recovery that already does exactly "re-open the same URL and seek back to
where the listener was", with tests (`seek_skips_ahead`, `engine/examples/pause-resume.rs`).
See `plans/audio-stall-recovery.md`.

It also removes the need to wait for a track boundary at all — the update could fire at any
moment and land back mid-song. A boundary is still a slightly nicer moment (no cut
mid-word), but it stops being a requirement.

**If this works, it is the better design.** Evaluate it before building the next-track
version. The main unknown is the same URL-lifetime question (experiment 2), plus whether
resuming mid-track after a full process restart re-triggers Pandora's "one stream per
account" check awkwardly.

## Things not to do

- Don't call `download_and_install` — it collapses the two phases and puts the download
  back on the critical path.
- Don't defer the window-close path; only app-initiated restarts wait.
- Don't assume an external kill can be deferred. The NSIS installer's own taskkill, a
  crash, or Task Manager all stop audio instantly; this only covers shutdowns the app
  initiates.
