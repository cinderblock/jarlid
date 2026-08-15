# DJ blend mode — beat-matched transitions between tracks

## Goal

Instead of one song ending and the next starting, overlap them: fade the outgoing track out
while the incoming one comes in, **beat-matched** so the two pulses line up. Where the tempos are
close enough, nudge the time base to make them agree. A **maximum BPM deviation** control decides
how far we're willing to stretch; beyond it, no blend — just the normal end-of-song transition.

Status: **researched, not started.** The gating experiment has been run and passed (see Findings).

## Environment / context

- Same worktree and app as [`technical-footer-bpm.md`](technical-footer-bpm.md), which added the
  BPM measurement this feature builds on (`crates/audio/src/tempo.rs`).
- Windows-only in practice; audio is Media Foundation + cpal.

## Decisions already made (don't re-ask)

1. **Tempo matching by resampling, not time-stretching.** Playing a track fast shifts its pitch by
   the same ratio. That is exactly what a CDJ pitch fader does, so it is authentic rather than a
   compromise, and it avoids a phase vocoder and any new dependency.
2. **The "maximum BPM deviation" control is a pitch-fader range.** ±6% / ±10% / ±16% are the
   standard CDJ settings; ±6% at 128 BPM is ±7.7 BPM of catch.
3. **Rate control lives in the mixer, not the decoder.** See Findings — Media Foundation cannot
   change its resample ratio mid-stream, so it can only ever give a fixed offset.
4. **Pre-buffer the incoming track into RAM.** Confirmed cheap by experiment; see Findings.

## Findings / gotchas

### ✅ GATING EXPERIMENT PASSED — Pandora serves two concurrent audio streams

`crates/engine/examples/concurrent-streams.rs`, run 2026-08-12. It mirrors the real scenario:
stream track A at playback speed, and five seconds in — while A is still open and being consumed
— open track B and pull a pre-buffer as fast as it will come.

```
[  0.0s] A opened: HTTP 200 OK
[  5.0s] --- opening B while A is still streaming ---
[  5.2s] B opened: HTTP 200 OK
[  5.6s] B pre-buffered 769 KB
[ 28.4s] A still reading: 454 KB
--- asking the tuner API for another playlist ---
OK, 4 tracks
```

Three separate things had to hold and all did:

- **B was served** — HTTP 200, not 403 or a refusal.
- **A was unharmed** — it kept reading at playback speed for the rest of the run. This was the
  outcome that mattered most; A dying would have been a mid-song dropout in the app.
- **The account was not flagged** — the follow-up `getPlaylist` returned normally rather than
  `STREAM_VIOLATION`.

**The important number is 769 KB in 0.4 s** — about 48 seconds of 128 kbit/s audio, ~120×
realtime. Pre-buffering 30-45 s of the next track therefore costs well under a second, not the
"second or two" originally assumed.

That collapses the risk: the two-connection window is ~0.4 s, not the ~30 s of a crossfade. The
blend itself runs from RAM with only the outgoing track live.

**Caveat, deliberately not chased:** Pandora may count "streams" from `audioReceiptURL` pings
rather than raw CDN fetches, and the probe sent none. That is the safe side — a pre-buffer has
not been played and should not send a receipt either. Keep it that way.

### Media Foundation cannot glide the tempo — the mixer must

`Decoder::open_at(url, Some(format))` fixes the resample ratio **at open time**. It cannot change
mid-stream: `next_chunk()` explicitly errors on `MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED`
("stream changed format mid-playback"). So the tempting trick of asking MF for
`48000 × 124/128` gives exactly one fixed pitch offset chosen before the first sample, and no
ramp at all.

Tempo control therefore belongs in the mixer: each source holds a **fractional read position**
into its ring buffer, advanced by `rate` per output frame with interpolation between samples.
Ramping `rate` gives a continuous glide — structurally what the existing volume ramp already does
(`VOLUME_RAMP`, one-pole over ~15 ms), just over seconds. Cubic interpolation is ample for ±16%
and needs no dependency. This is better than the MF trick regardless, and the mixer needs it
anyway.

### One `Player` is one cpal stream — this is the real work

`AudioThread` holds `player: Option<audio::Player>`, and each `Player` opens its own output
stream. Two streams on one device are **not sample-synchronised** — separate callbacks, separate
clocks. Adequate for a dumb crossfade, fatal for beat matching, because the phase relationship
between the two tracks would drift over the blend.

