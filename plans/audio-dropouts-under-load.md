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

> **Wrong** (found 2026-10-20, round 3 below): that code passes a thread id as a handle and the
> call fails; the callback thread was at normal priority all along. Kept as written above
> because it is the mistake the rest of this file was built on.

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

- ~~Don't raise the cpal callback priority. It is already `TIME_CRITICAL`.~~ **It was not** —
  see "The premise was wrong" in round 3. It is now registered with MMCSS from the first
  callback. Don't go further than that: the `Pro Audio` class is the sanctioned ceiling.
- Don't register the *whole process* as high priority to fix this. The problem is one thread.
- Don't trust a claim about a thread's priority that was not measured from that thread. The
  one at the top of this file cost a week.

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

## Shipped

In v1.12.0 (2026-10-13). It was released before the open verification items above were
checked, at the user's call. Those items still stand, now against the released build.

## Round 3 (2026-10-19): "I'm still getting underruns"

Read from the debug log (`%LOCALAPPDATA%\com.camer.pandora-desktop\logs\jarlid.log`), session
15:29–17:37 on installed **v1.13.2**, default output = Dell P2715Q (Intel Display Audio), on an
i7-10710U with 64 GB (30 GB free, so not memory pressure).

### Findings

- **The adaptive buffer worked as designed and did not help.** It climbed 30 → 60 → 120 → 240 ms
  inside three minutes, and then **two underruns happened at 240 ms, the cap** (15:41:18 and
  15:41:25). It relaxed back to 30 ms over the next hour, and the cycle repeated (16:05, 16:19,
  17:29).
- **The callback period on this device is ~10 ms at every buffer size** (measured 2026-10-19
  with the instrumented `device-buffer` example, muted: 30 ms and 240 ms both run at a 6–10 ms
  period, longest wait 18 ms). So an underrun at 240 ms means about 24 periods with no callback,
  or the device discarding a full buffer. That is not scheduling jitter, and a bigger buffer is
  not the fix. The cause is a stall of a quarter of a second or more, and its source is unknown.
- **Underruns come in pairs or threes, 4–7 s apart** (84.1/87.9 s, 39.4/46.9/76.7 s,
  113.5/120.4 s, 114.5/120.0/151.4 s). No correlation with autosync: most happened with the
  speech engine idle. Only one (17:29:01) happened while it was running.
- **The mid-song rebuild is itself a ~1 s dropout.** "Still underrunning; rebuilding" drops the
  player and reopens the CDN stream at the position. Measured from the log: 15:30:46.865 →
  47.918 (1.05 s), 15:33:18.260 → 19.437 (1.18 s), 16:06:02.610 → 03.436 (0.83 s). That is a
  longer hole than the glitch it is trying to fix.
- The bursts drove the buffer up two sizes in 110 ms (15:30:46.694 and .808). That is
  `a_burst_in_one_poll_counts` working as written, but it means one bad moment can count twice.

### Instrumentation added (`6d6edb6`)

Each underrun line now says how long the callback waited before it, the device period and the
slowest callback. A callback waiting more than ¾ of the buffer is logged as a late callback.

