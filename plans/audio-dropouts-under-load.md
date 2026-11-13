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

- [x] Confirmed in real use, 2026-10-20 00:44–01:56 (local build of `07b5413`, 30 ms buffer
      throughout, autosync on): 26 songs, **zero underruns**, through four builds from other
      sessions (CPU 60–84%) and a from-scratch Jarlid release build with its own build dir,
      01:47:04–01:56:06 at **90–99% CPU**. The one event was a late callback (23.2 ms of
      30) at 1.1 s into a stream, during startup. The buffer never grew. Compare the evening
      before, unloaded: underruns within minutes and waits of 30–41 ms.
- One unrelated event at 01:42:50, with no build running: Windows reported the Dell's audio
  endpoint gone ("the requested device is no longer available"). The device-error path
  rebuilt on it at the same position about 1 s later. A DisplayPort audio dropout, not
  scheduling.

### Gotcha for anyone making load

`~/.cargo/config.toml` sets a machine-wide `build-dir`, so a "clean" build into a fresh
`CARGO_TARGET_DIR` still finds every intermediate cached and finishes in a second. Real load
needs `CARGO_BUILD_BUILD_DIR` pointed at an empty scratch directory too; that gave a 9-minute
from-scratch Tauri release build.
- If they continue with a one-period gap and an empty device, that is the endpoint (Intel
  Display Audio lives on the iGPU driver; autosync runs Vulkan on the same iGPU), and nothing
  in the app fixes it.

## Released

In **v1.14.1** (`27ac345`, 2026-10-20): the callback-thread MMCSS fix, the stream-only reopen,
and the callback timing in the debug log. Release run succeeded and all five assets are
attached (setup exe, msi, both signatures, `latest.json` at 1.14.1).

## Round 4 (2026-11-10): "still getting glitches in v1.20.1 when heavily loaded"

### What the debug log says (v1.20.1, installed 2026-11-09 6:46 PM PST)

- **Zero counted device underruns and zero starved stretches in v1.20.1.** The last underrun in
  the log is 2026-11-09 1:00 PM PST, on the build before. By the app's own measurement its output
  callback was never late enough to empty its 30 ms client buffer.
- What remains are "late callback" near-misses: 22–27 ms waits on the Dell (Intel Display Audio)
  today at 100% CPU, and 29–44 ms waits on the WH-1000XM4 (Bluetooth) on 2026-11-09 7:02 PM PST,
  with no underrun counted (the Bluetooth endpoint's real buffer is larger than the 30 ms asked
  for; the log prints the request, not the size Windows granted).
- **The shape is a burst:** each late callback is followed by callbacks 0.1–0.9 ms apart
  ("period" in the log is the shortest gap in the 50 ms poll). An auto-reset event signalled
  once cannot produce that if *our* thread was the one held up: it would give one big request
  and then the normal ~10 ms cadence. Back-to-back signals mean the audio engine (audiodg) was
  itself late and caught up in a rush. That points downstream of the app: audiodg, the
  endpoint driver, or DPC/ISR latency. A bigger client buffer does not help with any of those.

### Measuring it

ETW trace with WPR's built-in `Audio` (audio glitches) and `CPU` profiles, recorded at
12:18 PM PST while the machine sat at 100% CPU (rustc, Firefox, Defender, SearchIndexer) and
Jarlid was playing on the Dell. Analysed with `xperf` from the Windows Performance Toolkit.

#### Gotcha: WPR's `CPU` profile is unusable at 100% CPU

The first attempt (`wpr -start CPU -start Audio -filemode`, started 12:18 PM PST) produced a
**10 GB** ETL. `wpr -stop` then ran for more than 30 minutes, most likely generating NGEN PDBs for
managed processes, and was killed. `xperf -a tracestats` reported **177,530 lost events** and
exited 112 (`ERROR_DISK_FULL`, with 128 GB free on C:). The file was deleted.

Second attempt (12:56 PM PST): `wpr -start Audio -filemode` alone, 90 s, stopped with
`wpr -stop <file> -skipPdbGen`.

