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

- **Four policies, not a checkbox.** The axes are *when do we download* and *when do we
  install*, which a boolean cannot express. Settings → "When a new version is available":
  `afterSong` (default), `instant`, `manualInstall` (download, then the first click
  schedules it for the end of the song and a second means now), `notifyOnly` (no download).
- **A separate check schedule:** never / 30 min / 4 h / 24 h / daily at a wall-clock time.
  Daily is a clock time rather than an interval so the restart lands at a predictable hour.
- Both live in `app_config_dir/settings.json`, NOT `localStorage`, because the update loop
  reads them from Rust long before (and sometimes without) the UI being involved.
- **State model:** *known* → *staged* → *armed*. The policy decides how far a new version
  travels on its own; the badge walks the rest, one click per step. `armed` is deliberately
  separate from `staged` — `manualInstall` downloads ahead of time but waits to be asked.
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
8. [x] **VERIFIED END-TO-END 2026-08-10** — v1.3.2 updated itself to v1.3.3 with no user
   interaction at all. See the evidence below.

## ✅ Verified in the wild (2026-08-10)

A running v1.3.2 took v1.3.3 entirely on its own, observed by polling the process every
20s from outside the app:

    [17:28:10] 1.3.2 pid=22220 started=16:43:15
    [17:45:12] NOT RUNNING
    [17:45:32] 1.3.3 pid=15300 started=17:45:12

What the timing establishes, beyond "it updated":

- v1.3.2 launched 16:43 and checks every 30 min, so its checks fall at ~16:43 / ~17:13 /
  ~17:43. v1.3.3 was published ~17:27, so the **17:43 check** is the one that found it.
- The install landed at **17:45** — roughly two minutes later. That is the important
  number: `MAX_WAIT` is six minutes, so the backstop cannot have fired. The only other
  trigger is `Event::TrackEnded`, which means **the track-boundary path is what fired**.
  A mid-song install would have shown up as a restart within seconds of the check.
- Downtime was under the 20s poll granularity — the replacement process reports
  `started=17:45:12`, the same second the old one was gone. NSIS quiet install plus
  relaunch is fast; the earlier 7–17s estimate was pessimistic.
- The app came back healthy (responding, 40 threads) and resumed its station.

No prompt, no click, no dialog. Exactly the intended behaviour.

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

## 2026-08-11 — reversal: paused is the *best* moment, not a hold

**Reported symptom:** *"Automatic update should happen silently and automatically if the
music is currently paused. Right now, it waits until I hit play and then some race condition
or something makes the update happen right after resuming the track."*

**It was not a race.** It was two deliberate rules combining into the worst possible
outcome, and it reproduces every time:

1. `decide()` held on `!playing`, so a staged update sat there for the whole pause.
2. The backstop measured `staged_at.elapsed()` — **wall time** — while its gate
   (`if playing(&app).await`) only opened once playback resumed.

So an update staged during a pause longer than `MAX_WAIT` (6 min) arrived at the moment the
listener pressed play with its backstop *already expired*. The next loop tick — at most
`TICK` = 20 s later — fired "backstop (no track boundary)" and cut the just-resumed song in
half. The longer you left it paused, the more certain the interruption. `Hold::Paused` was
never reachable in production either: all four `try_install` call sites hardcoded
`playing: true`.

### What changed

- **Paused installs, on purpose.** After `PAUSE_SETTLE` (30 s) of continuous pause an armed
  update installs itself. The settle window is so that pausing to answer a question does not
  restart the app; worst-case latency is `TICK + PAUSE_SETTLE` ≈ 50 s.
- **The app comes back paused.** This is what made the old guard necessary and is the whole
  cost of the reversal. `updates::arm_resume_paused` writes a marker file next to
  `last-station.json`; `native::attach` consumes it (delete-on-read) and calls
  `Engine::begin_paused()` before `play_station`, so `advance()` issues `play_paused`
  instead of `play`.