So `Player` has to split into a **Source** (decoder + ring buffer + fractional-rate reader) and an
**Output** (one cpal stream + mixer pulling from N sources). That is the bulk of the effort, and
it is independently worth doing because it also gives true gapless playback.

Do not do the mixing in the cpal callback naively — `player.rs:7-11` records that a `Mutex` in
that path once produced "continuous scratchiness". The callback must stay allocation-free and
lock-free; sources feed it through the same lock-free ring-buffer discipline used today.

### BPM alone is not enough — beat *phase* is needed

`TempoTracker` returns a period, not a downbeat position. Two tracks at identical BPM but
off-phase sound worse than no blend at all. Phase means cross-correlating the onset envelope
against a pulse train at the found period — roughly 100 lines, testable exactly like the existing
tempo tests. The envelope is already computed, so this is an extension rather than new machinery.

### Probably no glide is needed in the common case

The classic DJ move pitches only the **incoming** track to match the outgoing one. The song
already playing never changes speed, so nothing audible glides.

Nicer variant, available because we own the whole pipeline and this is radio rather than a club
set: once the blend completes, glide the incoming track back to its native tempo over 20-30 s. A
3% correction spread over 30 s is about a cent per second, far under the perceptual threshold —
seamless mix in, and the song settles to its true speed.

### Cold endings should suppress the blend

Plenty of tracks end hard rather than fading. Crossfading into one sounds wrong. The onset
envelope already gives the signal for free: a sharp energy cliff in the outgoing track's final
seconds should back the blend off to a normal transition.

### Re-opening after the RAM buffer runs out

When the blend completes, the incoming track has to continue from wherever the pre-buffer
stopped: re-open its URL and seek. That is exactly the existing stalled-stream recovery path, and
`master` recently made it safer — `8d07d6f audio: report where the seek landed, not where it was
aimed`. That matters here specifically: a seek landing earlier than requested would replay audio
the listener already heard.

## Plan / steps

- [x] **0. Rebase onto master.** The BPM work had already been merged and released as v1.5.0 by
      another session, so the rebase dropped four duplicate commits and kept only the probe.
- [x] **1a. The mixing core** — `crates/audio/src/mixer.rs`. `Voice` reads interleaved PCM at a
      fractional, movable rate and mixes into a shared buffer at a movable gain. Pure arithmetic,
      no `cpal`/`rtrb`/MF, 9 tests. Commit `83308c3`.
- [x] **1b. Settings** — `BlendMode` (Off / Crossfade / BeatMatched), overlap seconds, max pull
      percent, restore-tempo. Rust + UI, 6 tests. Commit `deba0eb`.
- [x] **2. The output callback mixes instead of popping.** Commit `cb64317`. Verified in the
      running app, not only by test: two minutes at drift 0.00 s, starved 0.00 s, buffer steady
      at 5.0 s. Drift is exactly the invariant that fails if the frame accounting is wrong.
- [x] **3. Pre-buffer the next track into RAM** and measure its tempo — `crates/audio/src/prefetch.rs`,
      commit `e61c4b9`.
- [x] **4. Beat phase in `TempoTracker`.** Commit `2c8e1de`.
- [x] **5a. A second voice in the player**, fed from a prefetched buffer, fading in on the
      equal-power curve. Commit `527845a`. `Player::blend_in` / `blend_done` / `blend_position`.
      Nothing calls it; verified only that it costs ordinary playback nothing.
- [ ] **5b. Sequencing.** The last piece, and the only one that is not additive. See below.
- [ ] **6. Drive it in the real app.**

## Step 5b: why it has to touch the engine, and what it needs

Everything so far is inert and additive — blending is off by default, nothing reads the settings,
and normal playback has been measured unchanged (drift 0.00 s, starved 0.00 s) after each change
to the callback. **5b is the first step that cannot be additive**, which is why it is worth
starting fresh rather than at the end of a long session.

The obstacle is that `Engine::run` owns track sequencing: it plays a track, waits for
`track_ended`, then plays the next from `queue`. If `AudioThread` performed a handover by itself,
the engine would still see the old track end and would call `play()` for the next one — replacing
the incoming track that is already audible. So the engine has to be the one that knows.

The shape that fits:

1. **`engine` needs its own blend config.** `app/src-tauri/src/settings.rs` cannot be a dependency
   of `crates/engine` (wrong direction), so mirror the four values into a small `BlendConfig`
   there and add `Engine::set_blend`, applied from `native.rs::attach` next to the existing
   `set_volume` / `set_output`.