#### Second trace: a driver, not the app (2026-11-10 12:59–1:00 PM PST)

`wpr` refused to start again ("The profiles are already running", `0xc5583001`) while reporting
nothing recording: Windows' own `WPR_initiated_DiagTrackMiniLogger_WPR System Collector`
session holds the collector. Left alone; recorded with `xperf` directly instead
(`-on PROC_THREAD+LOADER+DPC+INTERRUPT+CSWITCH+DISPATCHER`, plus a `Microsoft-Windows-Audio`
user session). 60 s, 394 MB. No Jarlid late callback fell inside that minute, but the DPC
histogram (`xperf -a dpcisr`) is damning on its own:

- 601 DPCs over 2 ms in 66 s, 324 of them 8–16 ms, 3 of them 16–32 ms. **Every one is
  `vmswitch.sys`** (the Hyper-V virtual switch). No other driver has a single DPC over 1 ms.
- `vmswitch.sys` used 7.7 s of **CPU 0** (11.6%), and almost nothing on the other 11 CPUs.
- Network at the time: only ~1.6 MB/s, 1,700 packets/s on the I219-V.
- `VLAN-vSwitch` (external, on the I219-V, carrying 8 tagged host vNICs) has
  `SoftwareRscEnabled = True`. The I219-V has no VMQ, so all the switch's receive work runs as
  DPCs, and nearly all of it on CPU 0.

A DPC blocks everything at lower IRQL on its CPU, other DPCs included. A 16–32 ms vmswitch DPC on
the CPU that services the audio interrupt or timer delays audiodg by that much. That fits the
22–44 ms late callbacks followed by catch-up bursts exactly. Third trace (DPC only, 3 min) is
for timestamp correlation against the debug log.

#### Third trace: correlation (DPC only, 1:04:36–1:07:36 PM PST, 142 MB, 0 events lost)

`xperf -a dumper`, DPC rows, timestamps converted from the header's `Trace Start` FILETIME.

- **1,374 DPCs of 4 ms or more, every one `vmswitch.sys`, every one on CPU 0.** In 122 of 1,800
  100 ms windows (6.8%), vmswitch held CPU 0 for more than half the window.
- Jarlid late callbacks in the window, against the vmswitch DPCs of 4 ms or more in the 300 ms
  before each log line (lines are written at the engine's 50 ms poll):
  - 1:05:51.594 PM (23.6 ms wait): **eight back-to-back 12.5–16 ms vmswitch DPCs**,
    1:05:51.580–.681, i.e. CPU 0 almost solidly in vmswitch for ~110 ms.
  - 1:07:10.373 PM (28.2 ms wait): two 5.5 ms vmswitch DPCs, 1:07:10.264 and .427.
  - 1:07:11.082 PM (29.8 ms wait): none of 4 ms or more.
- So: a strong match for one, weak for one, none for the third. With vmswitch saturating CPU 0
  6.8% of the time, this is good evidence that vmswitch is *a* cause, not proof it is the only one.
- None of these three was an underrun of Jarlid's own 30 ms buffer. Whether audiodg's endpoint
  buffer glitched at those moments is not visible from inside Jarlid.

### Proposed fix (needs the user's explicit yes; host networking)

`Set-VMSwitch -Name "VLAN-vSwitch" -EnableSoftwareRsc $false`. Undo: same with `$true`.
Software RSC coalesces received TCP segments inside vmswitch's receive DPC. **Correction
(2026-11-10):** this was first written up as "the documented source of long vmswitch DPCs". That
overstated it. It is a common suspect, not something this trace shows. The trace says *that*
vmswitch runs long on CPU 0, not *which part* of vmswitch does. The I219-V has no VMQ, so the
switch cannot spread that work off CPU 0. At 1 Gbps the throughput cost of losing RSC should be
negligible.

Verify by repeating the DPC-only trace under the same kind of load and comparing the vmswitch
histogram and Jarlid's late-callback rate (baseline: 3 late callbacks in 3 min, 1,374 vmswitch
DPCs of 4 ms or more).