- **Starting paused opens no audio device at all.** `AudioThread`'s build step is
  `if !paused && player.is_none()`, so a `Play { paused: true }` records the track and stops.
  Pressing play runs the same rebuild path a long pause already used. There is no window in
  which a frame could be emitted, so no blip — which is why this is a new `Command` field
  rather than `play()` followed by `set_paused(true)` (those are two messages, and the
  build step runs between them if the drain loop happens to split them).
- **The backstop counts playing time only.** `Staged.staged_at`/`waited()` are gone,
  replaced by `Waiting { playing, paused }`, accumulated a `TICK` at a time. `playing` is
  what the 6-minute backstop measures ("no boundary arrived *while playing*"); `paused` is a
  streak, reset on resume, and is what `PAUSE_SETTLE` measures. This alone kills the
  reported symptom, independently of the paused-install feature.
- **`decide` enforces the rule instead of trusting callers.** New field `may_interrupt`
  (true at a track boundary, at the backstop, and on an explicit request; false on the
  paused tick), new `Hold::MidSong` replacing the dead `Hold::Paused`, and it now returns
  `Resume::Playing | Resume::Paused` — which is what decides whether the marker is written.
  Callers pass the *polled* `playing` value rather than `true`; only `on_track_boundary`
  still asserts it, and there it is justified (a track just ended).
- **`force` no longer overrides `playing`.** It sets `armed`, clears `remote` and grants
  `may_interrupt`. So clicking the badge while paused installs *and comes back paused*,
  which is what the click meant.
- **Last-moment re-check.** The 250 ms UI-paint sleep before `install()` is followed by a
  best-effort re-read of the engine's `paused` flag. If it disagrees with what `decide` saw,
  the install is abandoned and everything (staged bytes, `firing`, the marker) is put back.
  This is the only genuinely racy window left, and it is now covered.

### Caught in review, after the first draft worked

An adversarial pass over the diff found two bugs that were *the same bug as the original*,
wearing different gates. Both are fixed; both are worth remembering as a pattern.

1. **A countdown must never run behind a shut gate.** The first draft reset the clocks only
   when nothing was staged, so `Waiting::playing` kept accumulating while the install was
   held for `NotArmed`, `Exporting` or `Remote`. Under `manualInstall` an evening's
   listening banked the whole six minutes; the click that armed it — whose own tooltip says
   *"install after this song"* — then restarted the app mid-song within one tick. Same shape
   when a long export finished. Fixed by gating both clocks on
   `decide(conditions(playing, may_interrupt: true)).is_ok()`, i.e. "everything except the
   moment is satisfied", which is derived from the real rules rather than restated.
2. **A one-shot flag must not outlive the call that set it.** `begin_paused()` set a sticky
   flag consumed by *whatever advance happened next*. But `advance()` has four early returns
   before the consumption point, and a stream violation seconds after the installer ran is a
   thoroughly ordinary way to hit one. The flag then survived to be eaten by the advance the
   listener asked for — clicking **Take Over**, or picking a station — handing them a loaded
   track and silence with nothing on screen to explain it. Fixed by replacing it with
   `Engine::play_station_paused`, which sets and clears around its own await, so the intent
   cannot escape.
3. **The marker is self-validating, not merely time-limited.** `install()` exits this process
   as soon as NSIS launches, which is not the same as the install succeeding — a cancelled
   UAC prompt leaves the old binary and a live marker. Relaunching by hand (the natural
   reaction) would then give a silent app. The marker now holds the version it was written
   for, and a launch ignores it unless it names the running version. The 10-minute TTL is
   now genuinely belt-and-braces.
4. **Existence comes from `metadata`, not from `remove_file` succeeding.** If the file was
   momentarily locked, the old code reported "not there" *and left it on disk*, so the next
   launch inside the TTL consumed it.

Two smaller notes from the same pass, both left alone deliberately: `PAUSE_SETTLE` is
sampled at `TICK` granularity rather than measured (the last-moment re-check covers the case
that matters, and the doc now says so); and `app://update-failed` is clobbered by the
`publish` that follows it before its 2.5 s timeout — pre-existing, in a file another session
is editing, and harmless to the stand-down path.

