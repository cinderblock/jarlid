# Technical footer, and the BPM behind it

## Goal

Put a quiet technical readout in the **bottom-left** corner of the player, opposite the
existing `#version` / update badge at bottom-right. It answers "what is actually playing,
and is it playing well?" — including the **BPM of the current song**, which is the part
that has to be computed rather than looked up.

## Environment / context

- Repo: `Pandora` (the app is **Jarlid**), worktree `C:\Users\camer\.t3\worktrees\Pandora\t3code-838673ea`,
  branch `t3code/838673ea`, primary branch `master`.
- Tauri 2 app. Frontend is **vanilla TS** in `app/src` (no framework). Rust in
  `app/src-tauri` plus a three-crate workspace in `crates/` (`pandora`, `audio`, `engine`).
- JS tooling is **Bun** (`app/bun.lock`).
- Windows-only in practice: audio decoding is Media Foundation, `#[cfg(windows)]`-gated in
  `crates/audio/src/lib.rs:10-18`.

## Decisions already made (don't re-ask)

1. **The readout shows BPM + stream facts + live buffer health.** Chosen over "BPM only"
   and "BPM + stream facts". So it is a live strip refreshed on a ticker, not a static
   per-track label.
2. **Always visible, faint.** Mirrors `#version` exactly — `var(--faint)`, `0.65rem`,
   `position: fixed`, `bottom: 8px`, tabular numerals. Not behind a setting, not
   click-to-expand.
3. **BPM is computed locally from decoded PCM.** Pandora's API carries no tempo data of any
   kind (see Findings), so there is no lookup shortcut and no external service involved.
4. **No new crate dependency for the DSP.** The workspace currently has zero DSP libraries
   and the codebase visibly prefers it that way (`crates/audio/Cargo.toml:31-33` keeps a
   probe in `examples/` purely so `reqwest`/`tokio` don't leak into the library's deps). The
   detector is an energy-envelope + autocorrelation design that needs no FFT.
5. **Codec and bitrate come from Media Foundation's native media type, not from
   `Track::audio_encoding`.** See Findings — the Pandora label is knowingly stale.
6. **No `title=` attributes** (global rule). Everything in the strip is inline text.

## Plan / steps

### Rust — measuring

1. **`crates/audio/src/tempo.rs`** (new). `TempoTracker` + `Tempo { bpm, confidence }`.
   Pure DSP, deliberately *not* `#[cfg(windows)]`-gated so its tests run anywhere.
2. **`crates/audio/src/media_foundation.rs`** — read `GetNativeMediaType` to expose what the
   container actually holds: codec subtype, nominal bitrate, source sample rate, channels.
3. **`crates/audio/src/lib.rs`** — export `Source`, `Tempo`, `TempoTracker`.
4. **`crates/audio/src/player.rs`** — feed the tracker from the decode thread at the point
   PCM becomes `i16` (`player.rs:303-310`); publish the result through `Shared` atomics,
   the way `volume` already stores an `f32` as bits. Expose `Player::tempo()` /
   `Player::source()`.

### Rust — publishing

5. **`crates/engine/src/audio_thread.rs`** — add BPM/confidence/source to `Published`.
   **Latch** the BPM across player rebuilds (a stall mid-song must not blank the readout for
   another 15 s) and clear it on `Play`/`Stop`.
6. **`crates/engine/src/lib.rs`** — `Engine::tempo()`, `Engine::source()`.
7. **`app/src-tauri/src/native.rs`** — a second ticker emitting `engine://technical` at 1 Hz,
   modelled on the existing playhead ticker at `native.rs:224-249` but ten times slower,
   since none of this needs lyric-grade smoothness.

### Frontend

8. **`app/index.html`** — `<div id="tech"></div>` as a body-level sibling of `#version`.
9. **`app/src/styles.css`** — mirror the `#version` rule (`styles.css:387`) to bottom-left.
10. **`app/src/main.ts`** — `listen("engine://technical")` + a render function.

### Docs

11. README feature bullet.

## Findings / gotchas

### Pandora has no tempo data at all