### Nothing app-side would help

If audiodg itself is held up, a bigger Jarlid buffer or a higher priority cannot help: the
engine is late for every stream on the machine. Exclusive-mode WASAPI would bypass audiodg, but
not the DPC queue on CPU 0, and it locks out other apps' audio.

### More findings (read-only, 2026-11-10 1:30 PM PST)

- `VLAN-vSwitch` extensions: **Microsoft NDIS Capture is Enabled** (Azure VFP and WFP are
  disabled). NDIS Capture is the packet-capture filter that `pktmon` / `netsh trace` use. It is
  a second suspect alongside RSC.
- Eight host vNICs on the switch (VLANs 0, 3, 4 (FMS), 5, 7, 11, 12, 99).
- No HD Audio module appears by name in the DPC table. The audio bus driver is KMDF, so its DPCs
  are counted under `Wdf01000.sys`, which runs on CPU 0 (1.34 s) and CPU 1 (0.48 s) in the
  3-minute trace. So some audio-driver DPCs very likely queue behind vmswitch on CPU 0.
- `jarlid.exe` and `audiodg.exe` both run with affinity `0xFFF` (all 12 CPUs).

### Candidate fixes, best evidence first

1. **Find out what vmswitch is doing** before changing it: a ~30 s sampled-stack trace (the
   `PROFILE` kernel flag with `-stackwalk Profile`), resolved against Microsoft's public symbols,
   shows which vmswitch functions the 16 ms is spent in. Then pick the change that targets it.
2. Disable software RSC on the switch (above).
3. Disable the NDIS Capture extension on the switch, if nothing relies on it.
4. Turn on default-queue vRSS (`Set-VMSwitch -DefaultQueueVrssEnabled $true`), which spreads
   vmswitch's host-vNIC receive work across CPUs in software. Unverified that it works without
   VMQ hardware.

All of 2–4 are host networking changes: one explicit yes per change.

### Moving Jarlid off CPU 0

Possible: `SetThreadSelectedCpuSets` (soft, preferred) or `SetThreadAffinityMask` (hard) on the
output callback thread. It only protects Jarlid's own thread, and only in one case: a thread
*running* on CPU 0 when a DPC starts is stuck until the DPC ends. A thread merely *waiting* to
run is already sent to another CPU by the scheduler. The burst pattern in the log points at
audiodg and the audio driver, not Jarlid's thread, and neither of those moves with Jarlid.

Cheap, reversible test without a code change: set `jarlid.exe`'s process affinity to `0xFFE`
(all but CPU 0) and compare the late-callback rate under load with the baseline (3 in 3 min).
If it drops, ship the CPU-set change. If not, it is audiodg or the driver, and the fix is on the
vmswitch side.

### Results (2026-11-10 afternoon)

User: Jarlid only plays to this computer (the Dell, Intel Display Audio), and "the scratchy is
pretty reliable now". Approved the affinity test and the stack trace; network changes: "let's
test first".

**The scratch is mostly invisible to Jarlid.** All afternoon: 2–10 late callbacks per half hour,
zero underruns, zero starved, while the user hears it reliably. The audio leaves Jarlid intact;
the damage happens downstream (audiodg, the endpoint driver, or the DisplayPort link).

**Affinity test: no effect.** `jarlid.exe` at `0xFFE` (off CPU 0) 2:54:31–3:47 PM PST: 8 late
callbacks in 53 minutes, all with the same 0.1 ms burst signature. Before: 10, 5 and 2 per half
hour from 1:30 PM PST. Restored to `0xFFF` at 3:47 PM PST. Don't build CPU pinning into Jarlid.

**What vmswitch's long DPCs are doing** (30 s, `PROFILE` with `-stackwalk Profile`, 2:56:36 PM
PST; Microsoft public symbols; `xperf -a dumper` with stacks, CPU 0 only):