### ⚠️ Verification status — what is NOT proven (as of 2026-08-11)

Shipped deliberately without a live test; the user was offered one and declined for now.
What has actually been run: `cargo test --lib` (62 pass, 16 of them `updates::`),
`cargo check --release` (mandatory here — the loop's call site is `cfg`'d out in debug), and
`bun run build`. That covers `decide()` and `Waiting`, which is where a mistake means
restarting at a bad moment.

**Unexercised at runtime, in rough order of risk:**

1. **The paused restart end-to-end.** Nothing has ever written the marker, restarted, and
   come back paused. Testable *without* a new release by planting the marker by hand:
   write the running version into `%APPDATA%\…\resume-paused` and launch. Expect: a track
   loaded, play icon showing, no audio device opened, and pressing play starts it. Note this
   still calls `getPlaylist` and so claims the account's single stream — do not run it while
   listening on another device.
2. **`play_paused` emitting no audio.** Reasoned from the audio thread's build gate
   (`if !paused && player.is_none()`), never observed.
3. **The paused install itself.** Release-only *and* needs a newer release to exist, so as
   always the version that adds this cannot demonstrate it — the first real proof is the
   vX → vX+1 update after this ships.
4. **The stand-down path** (playback changing inside the 250 ms paint window) and the
   version-mismatch marker rejection. Both are error paths with no test.

### v1.4.1 — "can't tell" is not "paused"

Reported as *"does the restart after update always start paused? that seems like a bug."*
Not always — a track-boundary install passes `playing: true`, so it comes back playing, and
`arm_resume_paused` is called in exactly one place. But the instinct was right, because
**how "paused" was decided was wrong**:

```rust
async fn playing(app) -> bool {
    match app.state::<NativeEngine>().engine().await {
        Ok(engine) => !engine.is_paused(),
        Err(_) => false,          // "not signed in" silently became "paused"
    }
}
```

`engine()` errors whenever the engine is absent — signed out, or the window before login
finishes at launch. That arrived at `decide` as `playing: false`, indistinguishable from a
deliberate pause, so the loop banked `PAUSE_SETTLE` against a listener who had never touched
the play button, installed, and armed the silent restart. A network hiccup at launch meant
the *next* successful launch came up silent with nothing to explain it. `paused_now` — the
last-moment re-check that exists for exactly this — returns `None` in the same situation, so
`is_some_and` is false and it could not intervene.

It also **compounded**: an app that comes back paused genuinely *is* paused, so every later
update legitimately came back paused too. That is why it looked like "always".

Fixed by giving the answer three states instead of two — `Playback::{Playing, Paused,
Unknown}` — where only an observed `Paused` may arm a silent restart. `Unknown` holds
(`Hold::Unknown`) rather than installing on its own; an explicit click still goes through and
comes back **playing**, because being signed out is not somebody asking for silence. Neither
wait clock advances while blind, and it holds rather than resets so a sign-out does not
discard a legitimate wait already under way.

Two tests pin it: `not_knowing_is_never_mistaken_for_a_pause`, and
`only_a_real_pause_arms_a_silent_restart`, which loops every state so a fourth one forces a
decision here rather than defaulting to silence.

**The general lesson, third time in this file:** a two-valued answer to a three-valued
question puts the missing case somewhere, and it is never where you want it. Same shape as
the wall-clock backstop and the countdown behind a shut gate.

### Things this deliberately does not do

- It does not persist playback *position*. Coming back paused at the top of a fresh track is
  accepted; the mid-track resume idea is still the Deferred section below.
- It does not skip `getPlaylist` on a paused start, so a paused relaunch still claims the
  account's single stream exactly as before.

## 2026-08-15 — the update that waited forever on the login card

**Reported symptom:** *"i'm sitting at the login screen, nothing playing, i click the
v1.4.2 and it checks for updates and then shows 'updating to v1.4.3 while paused' and then
nothing happens until i click it again. is that intended? it's not playing music. get the
update over with!"*