- Wait ≈ buffer or longer: the callback thread was not run (or cpal's loop was blocked).
- Wait ≈ one period with the device empty: Windows played or dropped a whole buffer in one
  period (an engine or endpoint event, not us).
- Slowest callback large: our own callback stalled (a page fault, for instance).

Not yet in any installed build. A release or a local install is needed before the next
occurrence can be read.

### Decisions (user, 2026-10-19; don't re-ask)

- **No other audio is playing when it crackles**, so the "does other audio crackle too" test
  cannot be run. The callback timing has to settle it instead.
- **A local install is fine** for getting the new logging onto the machine; no release needed.
- **Fix the 1 s rebuild gap**: keep the decoded audio and reopen only the stream. Done, below.

### The reopen (built 2026-10-19)

`Player::reopen_on(output, device_buffer)` in `player.rs`. The callback's track state
(`Live`: ring consumer, voice, blend, control queues, gain ramp) is carried into the stream's
closure by `Carried`, whose `Drop` pushes it back through a one-slot rtrb queue when cpal drops
the closure. cpal's `Stream::drop` joins its thread first, so after `drop(stream)` the state is
there to pop. `open_stream` is the stream-building half of `play_on`, extracted so both use it;
its "device refused the size" retry also goes through the same hand-back, which removed the old
rebuild-the-ring-and-channels dance.

Before dropping the old stream, the player pauses itself and waits for `device_audio` to reach
zero (bounded by buffer + 250 ms), so the audio Windows was holding plays out instead of being
discarded. The position therefore moves by exactly the call's wall-clock time, and nothing is
skipped. Refused (playback untouched) if the target device's rate or channel count differs
from the format the ring was decoded for; the engine then falls back to the full rebuild.

The engine uses it for both the undersized-buffer rebuild and "default output moved". The
device-error and stream-stall paths still build a fresh player, as they must (the stream or the
device is gone).

Measured with `cargo run --example device-buffer -- <from> 6 <file> <to>` on the Dell endpoint,
muted (`reopened at … in X ms; position a -> b`):

| reopen | the call took | playhead moved | gap (call − moved) |
| --- | --- | --- | --- |
| 30 → 120 ms | 75 ms | 40 ms | ~35 ms |
| 240 → 30 ms | 225 ms | 199 ms | ~26 ms |

The playhead moving by the device's buffer is the old queue playing out, not a skip; the gap
is the stream open itself. Against 0.83–1.18 s for the old rebuild from the 2026-10-19 log.

### The data (local build installed 22:20 on 2026-10-19, 20 minutes of play)

Every event had the same shape: the callback waited **22–41 ms** to be run, and the callback
itself took **0.04–0.55 ms**. One late callback (12.3 s in) took 39.7 ms *inside* the callback,
i.e. it was pre-empted mid-call. The device never had anything to do with it.

```
1 device underrun at 9.0s (30 ms buffer); the callback came 41.5 ms after the one before (period 9.3 ms, slowest callback 0.06 ms)
2 device underruns at 73.1s (30 ms buffer); the callback came 33.3 ms after the one before (period 3.2 ms, slowest callback 0.10 ms)
```

The reopen also ran for real at 22:40:16: `reopening at 73.2s with 60 ms` → `reopened … at
73.2s, 60 ms device buffer` 200 ms later, position unchanged, no CDN reconnect.

### The premise was wrong: cpal never raised the callback thread

The diagnosis at the top of this file says cpal runs its WASAPI callback at
`THREAD_PRIORITY_TIME_CRITICAL`. The code is in `cpal-0.16.0/src/host/wasapi/stream.rs`,
`boost_current_thread_priority` (the `audio_thread_priority` feature is off):

```rust
let thread_id = Threading::GetCurrentThreadId();
let _ = Threading::SetThreadPriority(HANDLE(thread_id as isize), THREAD_PRIORITY_TIME_CRITICAL);
```

A thread *id* where a thread *handle* belongs, and the error discarded. Measured from inside the
callback with `GetThreadPriority(GetCurrentThread())`: **0, normal priority.** So the thread
that has a 30 ms deadline was competing on equal terms with everything else on the machine, and
a 30–40 ms hold under load is exactly what that looks like. "Raising the callback priority
would not help, it is already TIME_CRITICAL" was asserted twice in this file and never checked.

### Fix

The first callback registers its own thread with MMCSS as `Pro Audio` (`AudioPriority`, now
taking a task name; falls back to `SetThreadPriority(TIME_CRITICAL)`), and the guard lives in
the closure, which cpal drops on the same thread. It records what it found and what it got;
the engine logs `output callback thread found at priority 0; now MMCSS Pro Audio
(priority 15)` once per stream, the `device-buffer` example prints the same, and
`the_output_callback_raises_its_own_thread` pins `found == 0` so a cpal upgrade that fixes
this upstream will fail the test and say so.

- [ ] Confirm in real use: with the thread registered, underruns under the same load should
      stop, or at least the "callback came N ms after" figure should stop exceeding the buffer.
- If they continue with a one-period gap and an empty device, that is the endpoint (Intel
  Display Audio lives on the iGPU driver; autosync runs Vulkan on the same iGPU), and nothing
  in the app fixes it.