- 1,817 of 29,997 CPU 0 samples (6.1%) have vmswitch on the stack. 1,570 of those start in
  `vmswitch.sys!VmsVrssDpc`, and 1,469 are below `ndis.sys!NdisMIndicateReceiveNetBufferLists`:
  vmswitch handing received packets up to the host's own TCP/IP stack.
- Where the CPU actually is: `NETIO.SYS!FindCacheMatch` 347, `FilterMatchEx` 146,
  `RtlLookupEntryHashTable` 85, `NETIO memcmp` 72, `MatchValues` 38, `IndexTrieClassify` 32,
  `CompareSecurityContexts` 28, with `tcpip!UdpReceiveDatagrams` / `UdpDeliverDatagrams`.
  That is **Windows Filtering Platform classification** (the firewall) of inbound UDP, run
  inside vmswitch's receive DPC. No RSC code and no NDIS Capture code in the samples.
  **RSC and NDIS Capture are cleared as suspects.**
- Traffic at 3:45 PM PST: ~970 unicast + ~150 multicast/broadcast packets/s on the I219-V. Not
  much, so the cost per packet is the problem: ~1.8 s of CPU 0 per 30 s.

**The filter set is bloated:**

- 5,500 WFP filters; 5,289 from Windows Firewall (`FWPM_PROVIDER_MPSSVC_WF`). 2,860 at
  `ALE_AUTH_RECV_ACCEPT` (v4 + v6), the layer inbound UDP is classified at.
- 917 firewall rules, 671 enabled, 466 of them inbound enabled.
- **206 of the 466 enabled inbound rules name a program that no longer exists.** 132 are under
  `target`, `Temp`, `build`, `.cargo` or `node_modules` paths: old test binaries and dev builds
  that Windows prompted for, and the "Allow" stayed. Repeat offenders include `gate.exe` (18),
  `empyrean-gate-mini-test.exe` (14), `electron.exe` (14), `node.exe` (14),
  `Microsoft Edge (mDNS-In)` (21).

**Side finding, not the cause:** `fl2000.sys` (Fresco Logic FL2000 USB display adapter) uses ~20%
of one CPU in System and WUDFHost threads. None of it is in DPCs.

### Firewall cleanup test: no effect (approved by the user, 2026-11-10)

- Counted rule by rule, 169 (not 206; the first count paired rules and programs by position)
  enabled inbound rules named a program that no longer exists. All 169 were **disabled** (not
  deleted) at 3:59 PM PST. List saved to
  `%LOCALAPPDATA%\firewall-cleanup\disabled-stale-rules-2026-11-10.csv`. Undo:
  `Import-Csv <that file> | % { Enable-NetFirewallRule -Name $_.Name }`.
- WFP filters 5,500 → 3,690. Enabled inbound rules 466 → 297.
- 3-minute DPC trace 3:59:22–4:02:22 PM PST (61–98% CPU), against the 1:04 PM PST baseline:
  vmswitch DPCs of 4 ms or more **1,466 vs 1,365**; CPU 0 in vmswitch 10.5% vs 11.5%. Jarlid: 7
  late callbacks 3:59–4:05 PM PST. **No improvement.** The size of the filter set is not what
  makes each packet expensive.

### What the inbound UDP is (pktmon, 15 s, 4:05 PM PST, physical NIC)

- **An mDNS loop between this PC and the AT&T gateway on VLAN-7.** 192.168.1.252 is this PC
  (`vEthernet (VLAN-7)`); 192.168.1.254 is the gateway (MAC `10-C4-CA-68-CE-B1`). The gateway
  multicasts `Noook.local` AAAA (476 in 15 s) and A (72) queries, ~36/s, and this PC answers each
  one unicast, often twice (793 four-record answers and 102 one-record answers). Together ~70% of
  the UDP captured. Most likely the gateway's device-name discovery never accepts the answer and
  keeps asking.
- Everything else is ordinary: occasional mDNS from the Mac mini on several VLANs, Cloudflare
  tunnel traffic (198.41.x.x:7844 from 10.255.0.77), QUIC.