2. **Prefetch on a worker thread**, not the audio thread. `audio::prefetch` is a blocking decode;
   the audio thread must not do it. Trigger it once the current track has enough left to be worth
   preparing for, from the head of `queue`.
3. **Decide, once.** `BlendConfig::rate_for(outgoing_tempo, prefetched.tempo)`. `None` — or mode
   Off, or remote mode active — means do nothing at all and let the normal transition happen.
4. **Start it** at `blend.seconds()` before the end, via a new `Command::StartBlend`.
5. **Complete the handover** when `Player::blend_done()`: rebuild an ordinary player for the
   incoming track at `Player::blend_position()`, and tell the engine so it can pop the queue and
   emit `TrackStarted`.

### The handover cannot be gapless without the decode thread outliving the track

Traced before writing any of 5b, because it changes the shape of it. The blend itself is solved:
the incoming track fades in from RAM over the outgoing one. The problem is what happens **after**
the fade, when the RAM buffer has to give way to a live stream.

Three approaches, and why the first two fail:

1. **Drop the player and rebuild at `blend_position()`** — the existing disposable-player
   handover. This leaves a ~200-400 ms hole while the new stream opens and seeks. That gap
   exists today between every pair of songs and nobody minds, because it lands in silence. A
   blend *moves* it to eight seconds into the incoming song, interrupting music instead. Worse
   than not blending.
2. **Build the replacement `Player` during the blend.** A `Player` owns a `cpal` stream, so this
   means two output streams. On one endpoint in shared mode they do not drift — both are clocked
   by the same audio engine — but they start at an arbitrary buffer phase, giving a fixed offset
   of up to one device period (~10 ms at 480 frames). Fine for a plain crossfade; for beat
   matching that is the whole error budget spent on nothing, since DJs aim under 5 ms.
3. **Keep the one player and swap what feeds the primary voice.** The only one that is both
   gapless and sample-exact.

So 5b needs `Player::continue_with(url, at)`: open a decoder for the incoming track seeked to
where the RAM buffer has reached, spawn a decode thread with a fresh ring buffer, and hand the
new consumer to the callback through the same one-slot queue the blend already uses. The callback
crosses from the blend voice to the refilled primary over ~50 ms, which is short enough to be
inaudible and long enough to hide a seek that did not land exactly where it was asked.

`master`'s `8d07d6f` (report where the seek landed, not where it was aimed) is what makes that
alignment checkable rather than assumed — Media Foundation seeks to a nearby boundary, not to the
sample requested.

The counters in `Shared` — position, buffered, decoded, starved — have to be reset as part of the
swap, since they describe a track and the track is changing. That is safe to do from the control
thread precisely because the outgoing decode thread has already exited: a track that ended is the
only condition under which this handover ever runs.

Things that will bite:

- **The watchdog must be quiet during a blend.** `audio_thread.rs` treats a drained queue and a
  motionless position as a stall, and both are *normal* while the outgoing track finishes. Every
  `reason` branch needs to be suppressed for the duration.
- **`track_ended` must not fire mid-blend**, or the engine advances underneath the handover.
- **Skip during a blend** has to cancel it, not stack a second one.
- **Lyrics, SMTC and the title should switch at the crossover midpoint**, not at either edge, or
  the app will name a song that is barely audible yet.
- **Beat phase is measured from each player's stream start**, not the track's. A rebuilt player
  seeks, so its phase is relative to that seek — the alignment maths has to use the same origin
  the position uses, or the beats will be confidently wrong.

## How the handover should work (step 5), and why

The tempting shape — teach `Player` to own two tracks properly — means two of everything in
`Shared`: two positions, two buffer depths, two decode-error flags. Every accessor then has to
answer "which track?", and the watchdog in `audio_thread.rs` currently assumes exactly one.

There is a much better fit already in the codebase: **the player is disposable.** Every recovery
path — stall, decode error, device change, long pause — throws the `Player` away and builds a new
one at the position the listener reached. A blend can end the same way, and then the second track
never needs full accounting at all:

1. Well before the end, `prefetch` the next track into RAM (~30 s) and measure its tempo.
2. Decide: `Blend::rate_for` against the outgoing tempo. No match, or mode is Off → normal
   transition, nothing else happens.
3. At `blend.seconds()` before the end, hand the callback a second voice fed from **the RAM
   buffer**, at the matched rate, fading in on an equal-power curve while the primary fades out.
