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
- [ ] **2. Wire the mixer into `Player`** — the Source/Output split. **Next, and the risky one.**
- [ ] **3. Pre-buffer the next track into RAM** (~30-45 s) from the queue the engine already keeps
      topped up (`MIN_QUEUED = 2`). Analyse it for BPM and phase.
- [ ] **4. Beat phase in `TempoTracker`.**
- [ ] **5. Blend orchestration** in `AudioThread` — start the incoming voice, run the crossfade,
      apply `rate_for`, glide back to native afterwards.
- [ ] **6. Drive it in the real app.**

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