- 18 sockets on 5353: Bonjour `mDNSResponder` (14, one per address), ChatGPT (2), Chrome (2).
  No delivery multiplier.
- **Doesn't add up yet:** ~100 mDNS packets/s cannot explain ~110 ms/s of CPU 0 in vmswitch DPCs
  unless each costs ~1 ms. The ~1,000 unicast packets/s in the interface counters are mostly not
  UDP, so the per-packet cost of WFP classification is still unexplained.
- Third-party WFP providers are registered ("Tessera Sandbox Provider", "ThisIsFine"), but no
  third-party module appears in the CPU 0 samples; the hot path is all `NETIO.SYS`.

### User answers (2026-11-10 ~4:25 PM PST)

- **Pandora in Chrome glitches too.** Jarlid is ruled out; the fault is system-wide.
- The 169 dead-program rules can stay disabled (or be deleted). Left disabled.
- Stop the mDNS loop; Bonjour test approved.
- Reducing the eight VLAN vNICs: open to it, but it's a bigger change (they're used for
  testing), to be planned carefully. Not now.

### Bonjour stopped (4:30:12 PM PST): the loop is the gateway's

- With `Bonjour Service` stopped the loop **doubled**: 933 AAAA + 179 A queries from
  192.168.1.254 in 15 s (~74/s), each answered once by Windows' own mDNS responder. The queries
  carry the QU bit (class `0x8001`, "unicast reply please"), get exactly that, and are re-sent at
  once: a gateway-side retry bug, paced by how fast answers come back.
- 3-minute DPC trace 4:31:42–4:34:42 PM PST at 100% CPU: vmswitch DPCs of 4 ms or more 1,214
  (vs 1,466 and 1,365); **CPU 0 in vmswitch 7.9%** (vs 10.5% and 11.5%).
- Jarlid: **0 late callbacks 4:30–4:36 PM PST** at 100% CPU, vs 7 in 3:59–4:05 PM PST. Small
  sample.
- Odd: Jarlid reopened its stream at 4:30:13 PM PST, one second after Bonjour stopped ("opened …
  at 102.7s"). A network-change side effect, probably harmless; not chased.

### User (2026-11-10 ~4:45 PM PST): "better... maybe, but not fixed". Why is it asking? Restart Bonjour.

Bonjour restarted 4:46:24 PM PST.

**Why the gateway keeps asking: still unknown.** What was checked:

- The gateway is AT&T's (`10-C4-CA` is a Humax OUI; AT&T's BGW320 is Humax-built). It queries
  only `Noook.local` (no other host on that LAN gets this), with the QU bit, ~30–60/s, and each
  query frame arrives **twice** at the NIC, 1.4 µs apart.
- **Hypothesis disproven:** that the answers leave out the IPv6 privacy address the PC actually
  uses. Both responders answer with every VLAN-7 address: `…8a10::49` (DHCPv6), `…78f6:…`
  (SLAAC), `…b9a9:…` (temporary, the one in use), and the link-local; Bonjour adds the A
  record. Replies go unicast to the gateway's MAC with cache-flush set. They are correct.
- Still a candidate: the replies never reach the gateway. This PC has two adapters on
  192.168.1.0/24: `VLAN-7` (.252, where the gateway is) and `VLAN-5` (.11, where .254 is
  unreachable). The VLAN each reply leaves on is unverified: a pktmon capture on the VLAN-5/7
  vNIC components logged only "Filtered VLAN" drops, not the traffic.
- Reply-to-next-query timing from pktmon is **not usable**: timestamps at the NIC component
  cluster by DPC batch (62 "unique" queries in 10 s by a 0.5 ms dedupe, against 578 frames), so
  they show when vmswitch processed a batch, not wire time.

### Verified (2026-11-10 ~5:00 PM PST, read-only)