4. Meanwhile — with seconds of slack, because the RAM buffer holds ~30 s and the blend uses ~8 —
   build an ordinary `Player` for the incoming track, seeked to where the RAM buffer will run
   out. This is the existing rebuild path, already well tested.
5. When the fade completes, swap that in as the new primary. The old track's `Player` is dropped,
   exactly as it is today at the end of a song.

So the second voice only ever needs to be *faded in from memory*. It needs no position, no buffer
health, no stall detection, and it cannot outlive the blend. All the accounting that already
exists keeps meaning precisely what it means now.

Two things to be careful of:
- The handover seek must land at or before where the RAM buffer ends, never after — `master`'s
  `8d07d6f` (report where the seek landed, not where it was aimed) is what makes that checkable,
  and a seek that overshoots would skip audio while one that undershoots merely repeats a moment.
- `Engine`/SMTC/lyrics should switch to the incoming track at the crossover midpoint, not at
  either edge, or the title will disagree with what is loudest.

Optional later: cache BPM per `pandoraId` on disk, following the lyrics-cache pattern
(`lyrics.rs:67`, `app_cache_dir()/lyrics/{hash}.json`), so a repeat play needs no analysis at all.

## Decisions taken while building

- **Out-of-range tempos get no blend at all**, rather than a plain crossfade or a reordered
  queue. Confirmed with Cameron. Pandora's sequencing is left alone; a station that occasionally
  doesn't blend reads as intentional, where a reordered one quietly becomes a different station
  and trends toward flat, tempo-homogeneous runs.
- **Equal power is interpolated in the square, not as a shaping of `t`.** The classic sin/cos
  pair only holds level if the fade-*out* gets the cosine, and `from + (to - from) * s(t)` gives
  the curve no direction to choose by — written that way it sagged 1.25% by the third frame.
  `√(from² + (to² - from²)·t)` is direction-agnostic and exact. See `Curve::gain`.
- **A dry voice leaves the output buffer alone** rather than writing silence. It shares a buffer
  with the other voice, so erasing is not neutral: a stall on the outgoing song would have muted
  the incoming one too.
- **Half and double time count as already matched** (`Blend::rate_for`). Candidates are
  `outgoing · 2ᵏ / incoming` for k in −1, 0, 1. Doubling the *rate* is never the answer — that
  raises the pitch an octave.
- **Blending defaults to Off.** It changes how every song ends; that should be asked for.

## The next step, in detail (step 2)

`Player` currently owns: a decoder thread, one `rtrb` ring buffer, one `cpal` stream, and a
callback that pops samples and applies the volume ramp. It needs to become:

- **Source** — decoder thread + ring buffer + `Shared` atomics. Roughly today's `Player` minus
  the cpal stream. One per track.
- **Output** — one `cpal` stream and the mixer. Owns a small fixed array of `(Voice, Consumer)`
  slots and renders them into an `f32` scratch buffer, then applies the master volume ramp once
  and converts to the device's sample format.

**Handing a new source to a running callback** is the part with a trap in it. The callback may
not lock or allocate. Use a second `rtrb::RingBuffer` as a command channel — `rtrb` carries any
`T`, so a `RingBuffer<SourceHandoff>` moves the consumer and the `Arc<Shared>` across, and the
callback pops pending handoffs at the top of each invocation. Bounded, lock-free, no allocation,
and it reuses a dependency already in the tree.

Keep `native_rate_is_a_wire` honest throughout: single-track playback is very nearly all
listening and must not acquire interpolation smear to buy a feature that runs for eight seconds
between songs. When only one voice is playing at rate 1.0, the path through the mixer has to be
arithmetically identical to today's.

## Things not to do

- Don't try to change tempo by re-opening the decoder at a different rate — it cannot glide.
- Don't run two cpal streams and hope they stay in phase. They won't.
- Don't put locks or allocation in the cpal callback.
- Don't send `audioReceiptURL` pings for a pre-buffered track that has not been played.
- Don't enable any of this in remote (UPnP/WiiM) mode — we do not own the renderer's pipeline.

## Open questions for the user

1. **Blend duration** — fixed (e.g. 8 s), or proportional to the tracks? Recommendation: a
   setting, defaulting to ~8 s, which is a normal radio crossfade.
2. **Should the incoming track glide back to its native tempo after the blend**, or stay at the
   matched tempo for its whole play? Recommendation: glide back — it is imperceptible and means
   errors don't accumulate across a long listening session.
3. Should blending apply on **skip** as well as natural end-of-track? Recommendation: no. A skip
   is an impatient act and should feel immediate.
