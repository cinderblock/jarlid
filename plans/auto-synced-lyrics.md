# Auto-synced lyrics: generate timings locally when LRCLIB has none

## Goal

When LRCLIB has no synced lyrics for a track, generate them on the user's machine from
the audio we're already streaming, using a speech model (Whisper-class) plus whatever text
we have: title and artist always, LRCLIB's plain lyrics when they exist.

The speech core is an **optional, downloaded-on-demand component.** Nothing ships in the
installer. Turning the feature on in Settings downloads the runtime and the model; turning
it off can delete them.

Requested 2026-10-13: "embed something like Whisper, combined with song title and
optionally lyrics … use the stream of audio we have to look ahead and get missing lyrics
live as we listen", followed by "make the whisper core an optional feature that downloads
the binary blob it needs only if the user wants to use the auto lyrics".

## Environment / context

- Lyrics are fetched in `app/src-tauri/src/lyrics.rs` (`fetch_lyrics`, LRCLIB). The
  `Lyrics` struct has `synced`, `plain`, `source`, the record identity, `overridden`,
  `stale` and `pinned`. Local edits are stored as overrides (`lyrics_override_path`).
  LRCLIB "none" results are cached briefly.
- Frontend: `app/src/main.ts` `loadLyricsFor` → `fetch_lyrics`. Synced lyrics light up a
  line at a time; plain lyrics drift with `plainFraction`. `lrc.ts` parses LRC.
  `lyric-editor.ts` already has a timing pass (tap-to-stamp) and LRCLIB publishing.
- **The audio is available ahead of time, and fast.** `crates/audio/src/prefetch.rs`
  measured Pandora's CDN at about 120x realtime (769 KB in 0.4 s). `engine::cache_next`
  already downloads the *next* track whole while the current one plays (when the opt-in
  disk cache is on). `engine::prepare_blend` already decodes the start of the next track
  on a blocking thread.
- `audio::Decoder::open_at(url, Some(Format { sample_rate: 16000, channels: 1,
  bits_per_sample: 16 }))` should hand us Whisper-ready 16 kHz mono PCM, because Media
  Foundation inserts its own resampler. Not yet verified at 16 kHz mono.
- Speaker (UPnP) mode: the speaker plays from the URL, but we still hold the URL, so
  analysis works there too.
- Windows only (Media Foundation decoding). Tauri v2 updater with a minisign key in
  `tauri.conf.json`. Release CI is in `.github/workflows/release.yml`.

## Decisions already made (don't re-ask)

Answered 2026-10-13:

- **Sidecar executable**, not an in-process library (§5).
- **v1 aligns LRCLIB plain lyrics only.** Transcription-from-nothing comes later, but the
  design must keep it easy to add: sources, pipeline stages and the protocol are built so
  that more modes and stages can be added without rework.
- **Test CPU and GPU now**, in the spike, rather than deferring GPU.
- **Downloading the next song whole is fine**, and that download **is what plays**: one
  complete download replaces the stream, the way `engine::cache_next` already works. With
  the song cache off, the file is transient: it is deleted once played (or skipped).
  With the cache on, it is simply the cache entry.
