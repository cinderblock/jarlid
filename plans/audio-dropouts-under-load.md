# Audio dropouts when the machine is loaded

## Goal

Jarlid drops audio when the computer gets busy. Make playback survive a loaded machine, and make
the failure *measurable* so "is it the app or is it my PC" stops being a matter of opinion.

## Symptom (user, 2026-08-11)

> when the computer gets more overloaded... it starts to drop audio packets. do we need to set
> higher priority or something?

## Diagnosis

Right instinct, but the obvious thread was already handled — so raising "the audio priority" in the
obvious place would have changed nothing.

**cpal already runs its WASAPI callback at `THREAD_PRIORITY_TIME_CRITICAL`** (`cpal-0.16.0`,
`src/host/wasapi/stream.rs:353`). The callback is never the thread that loses a CPU race, and it
was never the problem.

The problem is the **decode thread** in `crates/audio/src/player.rs`, spawned with a bare
`std::thread::spawn` at default priority. It is the producer for the ring buffer the callback
drains. Under load — a big compile, a render, a game — the scheduler can deschedule it long enough
for the 5-second buffer to run dry. The callback then does exactly what it is supposed to do and
emits explicit silence, which is what gets heard as a dropout. Every layer behaves correctly; the
audio is missing because it was never decoded in time.

So the fix belongs on the producer, not the consumer.

## Decisions already made (don't re-ask)

- **MMCSS, not a bare priority number.** `AvSetMmThreadCharacteristicsW` with the `Audio` task is
  Windows' sanctioned way to say "this thread feeds a stream": the scheduler guarantees it a share
  of each period rather than leaving it to compete on priority alone. A plain `SetThreadPriority`
  bump is the fallback for when MMCSS is unavailable (it can be disabled by policy) — weaker,
  because it buys no guarantee, but better than nothing.
- **`Audio`, not `Pro Audio`.** `Pro Audio` is for low-latency render threads. This is a decoder
  feeding a five-second buffer; claiming the strictest class for it would be dishonest and would
  take scheduling headroom from things that need it more.
- **Don't just make the buffer bigger.** Five seconds is already generous. If the decode thread
  cannot run for five seconds the machine has a scheduling problem, and a bigger buffer would only
  postpone the same dropout while making pause/skip mushier.

## Plan

- [x] Register the decode thread with MMCSS for its lifetime (`AudioPriority` guard — MMCSS state
      is per-thread and must be reverted on the same thread, hence a guard rather than a bare call).
- [x] Count the dropout: `Player::starved()`, incremented in the callback whenever it has to invent
      silence. Excludes the end of a track and the priming gap before the first full buffer, which
      are both empty-queue-by-design rather than faults.
- [x] Publish it through `AudioThread`/`Engine` as a running total across tracks and rebuilds.
- [x] Put it in the diagnostics report, so a dropout complaint arrives with the number attached.
- [x] Test that MMCSS registration actually succeeds (`mmcss_registration_succeeds`).
- [ ] Confirm in real use: load the machine, watch `starved` in a diagnostics report.

## Findings / gotchas

- **The interaction with the stall watchdog is the dangerous part.** The watchdog added in
  `plans/audio-stall-recovery.md` treats "decoded output frozen for 3 s while the buffer is low" as
  a hung read and re-opens the stream — and CPU starvation looks exactly like that from the
  outside. Under heavy load it could have started re-opening streams, which is *more* work, and
  after three attempts it gives up and skips the track. Losing a song because a build was running
  would be a bad trade. Mitigated two ways: MMCSS makes three seconds of *zero* decode progress
  mean "genuinely hung" rather than "busy", and `RECOVERY_FORGIVENESS` dropped 30 s → 10 s so
  intermittent stalls stop accumulating toward a skip. Worth remembering if either constant is
  ever tuned again.
- Underrun counting has to survive the priming gap or every track starts with a few milliseconds
  of "dropout" and the metric stops meaning anything. Gated on having filled one complete buffer,
  which also covers the gap after a stall rebuild.
- The counter is accumulated as a *delta per poll* rather than folded in when a player is dropped,
  so it survives every rebuild path without each one having to remember to contribute.
- `starved` and `drift` are different faults and both are worth keeping: drift is audio that was
  decoded and then *lost* (a correctness bug in the queue), starved is audio that arrived too late
  to play (a scheduling problem). Conflating them would hide whichever is rarer.

## Things not to do

- Don't raise the cpal callback priority. It is already `TIME_CRITICAL`; going higher just starves
  something else.
- Don't register the *whole process* as high priority to fix this. The problem is one thread.

## Verification