`crates/pandora/src/models.rs:152-196` is the whole `Track`. There is no BPM, tempo, key,
energy or any other musicological field. The `key: Option<String>` field is **not** a musical
key — `models.rs:186-188` documents it as an XOR mask for obfuscated audio. Music Genome
features are internal to Pandora and not exposed on the tuner playlist item. So: DSP or
nothing.

### `Track::audio_encoding` is a known-stale label — do not display it

`crates/pandora/src/client.rs:182-185`:

> `audio_encoding` is left as Pandora reported it rather than guessed at: it describes the
> `audioUrlMap` stream, and with a preference list we cannot tell which spec was granted
> without measuring. Nothing depends on the label — the decoder sniffs the container — so an
> honest stale value beats a confident wrong one.

We request `HTTP_192_MP3,HTTP_128_MP3,HTTP_64_AACPLUS_ADTS` (`client.rs:34`) and normally get
128 kbit/s MP3, while `audio_encoding` may still say `"aacplus"`. Putting that string in a
technical readout would be exactly the "confident wrong value" the comment warns about.
**Use `IMFSourceReader::GetNativeMediaType` instead** — it reports the real subtype and
`MF_MT_AUDIO_AVG_BYTES_PER_SECOND` for the stream actually being decoded.

### BPM cannot be instant — the decoder is throttled to ~playback speed

`TARGET_BUFFER = 5 s` (`crates/audio/src/player.rs:77`), the ring buffer is sized to exactly
that (`:234-237`), and the decode thread *sleeps* when it is full (`:293-297`). So decoding
runs at wall-clock rate plus a 5 s lead — it is a stream, not a pre-fetch.

Consequences, all deliberate:
- The first estimate lands ~10 s into a track; it firms up over the next few seconds.
- The UI shows `♩ —` until then. A reserved slot, not a hidden one, so nothing shifts when
  the number arrives.

Two rejected alternatives:
- **A second faster-than-realtime decode pass.** `Decoder::decode_all()` already does this
  and would give a BPM in a second or two. But it re-fetches an expiring signed URL
  (`models.rs:166-167` — "treat as a live credential") and Pandora allows only one concurrent
  stream per account (`crates/pandora/src/client.rs:13`), which is the `STREAM_VIOLATION`
  failure the app already has recovery code for. Not worth it for a footer.
- **Raising `TARGET_BUFFER`.** The comment at `player.rs:75-76` says the 5 s was tuned against
  pause/skip responsiveness. Not a knob to turn for this.

### The tap point, and the one place not to tap

Tap **`crates/audio/src/player.rs`, decode thread, immediately after `pending.extend(...)`**.
`pending` is cleared on the line above, so the new samples are exactly `&pending[..]`. That
thread is non-realtime, sees every sample exactly once and in order, and already exists.

Do **not** tap the cpal callback (`player.rs:354-416`) — `player.rs:7-11` records that an
earlier `Mutex` in that path produced "continuous scratchiness". Any DSP there is an audible
defect.

Cost note: the analysis is ~2-4 ms of tight f32 multiply-add, run once per second, on a thread
holding MMCSS priority. Against `DECODE_STALL = 3 s` (`audio_thread.rs:31`) that is a ~1000×
margin, and the thread is blocked on network reads most of the time anyway.

### Sample rate is the *device's*, not the source's

`player.rs:215-224` asks Media Foundation to decode straight to the output device's rate, so
the tapped PCM is typically 48 kHz even though Pandora sends 44.1 kHz. **Never hardcode
44100** in the analyzer — derive everything from `format.sample_rate`. Resampling preserves
tempo, so the BPM is unaffected; but `player.rs:216-217` notes that unconverted 44.1→48
playback runs ~8.8% sharp, which would have skewed a naive detector by the same 8.8% had the
tap been upstream of the resampler.

Channel count is also not guaranteed to be 2 — downmix generically over `format.channels`.
Chunks from `next_chunk()` are arbitrary byte counts, so a chunk boundary can fall mid-frame;
the tracker carries partial-frame state across calls.

### A `Player` is disposable; a track is not

`crates/engine/src/audio_thread.rs:8-12` — the player is rebuilt on stall, decode error,
device error, device change, or a long pause, each time re-decoding from `position`. So an
analyzer living inside `Player` restarts mid-song and re-sees overlapping audio.