- **"Whisper" in the original ask meant Whistle** (Cactus Compute,
  <https://cactuscompute.com/blog/whistle>), clarified 2026-10-13 mid-spike. So Whistle is
  benchmarked alongside Parakeet and Whisper, on the same corpus and harness, and the engine
  is picked on the numbers.
- Benchmark harness first (§4), sidecar with hash-pinned on-demand download (§5),
  "Review & publish" through the timing editor, never auto-publish (§6): all approved.

## Key design points

### 1. Look ahead a whole track, not "live"

Running inference live alongside the playhead is the hard way to do this. We can fetch a
whole song in a second or two, so:

- **Next track:** when the queue's next track has no synced lyrics, fetch and analyse it
  while the current song plays. A typical song gives 3–4 minutes of budget, and its lyrics
  are ready when it starts.
- **Current track** (first song, skips, turning the feature on mid-song): decode the whole
  thing at once and analyse it in windows starting from the beginning, publishing lines
  progressively. If inference beats realtime it stays ahead of the playhead. Until a line
  is reached, the pane shows plain lyrics with a "syncing…" status.
- Alignment is better done over the whole track (global context), which favours this
  shape anyway.

### 2. Two modes, very different quality

| We have | Do | Expected quality |
| --- | --- | --- |
| LRCLIB **plain** lyrics | **Align** the known text to the audio | Good. The words are right; only the timing is generated. |
| Nothing | **Transcribe** with title/artist as the prompt | Mediocre on sung vocals. Words will be wrong. Label it clearly. |

Alignment is the main prize. Transcription-from-nothing is a stretch goal, behind its own
toggle, labelled "auto-transcribed".

### 3. The model pipeline, in stages so each can be measured

1. **Decode** to 16 kHz mono (MF, see above).
2. **Vocal isolation** (optional stage, phase 2). An MDX-Net/UVR vocal model in ONNX.
   Published evidence is mixed: on full mixes Whisper hallucinates; with a poor separator
   it degrades. One paper finds the best results come from using the vocal stem only as a
   *voice-activity detector*, segmenting the original mix with it.
3. **ASR** with word timestamps: whisper.cpp (ggml). `large-v3-turbo` quantised is the
   likely quality/speed point; `small`/`base` for slow machines.
4. **Align**:
   - Phase 1: whisper transcript words ↔ known lyric words by sequence alignment
     (Needleman–Wunsch on normalised words). Matched words give timestamps; each line takes
     its first matched word's time, and unmatched lines are interpolated.
   - Phase 2, if phase 1 isn't good enough: CTC forced alignment of the known text on the
     vocal stem (wav2vec2 / MMS_FA class, ONNX). Practitioners report Whisper word times
     on singing drift **early**, by 1–3 s between overlapping windows, and that CTC
     forced alignment on the vocal stem fixes it.
5. **Quality gate.** Score every result: share of lines matched, median timing
   confidence, monotonicity. Below the threshold, don't show it as synced; fall back to
   plain.

### 4. A free accuracy benchmark

Every track that *does* have LRCLIB synced lyrics is labelled test data. Hide the synced
copy, align the plain copy against the audio, and compare per-line start times. That
gives a real number for "median line error" and "% of lines within 400 ms" to tune
against and to set the quality gate with. Build this harness before tuning anything.

### 5. Optional component: how the binary gets onto the machine

Recommended: **our own sidecar executable**, built in our release CI and downloaded on
opt-in.

- A small Rust binary (working name `jarlid-sync`) that links whisper.cpp (via
  `whisper-rs`) and later ONNX Runtime. It speaks a JSON-lines protocol on stdio: the app
  sends PCM plus text, and the sidecar returns progress events and timed lines.
- **Why a sidecar rather than an in-process DLL:** GPU and driver crashes can't take
  playback down; it can run at below-normal priority without touching the MMCSS decode
  thread; we can swap CPU/Vulkan builds without rebuilding the app; and the model stays
  loaded across songs, which per-song `whisper-cli` spawns wouldn't do.
- **Why our own build rather than upstream whisper.cpp release zips:** a stable protocol
  that we control, one artefact covering ASR + alignment + separation, and provenance (we
  build it, we pin it).
- **Integrity:** each app version pins the exact SHA-256 of the sidecar and model files it
  expects. Download over HTTPS into `app_data_dir()/auto-lyrics/`, verify, then use.
  Models come from Hugging Face, pinned by hash, or are mirrored to our GitHub releases.
- **Size, roughly:** sidecar CPU build is a few MB, a Vulkan build tens of MB;
  `large-v3-turbo` q5 is about 550 MB, `small` about 190 MB, `base` about 60 MB. Show the
  size before downloading.
- **Uninstall:** a "Remove downloaded files" control in Settings.

#### Sidecar protocol (draft)

One long-lived process per app session, started on first use and stopped after idling.
JSON lines over stdin/stdout; stderr is logged. PCM goes through a temp file, not the
pipe: 16 kHz mono s16 WAV, written by the app (MF decode) and deleted afterwards. That
keeps the protocol text-only, and lets the sidecar re-read the audio for a second pass.

```text
→ {"op":"hello","protocol":1}
← {"ev":"ready","protocol":1,"engines":["parakeet","whisper"],"backends":["cpu","vulkan"]}
→ {"op":"load","engine":"parakeet","model":"<path>","backend":"vulkan","threads":4}
← {"ev":"loaded","ms":2100}
→ {"op":"job","id":"<pandoraId>","wav":"<path>","mode":"align","lines":["…","…"],
   "language":"en","from":0.0}
← {"ev":"progress","id":"…","heardTo":61.2}
← {"ev":"lines","id":"…","lines":[{"i":0,"t":12.31,"matched":5,"words":5}, …]}   (partial)
← {"ev":"done","id":"…","coverage":0.82,"score":…}
→ {"op":"cancel","id":"…"}
```

- `mode` is `align` in v1. `transcribe` slots in later without a protocol change: no
  `lines`, and the result carries text. Same for future stages (`separate`), which become
  options on `job`.
- Alignment runs **in the sidecar** (the `autosync` crate links into it), so partial
  results stream back as soon as there's enough heard text to anchor lines.
- `from` lets the current track start near the playhead rather than at 0:00 when that
  matters.

### 6. Storage, precedence, UX

- Generated timings are cached per track like LRCLIB results, but as their own source
  with provenance (`auto-aligned` / `auto-transcribed`, model name, score).
- Precedence: local override > pinned record > LRCLIB synced > **auto-aligned** > plain >
  **auto-transcribed** (opt-in) > nothing.
- Lyrics pane status line: "Auto-synced" with an indicator; "Syncing… 1:20" while working.
- **Never auto-publish to LRCLIB.** Offer "Review & publish" by opening the existing
  timing editor pre-seeded with the generated stamps. That turns the generator into a
  fast way to contribute real synced lyrics back.
- Settings: an app-drawn toggle (no native controls, no `title=`), model choice with sizes,
  download progress, CPU/GPU selection, and "remove files".

### 7. Resource contention

`plans/audio-dropouts-under-load.md` exists for a reason. Inference must not starve the
decode thread: run the sidecar at below-normal priority with a capped thread count, and
pause analysis if the device-buffer readout shows underruns climbing.

## Plan / steps

1. **Spike, measure before building** ← *current, pending your go-ahead*
   - Decode a Pandora track to 16 kHz mono through MF; dump it to a WAV.
   - Run it through stock whisper.cpp (`base`, `small`, `large-v3-turbo` q5; CPU and
     Vulkan) on this machine. Record speed vs realtime and word-timestamp quality.
   - Build the benchmark harness (§4) as a dev example over ~20 tracks that have LRCLIB
     synced lyrics. Measure phase-1 alignment error, with and without vocal isolation.
2. Sidecar crate + JSON protocol + CI build + pinned hashes.
3. Download manager + Settings UI (opt-in, progress, remove).
4. Engine hook: analyse the next track ahead; analyse the current track progressively.
5. Lyrics integration: new sources, cache, precedence, pane status, quality gate.
6. "Review & publish" through the timing editor.
7. Phase 2 quality work, as the benchmark dictates: vocal isolation, CTC aligner.
8. Transcription-from-nothing mode (opt-in), if quality is tolerable.
9. README section.

## Findings / gotchas

- Research summary (2026-10-13): aligning *known* lyrics is far more tractable than
  transcribing. Whisper word timestamps on singing land early, even with DTW. CTC forced
  alignment on a vocal stem is the common fix. Practitioners gate results with an
  independent check, e.g. median start error ≤ 400 ms and ≥ 70% of lines within 400 ms.
  Sources: sydlexius/canticle#482, atomantic/PortOS#10610, zbynekdrlik/songplayer#143,
  arXiv 2506.15514 (source separation for Whisper ALT).
- `condition_on_previous_text = false` is the usual anti-repetition-loop setting for
  Whisper on music.
- **whisper.cpp official Windows assets** (release `b5454` = v1.9.5, 2026-10-06): CPU x64
  (8.9 MB zip, runtime-dispatched `ggml-cpu-<arch>.dll` variants), BLAS, and CUDA 11.8/12.4
  (285 MB / 685 MB). **No Vulkan build for Windows.** DTW flag is `--dtw <preset>`, presets
  `base`, `small`, `large.v3.turbo` etc.
- **whisper.cpp now ships Parakeet** (`parakeet-cli`, `parakeet.dll`): NVIDIA
  `parakeet-tdt-0.6b-v3`, 25 European languages, ggml models at `ggml-org/parakeet-GGUF`
  (q4_0 356 MB, q8_0 669 MB, f16 1.26 GB). TDT emits per-token frame times with a
  `word_start` flag, at 10 ms resolution. That's a strong candidate for word timing, so it's
  benchmarked alongside Whisper.
- **Building whisper.cpp with Vulkan on Windows:** the SDK ships `SPIRV-HeadersConfig.cmake`
  flat in `$VULKAN_SDK/Lib/cmake`, which CMake's prefix search doesn't find
  (`CMAKE_PREFIX_PATH=$VULKAN_SDK` still fails with "Could not find a package configuration
  file provided by SPIRV-Headers"). Fix: `-DSPIRV-Headers_DIR=$VULKAN_SDK/Lib/cmake`.
  Result: `ggml-vulkan.dll` is 36 MB; everything else is about 3 MB.
- **MF decodes straight to 16 kHz mono 16-bit:** `Decoder::open_at(path, Some(Format { 16000,
  1, 16 }))` negotiated exactly that on all 56 corpus tracks. No resampler of our own needed.
- **Corpus** (`crates/engine/examples/autosync-corpus.rs`): 16 seed artists on anonymous
  stations gave 64 tracks, 56 of which have LRCLIB synced+plain lyrics within 2 s of
  Pandora's length. Spans rap (Eminem, Kendrick, Coolio), metal (Metallica, Pantera, SOAD),
  country, soul, pop and rock. 408 MB of 16 kHz WAV, kept outside the repo.
- **Sweep results** (one track, `coldplay-yellow`, 266 s; 37 scored lines; 4 threads;
  wall time includes model load):

  | Config | Wall | × realtime | Median err | Signed | p90 | ≤0.3 s | ≤1 s |
  | --- | --- | --- | --- | --- | --- | --- | --- |
  | parakeet q8 CPU, whole track | 160 s | 1.7 | 0.17 | +0.06 | 10.2 | 46% | 49% |
  | parakeet f16 Vulkan, whole track | 131 s | 2.0 | 0.18 | +0.05 | 12.9 | 57% | 59% |
  | **parakeet q8 CPU, 30 s windows** | 103 s | 2.6 | **0.13** | +0.06 | **1.26** | **78%** | 84% |
  | **parakeet f16 Vulkan, 30 s windows** | 97 s | 2.7 | **0.10** | +0.06 | 1.34 | 76% | 81% |
  | whisper base CPU | 48 s | 5.5 | 0.46 | 0.00 | 1.96 | 32% | 76% |
  | whisper base Vulkan | 76 s | 3.5 | 1.22 | −1.22 | 52.4 | 16% | 41% |
  | whisper small CPU | 133 s | 2.0 | 0.39 | −0.18 | 2.46 | 43% | 73% |
  | whisper small Vulkan | 177 s | 1.5 | 0.52 | −0.14 | 2.66 | 38% | 70% |
  | whisper large-v3-turbo CPU | 1072 s | **0.25** | 2.04 | −1.21 | 7.42 | 0% | 8% |
  | whisper large-v3-turbo Vulkan | 1392 s | **0.19** | 1.54 | −0.80 | 7.60 | 11% | 27% |

  Takeaways:
  - **Parakeet's word times are far more precise than Whisper's.** Where it heard a line,
    it's typically within 0.1–0.2 s (33.60 vs 33.80, 52.96 vs 52.82, 58.56 vs 58.58).
  - **Parakeet must be fed windows, not a whole song.** Whole-track, it stops transcribing
    reliably after about 3:00, and every later line is wrong (the p90 of 10–13 s).
    30 s windows with 6 s overlap fixed it and were faster too.
  - **The Intel iGPU barely helps Parakeet and hurts Whisper.** Vulkan's value on such
    machines is leaving the CPU free for playback, not speed. On a discrete GPU this will
    differ; measure it there before deciding the default backend.
  - **large-v3-turbo is unusable on a 15 W laptop CPU or iGPU**: 4–5× *slower* than
    realtime.
- **Whisper failure modes seen:**
  - Turbo looped "Music by Kevin MacLeod" for 2½ minutes over real singing: the classic
    conditioning-on-previous-text hallucination. `-mc 0` (max context 0) is the fix.
  - **`--dtw` is silently ignored while flash attention is on**, and whisper.cpp now
    defaults flash attention *on*: `whisper.cpp:3817` "dtw_token_timestamps is not
    supported with flash_attn - disabling". Every `t_dtw` came back −1. Pass `-nfa`.
  - **`--vad` broke timing**: with Silero VAD the token times were relative to the
    speech-only audio (errors of −80 to −160 s), and VAD discarded most of the singing
    (small ran in 29 s instead of 133 s). Don't use whisper.cpp's VAD on music as-is.
- **Full corpus, windowed Parakeet q8_0 on CPU, 4 threads** (56 tracks, 2,895 scored lines,
  13,288 s of audio in 4,674 s, **2.84× realtime**), aligner as of `441220c`:

  | Pool | Tracks | Median | p90 | ≤0.3 s | ≤0.5 s | ≤1 s |
  | --- | --- | --- | --- | --- | --- | --- |
  | All | 56 | 0.26 | 3.42 | 52% | 68% | 79% |
  | All, each track's LRCLIB offset removed | 56 | 0.15 | 2.53 | 67% | 74% | 81% |
  | Coverage ≥ 60% | 43 | 0.23 | 1.51 | 58% | 74% | 85% |
  | Coverage ≥ 60%, offset removed | 43 | 0.13 | 0.94 | 74% | 82% | 89% |

  - **Rap is the best case**, not the worst: Eminem *Guilty Conscience 2* 87% within 0.3 s,
    J. Cole 90%, Kendrick *Not Like Us* 84%. Dense, dry, centred vocals.
  - **Constant per-track offsets are LRCLIB's, not ours.** Bruno Mars *Talking to the Moon*
    is −6.46 s on every line, Pink Floyd *Breathe* +3.66 s, Journey −1.30 s: synced against
    another master. That's why "offset removed" is reported separately.
  - **Hard failures are ASR silence.** Radiohead *Let Down*: 12 of 13 windows gave
    "Segments (0)", with quiet, reverb-soaked vocals under a dense mix, so coverage was 2%.
    Metallica *Wherever I May Roam* got 30%, Stevie Nicks *Stand Back* 23%. The coverage
    gate catches these; vocal isolation is the candidate fix.
- **Aligner tuning (on the q8_0 corpus results):**
  - **One stray match anchored a whole line.** Mr. Brightside: "But it's just the price I
    pay" matched 1 of 7 words (an "it's" from the line before) and pinned itself 20 s
    early, dragging five interpolated lines with it. Rule: a line needs ≥2 matched words,
    or all of its words if shorter (`min_anchor_words = 2`). Overall p90 went from 3.42 s
    to 2.77 s.
  - **Lead:** Parakeet's word starts run +0.15 s behind LRCLIB on the corpus. With
    `lead = 0.15`, the median went from 0.26 to 0.19 s and ≤0.3 s from 52% to 59%. Larger
    leads were worse. Now the default (`ca6aaf3`).
  - `gap` −0.2/−0.6 and `match_sim` 0.5–0.8 made no meaningful difference. Left at −0.4
    and 0.6.
- **Quantisation and backend** (corpus, aligner as of `ca6aaf3`):

  | Config | Tracks | × realtime | Median | p90 | ≤0.3 s | ≤1 s | Pass 60% gate |
  | --- | --- | --- | --- | --- | --- | --- | --- |
  | q8_0 CPU, 4 threads | 56 | 2.84 | 0.19 | 2.75 | 59% | 78% | 43/56 |
  | q4_0 CPU, 3 threads | 56 | 3.05 | 0.22 | 5.47 | 54% | 72% | **34/56** |
  | f16 Vulkan (iGPU), 3 threads | 33 of 56* | 2.74 | 0.17 | 2.98 | 61% | 78% | 26/33 |

  On the same 31 tracks, q8_0 CPU and f16 Vulkan are **indistinguishable** (0.16 vs 0.17
  median, 65% vs 65% ≤0.3 s, 83% vs 83% ≤1 s). q4_0 matches them where it passes the gate,
  but fails it far more often. **Default: q8_0 (669 MB).** q4_0 isn't worth its
  ~300 MB saving.

  \* The 3-thread q4+Vulkan job hit the 2-hour background-task limit after 32 Vulkan tracks
  and was stopped, not restarted. Its orphaned `parakeet-cli` had to be killed by hand:
  `taskkill /T` on the broker's node process doesn't reach a grandchild started through
  `bash -c`.
- **Whistle** (Cactus Compute, released 2026-10-02): Apache-2.0, a single 16.9 MB
  `whistle.cact` plus a 1.5 MB `needle.exe` with a prebuilt `windows-x86_64` folder at
  `Cactus-Compute/needle3`. CPU only, no GPU path. English, German, French, Spanish,
  Italian, Dutch, Polish. Word timestamps come from decoder attention. It has
  `--audio-keywords` biasing and windows long audio itself. On `coldplay-yellow` at 1 thread
  (338 s for 266 s of audio):

  | Config | Coverage | Median | p90 | ≤0.3 s | ≤1 s |
  | --- | --- | --- | --- | --- | --- |
  | whistle, no keywords | 46% | 1.37 | 5.45 | 32% | 46% |
  | **whistle, lyric words as keywords** | 70% | 0.81 | 3.38 | 38% | 62% |
  | whistle, lyric lines as keywords | 68% | 1.38 | 5.94 | 24% | 49% |
  | parakeet q8_0 windowed (same aligner) | 72% | 0.13 | 1.11 | 70% | 84% |

  - **Keyword biasing is a big lever for alignment.** We know the words, so feeding them
    in lifts coverage from 46% to 70%. Whole lines did worse: the decoder echoes phrases
    back (quote marks and all) at times they weren't sung.
  - **Timing precision is the gap**, not recognition. Attention-derived word times are
    about 6× less precise than Parakeet's TDT frame times, the same weakness Whisper
    showed.
  - **Download size is its big advantage:** 18 MB against 356 MB (Parakeet q4_0) or
    669 MB (q8_0).
  - **Corpus verdict (31 of 56 tracks, 1 thread, 1.14× realtime; stopped there because the
    answer was clear):** on the same 30 scored tracks, Whistle passes the 60% coverage gate
    on **12/30 against Parakeet's 26/30**, with median 0.41 s against 0.15 s, ≤0.3 s 42%
    against 66%, and ≤1 s 64% against 83%. Where it does pass the gate it's respectable
    (median 0.21 s, ≤1 s 83%), but it would deliver synced lyrics for about 4 songs in 10
    where Parakeet manages 9. Its numbers use Parakeet's 0.15 s lead; Whistle's own bias
    is about 0.05 s, which doesn't change the verdict.
  - **Decision:** Parakeet v3 q8_0 is the engine. Whistle isn't worth offering even as a
    "lite" download: an option that fails on most songs isn't a feature. Revisit if
    Cactus ships a larger checkpoint or frame-level timestamps.
- **Engine choice so far:** Parakeet v3 is the primary candidate. It covers English and 24
  European languages, including Spanish, French, German, Italian and Portuguese. Whisper
  stays the candidate fallback for anything else (Japanese, Korean, Hindi …), pending a
  fixed-flags re-run.
- **Test machine:** i7-10710U (6C/12T, 15 W laptop part), Intel UHD (Comet Lake) iGPU with
  Vulkan 1.3, 64 GB RAM. A deliberately low-end target: if it's fast enough here, it's fast
  enough nearly everywhere.

## Progress log

- [x] Survey codebase: lyrics flow, audio look-ahead (`prefetch`, `cache_next`), decoder
      formats.
- [x] Literature / practitioner research.
- [x] Design written (this doc).
- [ ] Step 1 spike ← in progress
  - [x] whisper.cpp CPU build (official) + Vulkan build (local, from v1.9.5 source).
  - [x] Models: whisper base-q5_1, small-q5_1, large-v3-turbo q5_0 + f16; parakeet v3
        q8_0 + f16; silero VAD v5.1.2. SHA-256s recorded in the spike dir.
  - [x] Corpus: 56 labelled tracks.
  - [x] `crates/autosync`: text normalisation, word alignment (Needleman–Wunsch, free
        heard-word ends), line timing, LRC, scoring. `examples/bench.rs` scores model output.
  - [x] Speed sweep, 8 configs on one track (`coldplay-yellow`, 266 s). Results in
        Findings → "Sweep results".
  - [x] Fixes found by the sweep: Parakeet in 30 s windows; Whisper `-mc 0 -nfa`.
  - [x] Accuracy over the full corpus: windowed Parakeet q8_0 on CPU.
  - [x] Same, q4_0 CPU (56/56) and f16 Vulkan (32/56, stopped at the time limit), at 3
        threads. q8_0 stays the default; Vulkan matches it in quality.
  - [x] Aligner improvements round 1: anchor rule, lead, Params + bench sweeps (`ca6aaf3`).
  - [x] Whistle on the test track: plain, word keywords, line keywords.
  - [x] Whistle (word keywords) over the corpus: 31/56, then stopped. Parakeet wins clearly.
  - [ ] Vocal isolation trial on the failing tracks (Radiohead, Metallica, Stevie Nicks)
  - [ ] Keyword biasing for Parakeet? It has no flag; check whether `parakeet.dll` exposes
        any context biasing.

Spike layout: `%LOCALAPPDATA%\jarlid-autosync-spike\` — `dl/` (official binaries),
`src/whisper.cpp/build-vulkan/` (Vulkan build), `models/`, `corpus/<track>/`,
`results/<config>/`. Runner: `crates/autosync/spike/run-asr.sh <config> <track-dir>…`.
Everything heavy goes through the CPU broker at 4 slots with `-t 4`.

## Open questions for the user

None open. (The first five were answered 2026-10-13; see "Decisions already made".)

## Things not to do

- Don't bundle the model or runtime in the installer. Opt-in download is a requirement.
- Don't auto-publish generated timings to LRCLIB.
- Don't run inference on or near the audio thread, or at normal priority.
- Don't show transcription-from-nothing output as if it were real lyrics. Label it, gate
  it, keep it opt-in.
- Don't run unpinned downloaded executables. Verify the hash before the first run, every
  time the file changes.
