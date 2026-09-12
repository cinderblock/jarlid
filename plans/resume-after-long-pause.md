# Resume after a long pause: expired URLs and the un-pause race

## Goal

Pressing play after a long pause should not skip through several songs, and pressing pause
straight afterwards must leave the app paused. Reported 2026-09-11:

> Hitting play after being paused for a while skips quickly through a couple songs. This even
> causes a small race condition where if I accidentally hit play (after paused for a while) and
> then quickly hit pause, a few seconds later it loads some songs and ends up in a playing state.

## Environment / context

- Engine: `crates/engine/src/lib.rs` (queue, advance, run loop) and
  `crates/engine/src/audio_thread.rs` (the thread that owns the player; watchdog; pause release).
- `crates/` is its own cargo workspace; the Tauri app at `app/src-tauri/` is separate.
- Pandora audio URLs are signed and expire (`Track::audio_url`, "treat as a live credential").
  The lifetime is not known precisely; it is long enough that an ordinary pause survives it and
  short enough that "paused for a while" does not. Nothing in the URL announces it.
- `RELEASE_AFTER_PAUSE` (45 s) tears the player down during a pause; play rebuilds it by
  re-opening the same URL at the remembered position (`plans/audio-stall-recovery.md`).

## Diagnosis (from code; matches the report exactly)

**Skipping through songs.** After a long pause the current track's URL has expired. Play →
`SetPaused(false)` → the build step re-opens the URL → it fails → the thread publishes `failed`
and drops the track. The engine's run loop sees `failed` and calls `advance`, which plays the
next queued track — fetched in the same playlist request, signed at the same moment, equally
dead. It fails too. The queue refills only when it drops below `MIN_QUEUED` (2), so a queue of
four stale tracks produces about three quick failures before a fresh playlist arrives. Each
failure is one HTTP rejection, so the whole cascade takes a second or two: "skips quickly
through a couple songs".

**Ending up playing after a pause.** Every automatic advance sent `Command::Play { paused:
false }`, which unconditionally sets the thread's `paused` to false. Sequence: play → first
open fails → user presses pause (`paused = true`) → engine notices `failed` → `advance` → `Play`
→ `paused = false` → the replacement track starts sounding. The listener's pause was overridden
by an advance nobody asked for.

**A third defect found on the way.** `track_ended` and `failed` were level flags cleared only by
the next `Play`. When `advance` itself fails (no playlist: stream taken by another device, or
the network is down) no `Play` is sent, the flag stays up, and the run loop called `advance`
again on every 200 ms tick. The 10 s `RETRY` timer that is supposed to govern that only ever
applied when nothing had set a flag. During a stream violation this hit Pandora five times a
second.

## Decisions already made (don't re-ask)

- **Failure-driven, not lifetime-driven.** We do not know the URL lifetime, so we do not guess
  one. A URL that will not open retires every queued track fetched with it or before it; tracks
  from a later batch are kept. One batch, one timestamp, taken once per playlist request so
  same-batch tracks compare equal.
- **The current track is lost after expiry.** There is no call that re-signs a URL for a track
  token, so resuming the same song mid-way is impossible once its URL is dead. The listener
  gets the next song, about a second after pressing play. Previously they got it after three
  failed ones.
- **Automatic advances never change the pause state.** Decided on the audio thread, where the
  command queue has already been applied, not by the engine reading the published flag (which
  can lag an in-flight pause by a tick). User actions — skip, take over, pick a station — still
  start playing, as before.
- **Flags clear on read.** `take_track_ended` / `take_failed` are consumed by the run loop, so a
  failed advance is retried on `RETRY`, not on every tick.

## Plan / progress

- [x] Diagnose from code.
- [x] `audio_thread.rs`: `Command::Play` carries a `Start` (`Playing` / `Paused` / `Unchanged`);
      `take_track_ended` / `take_failed` clear on read.
- [x] `lib.rs`: queue entries carry their fetch instant; `retire_batch_of_current` on failure;
      run loop advances with `Start::Unchanged`; `start_paused` one-shot flag removed in favour
      of passing the intent through `advance_as` / `play_station_as`.
- [x] Unit test for the retirement rule (same batch goes, later batch stays).
- [x] `cargo check`, `cargo test`, `cargo clippy` in `crates/`; `rustfmt --check` on touched files.
- [x] Committed as 0f65887; bumped to v1.6.3 in c687270; tag `v1.6.3` pushed 2026-09-12; Release run 34715828811 succeeded.
- [ ] Verify in the real app: pause for long enough that play used to skip, press play, expect
      one new song within a second or two and no skipping; then play → pause quickly, expect
      the app to stay paused. Needs Cameron and a long pause; not reproducible on a timer
      without knowing the URL lifetime.

## Findings / gotchas

- `Instant::now()` per track inside the `extend` closure would stamp same-batch tracks
  nanoseconds apart, and `queued.at > failed_at` would then keep the later ones in the batch.
  Stamp once per request.
- `failed` also fires when a track stalled mid-play and three rebuilds did not bring it back.
  Retiring its batch there costs one playlist request and nothing audible; not special-cased.
- A `TrackStarted` event for a track loaded paused is followed by `Paused(true)` so the event
  stream does not imply playback. For `Start::Unchanged` the engine reads the published flag
  after sending the command; exact unless a pause is still in flight, which the playhead tick
  corrects within a frame. Jarlid's UI ignores `Event::Paused` anyway.

## Open questions for the user

1. What is the actual URL lifetime? Not needed for this fix, but if it turns out to be, say, an
   hour, a proactive "don't even try a URL older than N" would save the one failed open on
   resume. Would need a live measurement: fetch a playlist, wait, try to open. Costs the
   account's single stream slot while it runs.

## Things not to do

- Don't read `is_paused()` in the engine to decide whether an automatic advance should start
  playing. The atomic lags the command queue; the thread must decide.
- Don't make `advance` skip the failed track's batch unconditionally on every call — only the
  run loop's failure path knows the URL was rejected.