- **The replies leave on VLAN 7.** Their source MAC `00-15-5D-18-7A-05` is the `VLAN-7` host
  vNIC (Access, VLAN 7), so vmswitch tags them 7. Over 10 s, `VLAN-7` sent ~95 unicast
  packets/s (the replies) and `VLAN-5` sent none. The wrong-VLAN hypothesis is **disproven**.
  From the PC's side the answers are correct and on the right wire; whether the gateway hears
  them can't be seen from here. Remaining explanations: the gateway's firmware, or something
  between the switch port and the gateway.
- **VLAN-5 is a dead leftover.** Static 192.168.1.11/24 (set around 2025-09-14), default
  gateways 192.168.1.1 and 192.168.1.2 (both ARP `Incomplete`), DNS the `fec0:0:0:ffff::1–3`
  placeholders, network "Unidentified / Public", **0 bytes received since boot** (14.6 MB sent,
  all unanswered). Looks like a static setup for reaching a device at a factory default address.
  Not in the ops repo's UniFi config (no VLANs are defined there). It costs no receive load, so
  it isn't part of the audio problem.
- **A trap it does cause:** both `VLAN-5` and `VLAN-7` have a 192.168.1.0/24 route at metric
  256, and Windows picks `VLAN-5` for 192.168.1.254 (`Find-NetRoute`). Anything on this PC that
  talks to 192.168.1.x without binding a source address goes into the dead VLAN, e.g. opening
  the gateway's web page.
- **VLAN-5 disabled** (2026-11-12, user approved): `Disable-NetAdapter "vEthernet (VLAN-5)"`
  (MAC 00-15-5D-18-7A-03; the Hyper-V vNIC and its static config are kept). Afterwards
  `Find-NetRoute 192.168.1.254` picks `VLAN-7` and TCP 80 on the gateway connects. Undo:
  `Enable-NetAdapter -Name "vEthernet (VLAN-5)"`.

### Gateway device list (2026-11-13 ~2:00 AM PST, read-only, user logged in and authorized)

`http://192.168.1.254/cgi-bin/devices.ha` (it also loads without a login; Chrome hung on it,
so it was fetched with `Invoke-WebRequest`). Nothing was clicked; "Clear and Rescan" was not
touched.

- **The gateway hears the answers.** It lists `00:15:5d:18:7a:05` (the VLAN-7 vNIC) as
  `192.168.1.252 / Noook`, status on, static, on port Ethernet LAN-2. It lists all four of the
  PC's IPv6 addresses (two SLAAC privacy addresses, the DHCPv6 `::49`, and the link-local).
  So it has the name and every address, and keeps asking anyway. The "it never gets an
  answer" theory is **disproven**. What's left is the gateway's own behaviour (firmware).
- Only two devices are active: Noook (LAN-2) and `e4:38:83:1d:41:cd` with the public address
  108.65.74.188 on LAN-1 (labelled "Google Chromecast series", but the e4:38:83 prefix is
  Ubiquiti: this is the UniFi gateway in IP passthrough). Six other entries are stale (off
  since October).
- Lead, not yet checked: each query arrives twice at the PC. With the UniFi gateway on LAN-1
  and the switch on LAN-2, a duplicate suggests the same frame reaches VLAN 7 by two paths.

### Open questions for the user

1. Block this PC's answers (firewall rule: inbound UDP 5353 from 192.168.1.254 on
   `vEthernet (VLAN-7)`) as a test, to see whether the gateway backs off when unanswered.
   Side effect: the gateway can't learn the PC's name (cosmetic in its device list). Needs a yes.
2. ~~Can the user check the gateway's device list?~~ Read 2026-11-13: it knows Noook's name
   and addresses. Original question: How it shows Noook, and whether
   it shows the PC twice, would say whether it hears the answers. A third-party dashboard: the
   user looks, or explicitly authorizes a read. (Reachable from this PC now that VLAN-5 is
   disabled.)
3. ~~Remove or disable the dead `VLAN-5` vNIC?~~ Disabled 2026-11-12. Removing the vNIC
   outright is left for the later VLAN-trim task.
