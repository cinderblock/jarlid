# Audio stall recovery (native player)

## Goal

Jarlid goes silent if it sits idle long enough, and **the play button cannot get it back** — only
"next" recovers, at the cost of losing the track you were on. Make the native audio path detect a
stalled or dead stream and recover in place, resuming the same track at the same position.

## Symptom (user, 2026-08-10)

> If the app has been sitting idle enough… the audio is lost. Hitting play doesn't resume. I need
> to hit "next" to recover.

## Diagnosis (from code, not yet reproduced under instrumentation)

Three separate defects compose into exactly this symptom.

1. **Nothing detects a stalled stream.** `crates/audio/src/media_foundation.rs` calls
   `IMFSourceReader::ReadSample` synchronously with no timeout. After a long idle, Pandora's CDN
   drops the TCP connection; a half-open socket makes that call block indefinitely. The decode
   thread never returns, so `decoder_finished` is never set, so `Player::is_finished()` is never
   true, so `AudioThread` never publishes `track_ended` and the engine never advances. The ring
   buffer drains, the callback emits silence, and the position freezes. **Silent forever.**

2. **"Play" cannot start anything.** `Engine::set_paused(false)` only clears an atomic
   (`Player::set_paused`). In `audio_thread.rs` the `SetPaused` arm is `if let Some(player) =
   &player` — with no player it is a no-op that doesn't even update the published `paused` flag.
   There is no path anywhere from "play" to "open a stream". Only `skip`/`replay` build a `Player`,
   which is why "next" is the only recovery.

3. **A single advance failure kills the radio permanently.** `Engine::run()` does
   `if let Err(e) = self.advance().await { …; return; }`. One failed playlist fetch (expired token,
   network blip, `STREAM_VIOLATION`) on an empty queue ends the auto-advance loop for the life of
   the process.

Related, same class: the cpal output stream is built once per track against
`default_output_device()` captured at that moment, and its error callback only `eprintln!`s. If the
default device changes or the endpoint disappears (monitor sleeps → HDMI audio gone, BT/USB device
suspends), audio dies with no recovery and, because the callback stops draining the queue,
`is_finished()` again never fires. Same wedge, different cause.

**This is a regression from the webview era.** `plans/pandora-desktop-app.md` Round 24 (v0.6.5)
built a recovery ladder in `bridge.js` for precisely this ("after a long pause Pandora sometimes
spins forever on play until a manual engine refresh (expired stream URL)"): a 2 s watchdog on
`lastPosMoveAt`, escalating to re-click play, then reload the engine. That ladder was deleted with
the webview in c8118d2 and nothing replaced it.

## Decisions already made (don't re-ask)

- Recover **in place**: re-open the same signed URL and seek back to where the listener was, so a
  stall costs nothing. Only when re-opening fails (URL genuinely expired) do we skip to a new
  track — which is what "next" does today anyway, so it is never worse than the status quo.
- **Release the decoder and the audio device during a long pause** rather than holding a socket
  open for hours. Short pauses keep the player alive so pause/resume stays instant.
- Seek via `IMFSourceReader::SetCurrentPosition`. If it fails, fall back to restarting the track at
  0 and *report* position 0, so synced lyrics stay honest rather than confidently wrong.

## Plan

- [x] `Decoder::seek()` — `SetCurrentPosition` with a VT_I8 PROPVARIANT in 100 ns units.
      Needs the `Win32_System_Com_StructuredStorage` + `Win32_System_Variant` features.
- [x] `Player::play_at(url, offset)` — seeds `frames_played` *and* `total_decoded` so both
      `position()` and the `drift()` invariant stay correct across a rebuild.
- [x] `Player::device_error()` — cpal's error callback sets a flag instead of only printing.
- [x] `AudioThread` watchdog: remember the current track's URL; rebuild the player at the current
      position on a stall (position frozen while unpaused and not finished), on a device error, and
      on unpause after the player was released. Give up after `MAX_RECOVERIES` and publish
      `failed` so the engine skips.
- [x] `AudioThread`: release the player after `RELEASE_AFTER_PAUSE` of pause; publish `paused`
      unconditionally so the flag can't desync when there is no player.
- [x] `Engine::run()`: never return on an advance error. **Already fixed by a concurrent session**
      in 516aacf while this work was in flight — it retries on a 10 s timer and stays quiet about
      `STREAM_VIOLATION`. Left alone; defect 3 above is closed.
- [x] Test `Decoder::seek` for real (`seek_skips_ahead`) — a hand-built PROPVARIANT with the wrong
      `vt` is the kind of mistake MF accepts while ignoring the seek, which would silently turn
      every recovery into a restart-from-the-top.
- [x] `engine/examples/pause-resume.rs` — reproduces the reported bug against a live account:
      play, pause past the release threshold, press play, assert the same track resumes within
      2 s of where it stopped.
- [ ] **Run that example.** Not yet done: Pandora allows one concurrent stream per account, so it
      interrupts whatever is actually playing for ~90 s. Needs the user's go-ahead.
- [ ] Rebuild/install and confirm in the real app.

## Findings / gotchas

- `windows` 0.61 exposes `SetCurrentPosition(*const GUID, *const PROPVARIANT)` only when both
  `Win32_System_Com_StructuredStorage` and `Win32_System_Variant` are enabled; without them the
  vtable slot is a `usize` and the method does not exist. There is no `From<i64>` for `PROPVARIANT`
  at this version, so the variant is built field by field (`vt = VT_I8`, `hVal = ticks`).
- `total_decoded` must be seeded on a seek-resume as well as `frames_played`. Seeding only the
  latter makes `drift()` (the lost-audio detector, `decoded − position − buffered`) saturate to
  zero permanently and quietly stop detecting anything.
- Stall detection must be armed from player creation, not from first motion: a track that never
  produces a single frame is the exact case worth catching.
- Unpausing must reset the stall clock, or a long pause is instantly misread as a stall.

## Things not to do

- Don't treat a decode error as end-of-track without distinguishing it. The decode thread's
  `Ok(None) | Err(_) => finished` conflates "the song ended" with "the network died", and the
  engine then advances as if the track had played out.
- Don't hold a Pandora CDN connection open across an indefinite pause. Pandora allows one
  concurrent stream per account.

## Verification

Rebuilt and installed, then: pause for >1 min and press play (must resume at the same second);
pull the network mid-track (must recover or skip rather than go silent); change the default output
device mid-track.
