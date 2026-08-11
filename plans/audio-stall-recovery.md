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
- [ ] **Run that example.** Still not run: Pandora allows one concurrent stream per account, so it
      interrupts whatever is actually playing for ~90 s. Overtaken by events — the fix shipped and
      has been in real use instead (below), so this is now a regression test for later rather than
      the thing standing between us and confidence.
- [x] Rebuild/install and confirm in the real app — shipped in **v1.2.0** ("Stations page,
      Settings, audio stall recovery", 645b2a0) and running since. See Verification.

## Recovery latency (asked about directly, 2026-08-10)

"Do I have to wait for it to self-recover?" — worth stating per path, because the first cut had
one bad answer:

| What happened | How it's noticed | Silence before audio returns |
|---|---|---|
| Long pause, you press play | The player was already released *during* the pause | Re-open + seek + buffer, well under a second |
| Connection dropped, MF returns an error | `decode_error`, immediately | None — the ring buffer covers the re-open |
| MF hangs in `ReadSample` (half-open socket) | Decoded output stops advancing | None to ~1 s — caught with buffer still in hand |
| Output device dies | cpal error callback | One poll, 50 ms |
| Device stops without reporting an error | Queued audio not being consumed | ~4 s (no earlier signal exists) |

The third row is the one that was wrong. The first version timed the **position**, which keeps
advancing off the ring buffer for a further five seconds after a read hangs and only freezes once
the buffer is dry — i.e. the clock started when the silence did, then added eight more seconds.
Timing **decoded output** instead catches the hang the moment it happens, roughly five seconds of
buffer before anyone could hear it, so recovery normally completes inaudibly. The position timer
survives only as a backstop for the opposite failure — audio queued that nobody is consuming.

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
- Watch the decoder, not the playhead. The five-second ring buffer means the position is a
  *lagging* indicator of a stalled read by exactly the margin you needed to fix it in.
- A decoder that has legitimately reached the end of a track also stops producing and lets the
  buffer drain, which is indistinguishable from a hang unless `end_of_stream` is checked. Without
  that gate the watchdog would re-open every track a few seconds before it ended.
- Unpausing must reset the stall clock, or a long pause is instantly misread as a stall.

## Things not to do

- Don't treat a decode error as end-of-track without distinguishing it. The decode thread's
  `Ok(None) | Err(_) => finished` conflates "the song ended" with "the network died", and the
  engine then advances as if the track had played out.
- Don't hold a Pandora CDN connection open across an indefinite pause. Pandora allows one
  concurrent stream per account.

## Verification

**Status 2026-08-11: in use, no recurrence, not yet deliberately provoked.**

- Landed in v1.2.0 (645b2a0). Nothing since has touched `audio_thread.rs`, `player.rs` or
  `media_foundation.rs` — `git log 81db310..master --` on those three is empty, so what shipped is
  what was written.
- Cameron has been on v1.3.3 (installed 2026-08-10 17:25, running continuously since 17:45) and
  reports no sign of the bug across roughly a day of normal listening. That is the original
  symptom failing to reproduce over the interval it used to appear in — good evidence, but passive:
  it does not confirm the recovery paths *fired* and worked, only that nothing broke visibly.
- `Decoder::seek` is covered by a unit test (`seek_skips_ahead`).

Still worth doing deliberately, in the app, when convenient — each takes under two minutes:

1. Pause >1 min, press play. Must resume at the same second of the same track, not restart it and
   not jump to a new one. This is the exact reported bug, and the one path with a visible tell.
2. Kill the network mid-track (disable Wi-Fi ~10 s, re-enable). Must recover or skip, not go
   silent forever.
3. Unplug headphones / let the monitor sleep mid-track, exercising the device-error path.

If a recovery ever does fire it prints its reason and position to stderr (`decoding stalled at
Xs; reopening`), which the release build has nowhere to show — worth routing into the diagnostics
report if this ever needs debugging in the wild.