Not intended. Two separate defects, both of which are the *same mistake in a new place* —
treating "we cannot tell what playback is doing" as if it were "paused". That is precisely
the conflation the 2026-08-11 → v1.4.1 work fixed in `decide()`; it survived in two spots
that fix never reached.

### 1. The background loop had no `Unknown` arm — so the install never fired

`Playback::Unknown` means "no engine to ask". The v1.4.1 fix correctly stopped that from
arming a *silent* restart, and `Waiting::tick` correctly stopped either clock advancing on a
guess. But the loop's `match` then read:

```rust
// Unknown never triggers an install on its own. Waiting costs nothing: the
// boundary path picks it up as soon as anything is playing, and the paused
// path as soon as playback is genuinely stopped.
_ => {}
```

**"Waiting costs nothing" assumed `Unknown` was transient.** On the login card it is the
steady state: no engine exists until somebody types a password, so playback never becomes
`Paused` or `Playing`, neither clock ever advances, and no arm ever matches. The update sat
staged, armed, and announced, forever.

**Fix:** a third clock, `Waiting::unknown`, and an arm that installs after `UNKNOWN_SETTLE`
(60 s) with `may_interrupt: true` — so `decide` yields `Resume::Playing`, never the silent
restart. A third clock rather than a change to the other two: `playing`/`paused` must keep
*holding* across a blind spot, which is a different question from *measuring* it.

### 2. The click that staged an update could not also install it

`update_action` reads `staged`/`armed` **at entry**. On the first click both are false, so
the `if staged && armed → "means now"` branch cannot run — yet by the time that same call
returns, `stage()` has downloaded *and* armed it (under `afterSong`). Hence "nothing happens
until i click it again": the second click enters with `staged && armed` already true.

**Fix:** after staging, re-read playback; if it is `Unknown`, install. Deliberately only
that case — `Playing` must still wait for the boundary the badge promises, and `Paused`
belongs to `PAUSE_SETTLE` and its silent restart, not to a click.

### 3. The badge said "while paused" while signed out, which was a lie

`main.ts` initialises `lastPlayhead = { …, paused: true, … }` and **no playhead events
arrive while signed out**, so the badge read a default as fact. It now models the same three
states the updater does, with the login card's visibility standing in for "no engine" — read
off the card rather than kept in a parallel flag, since `showLogin`/`onNowPlaying` already
move it in lockstep. Signed out it reads `updating to vX shortly`.

### Things not to do here

- **Don't shorten `UNKNOWN_SETTLE` toward zero.** Every launch spends a few seconds blind
  while sign-in finishes; restarting there would interrupt the one thing the listener is
  waiting for. It must stay comfortably longer than `TICK`.
- **Don't make the click path install on `Playing`.** The tooltip promises "after this
  song", and `force: true` would waive exactly the guard that keeps that promise.
- **Don't let `unknown` time leak into `paused`.** That reintroduces the v1.4.1 silent
  restart. There is a test named for it.

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
- The backstop timer **only counts time spent playing** — not wall time. Counting wall time
  meant an update staged during a long pause fired the instant the listener pressed play;
  see the 2026-08-11 section above.
- **Paused is the preferred moment**, not a hold: after 30 s of continuous pause the update
  installs and the app comes back paused. Superseded the original "never while paused" rule.
- **Signed out installs too**, after `UNKNOWN_SETTLE` (60 s). Added 2026-08-15 — see below.

## Guards (must not restart at a bad moment)

- **Never while unarmed** — which is how `manualInstall` and `notifyOnly` hold. Under
  `notifyOnly` the background loop does not download at all. An explicit click waives it:
  a click is a request, not automation.
- **Never while an export is running.** `ExportCtl.running` already exists; check it.
- ~~**Never while paused.**~~ **REVERSED 2026-08-11.** The reasoning was sound — a restart
  came back *playing* and started music at someone who deliberately stopped it — but the fix
  was to make the app come back *paused*, not to refuse to update. See the 2026-08-11
  section above.
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