- `cargo test -p audio` — MMCSS registration succeeds on this machine (asserted, not assumed:
  a wrong task name or missing crate feature fails silently into the fallback).
- Still to do: run the machine hot and check `starved` stays at zero in a diagnostics report.
  Before this change it should climb whenever audio breaks up; after it, it should not.

**Do not build the app on the machine you are verifying on, at the time you are verifying.**
A Tauri release build claims four of the machine's four compute slots (`compute-budget` skill) and
is precisely the kind of load that causes this bug. Building while listening would manufacture the
symptom, degrade the audio it is meant to fix, and make any reading of `starved` meaningless —
you would be measuring the build, not the fix. Build first, then let the machine settle, then
provoke the load deliberately.

As of 2026-08-12 a build could not be started anyway: 2 of 4 slots were held by a long-lived
`svdsa presentation dev (cargo+vite)` server, and a cargo build wants all four, so the claim would
have queued behind a process that does not exit on its own.

## Round 2 (2026-10-13): "scratchy", not gaps

User report: under CPU load the audio gets *scratchy*. That is a different sound from the
multi-second hole a dry 5 s ring buffer makes, and it points at the **device buffer**, not the
decode thread.

- `Player::new` opened the stream with `default_output_config().config()`, i.e.
  `BufferSize::Default`. cpal 0.16 turns that into `buffer_duration = 0` in
  `IAudioClient::Initialize` (`src/host/wasapi/device.rs`), so WASAPI shared mode uses its
  minimum. **Measured: 22 ms** (1056 frames at 48 kHz) on the Dell monitor's Intel Display Audio
  endpoint, refilled one ~10 ms engine period at a time.
- A callback that is late by more than that glitches, even at `TIME_CRITICAL`, and even with
  five seconds of decoded audio sitting in the ring. Short repeated glitches are heard as
  crackle.
- **`starved` cannot see this.** It counts the ring running dry inside the callback; a late
  callback with a full ring counts zero. A clean `starved` reading would not clear the app.
- cpal fills `buffer_frames - GetCurrentPadding()` each event, so a larger requested buffer
  is real cushion, not just a bigger allocation.
- cpal's `OutputCallbackInfo::timestamp().playback` is **not** usable for latency: it is
  `callback + frames_available`, which says nothing about what is already queued. The request
  size is the useful signal, since `requested = buffer - padding`.

### Decisions (user, 2026-10-13; don't re-ask)

- **Start small and grow only when needed**, rather than a fixed 100 ms. The user asked for about
  30 ms "so it still feels fast", growing if we are dropping frames.

### What shipped

- [x] `audio::DEVICE_BUFFER` = 30 ms, passed to `Player::play_on` as `BufferSize::Fixed`, clamped
      to the device's `SupportedBufferSize`. If the device refuses the size, the player falls back
      to `BufferSize::Default` rather than failing. The ring and channels are rebuilt for the
      retry, and the decode thread is now spawned only after the stream exists.
- [x] Underrun detection (`DeviceQueue` in `player.rs`, unit-tested): a callback that asks for
      the *whole* buffer found the device queue empty. It is only counted once a smaller request
      has proved the buffer is bigger than one period, and never while paused.
- [x] Adaptive size (`DeviceBuffer` in `audio_thread.rs`, unit-tested): 2 underruns within 60 s
      doubles the size, 30 → 60 → 120 → 240 ms (the cap). The next player gets the new size. If
      underruns continue on the current, undersized player after a grow, it is rebuilt mid-song
      without spending the recovery budget. 10 minutes without an underrun halves it again, but
      only at the next natural build, never by forcing one.
- [x] Position correction: `position()` subtracts the real audio still in the device buffer, so
      lyrics stay synced at 240 ms. `drift()` uses the uncorrected delivered count, or it would
      report the buffer as lost audio.
- [x] Diagnostics report: `underruns N (device buffer X ms)` next to `starved`.
- [x] `crates/audio/examples/device-buffer.rs`: plays a file muted at a given size and prints
      the measured buffer, underruns and position. Verified: 30 ms and 240 ms are granted
      exactly, and the position tracks wall-clock time at both (0.500 s at 0.5 s).
- [ ] Confirm under real load: underruns should appear and the buffer should grow. This was not
      reproducible here, because beating a `TIME_CRITICAL` thread needs a realtime-class
      process. The detection logic is covered by unit tests on simulated request sequences.
- [ ] When it crackles, does other audio on the machine crackle at the same moment? If so, the
      cause is `audiodg`, a driver or DPC latency, and no change here can fix it. This question
      is still open with the user.