Resolution: the tracker is **per-player** (correct, no overlap corruption), but the
**published BPM is latched at the `AudioThread` level** and only cleared on `Play`/`Stop`. A
rebuild therefore keeps showing the last good number while the new tracker warms up.

### Detector design

Envelope: downmix → 3 bands via two one-pole lowpasses (200 Hz, 4 kHz) → per-band RMS per hop
→ log compression → half-wave-rectified flux, summed across bands. Hop targets a ~200 Hz
envelope rate derived from the actual sample rate.

Estimation: centred-moving-mean adaptive threshold, then unbiased autocorrelation (normalised
by overlap length — a raw ACF tapers with lag and would bias toward fast tempos), then
**harmonic summing** `acf(L) + ½acf(2L) + ¼acf(3L)` to prefer the fundamental over the
half-tempo, a gentle log-normal prior centred on 120 BPM, and parabolic interpolation around
the winning lag so the reported BPM is not quantised to the envelope grid.

Confidence is the normalised autocorrelation at the winning lag — a real correlation
coefficient, not an invented score. Below `MIN_CONFIDENCE` the tracker reports nothing rather
than guessing.

### The octave error, in three acts — read this before touching the scoring

This is the single hardest part of the feature and it was got wrong twice. The scoring now is
`harmonic sum − subdivision penalty, times a log-normal prior`, and each term is there because
removing it reproduces a specific failure.

**Act 1 — plain harmonic summing halves fast tempos.** Scoring `acf(L) + ½acf(2L) + ¼acf(3L)`
failed: a 174 BPM click track read as exactly 87. The reason is arithmetic, not tuning — if `L`
is the true period then every multiple of `2L` is also a multiple of `L`, so the candidate `2L`
sums the very same peaks and ties exactly. The tie then fell to the 120 BPM prior, which at 87
vs 174 leans the **wrong** way. No reweighting of harmonics can fix this.

**Act 2 — subtracting the midpoint fixes that and breaks everything with a hi-hat.** Scoring
`Σ wₖ·[acf(k·L) − acf((k+½)·L)]` fixed the click track: at the true period the midpoint falls
between beats and is a trough, while at twice the true period the "midpoint" 1.5L lands on a
real beat and the difference collapses. All synthetic tests passed, so it shipped to the app.

It was wrong. It is the same mechanism seen from the wrong end: as soon as a track has
**subdivisions** — a hi-hat on every off-beat — the *true* beat's own midpoint at 1.5L lands on
a hat, so the true tempo gets penalised and the estimate runs away to the fastest subdivision.
Caught only by running the real app: **Mr. Saxobeat (~128 BPM) read `~64` for an entire song.**
A synthetic drum pattern with a backbeat and off-beat hats then reproduced it offline, and also
exposed the mirror failure — a 100 BPM pattern reading 200.

**Act 3 — penalise the subdivisions, not the midpoints.** `score(L) = Σ wₖ·acf(k·L) −
β·[acf(L/2) + ½·acf(L/3)]`. This asks *"is there a faster pulse I should have picked?"* rather
than *"does anything happen between my beats?"*, which is the actual definition of the beat: the
slowest pulse that is not merely a subdivision of a faster one. All three failures now guard each
other as tests.

The prior also had to tighten from 0.9 to 0.55 octaves. At 0.9 it was decorative; the comb can
rank unrelated periods but genuinely cannot choose between a tempo and its double, because both
*are* periodic — only a preference for the range people count breaks that.

**Do not "simplify" the scoring back to a plain harmonic sum, and do not reintroduce midpoint
subtraction.** Both look cleaner and both are wrong, in opposite directions.

### Harmonic summing alone does NOT fix the octave error (superseded — see above)

First implementation scored candidates as `acf(L) + ½acf(2L) + ¼acf(3L)`, which is the textbook
"harmonic sum". It failed:

```
---- tempo::tests::finds_an_even_pulse stdout ----
174 BPM click track read as 87.0 BPM
```

Exactly half, and the reason is arithmetic rather than tuning: if `L` is the true period then
`2L`, `3L`, … are all peaks — but *every multiple of 2L is also a multiple of L*, so the
candidate `2L` sums the very same peaks and scores identically. The tie then falls to noise and
the 120 BPM prior, which at 87 vs 174 BPM actually leans the **wrong** way (0.875 vs 0.838).
No amount of reweighting the harmonics fixes this.

The fix is to score `Σ wₖ·[acf(k·L) − acf((k+½)·L)]` — subtract the value halfway to the next
harmonic. At the true period the midpoint falls *between* beats and is a trough, so the
difference is large; at twice the true period the "midpoint" 1.5L lands on a real beat, so the
difference collapses. Include a peak term only when its midpoint is also in range, or the
imbalance quietly reinstates the bias.

`accented_beats_do_not_halve_the_tempo` (loud/soft beats at 128 BPM, where a plain ACF prefers
64) is the regression test for this and is the single most valuable test in the file.

### Measured against real music, not just click tracks

`crates/audio/examples/bpm.rs` runs the real `Decoder` at the player's 48 kHz stereo and prints
the estimate every 5 s, so a wandering answer is visible instead of averaged away. Results:

Results with the final (Act 3) scoring:

| Track | Result | Verdict |
|---|---|---|
| PioneerDJ Demo Track 1 | **127.9** BPM, conf 0.59 | 0.1% off a round 128 |
| PioneerDJ Demo Track 2 | **120.1** BPM, conf 0.79 | exact |
| human gazpacho — Blink Dogs | 109.2 BPM, conf 0.79 | plausible, strongly locked |
| Brylie Christopher — A Gentle Fog Descends | 115.2 BPM, conf 0.22 | ambient; correctly hedged |
| Rod — Inner Rhythm (23 min) | 116.5 BPM, conf 0.44 | long mix, tempo varies; hedged |
| Alice's Restaurant Massacree | **no tempo found** | 18 min of spoken word — correct refusal |

The last row is the one worth keeping: refusing to answer for speech is the behaviour that stops
a confident number appearing under every podcast-like track.

`Source` detection was validated on the same files — VBR averages (149k, 241k), CBR (192k, 256k,
320k), and a 48 kHz source that needs no resampling all read correctly.

### Sparse intros read the *dotted* pulse — fixed by latching on confidence

Both demo tracks read two-thirds of their true tempo during their drumless intros (85.2 against
a real 127.6; 80.0 against a real 120.0) — the dotted-note pulse, a 3:2 ambiguity that harmonic
summing does not address because it only folds 1:2 and 1:3 together. Demo Track 2 also slipped
back to 80.0 during a breakdown at 107 s.

Confidence separates the two readings cleanly and consistently (0.36-0.43 wrong vs 0.57-0.77
right), so `TempoTracker` now keeps **the most confident reading of the track so far** rather
than the most recent. Demo Track 2 then locks at 120.0/0.77 at 76 s and never moves again.

Trade-off accepted: a genuine mid-track tempo change is not followed. For a per-track readout on
a radio stream that is the right call; a tracker that had to follow one would need to age the
latch out.

This calibration is also where the UI's `BPM_HEDGE = 0.45` comes from — confident readings
cluster 0.51-0.79 and ambiguous ones 0.22-0.44, so the threshold sits in a real gap rather than
being a round number someone liked.

## Progress log

- [x] Confirm Pandora exposes no BPM — it does not; must be DSP.
- [x] Locate the decode tap and confirm it is safe (non-realtime, lossless, in order).
- [x] Establish that codec/bitrate must come from MF, not `audio_encoding`.
- [x] `crates/audio/src/tempo.rs` + unit tests
- [x] MF native-media-type `Source`
- [x] Player tap + atomics
- [x] AudioThread publish + latch
- [x] Engine accessors
- [x] `engine://technical` event
- [x] Frontend element, CSS, render
- [x] `cargo test` / `cargo clippy` / typecheck
- [ ] Drive the real app and read the strip against a song of known tempo
- [ ] README
- [ ] Commit

## Open questions for the user

1. None outstanding. (Content and visibility were settled up front — see Decisions.)

## Things not to do

- Don't display `Track::audio_encoding` as the codec. It is knowingly stale.
- Don't do DSP in the cpal callback.
- Don't hardcode 44100 — the tap is downstream of MF's resampler.
- Don't add an FFT crate; the design does not need one.
- Don't open a second stream on the track's `audio_url` to get an early BPM.
- Don't use `title=` attributes for any of this.
