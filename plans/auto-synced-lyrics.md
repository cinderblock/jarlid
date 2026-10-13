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

## Key design points (proposed — see Open questions)

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
- Not yet verified: which Windows assets whisper.cpp's official releases ship (CPU,
  Vulkan, CUDA), and the current DTW flag name. Irrelevant if we build our own sidecar,
  but needed for the step-1 spike.

## Progress log

- [x] Survey codebase: lyrics flow, audio look-ahead (`prefetch`, `cache_next`), decoder
      formats.
- [x] Literature / practitioner research.
- [x] Design written (this doc).
- [ ] Step 1 spike.

## Open questions for the user

1. **Sidecar exe vs in-process DLL** for the optional core. *Recommend sidecar* (§5).
2. **Scope of v1:** alignment of LRCLIB plain lyrics only, with transcription-from-nothing
   later? *Recommend yes*: transcription quality on singing is poor, and showing wrong
   words as "lyrics" is worse than showing none.
3. **GPU in v1?** A Vulkan build covers NVIDIA/AMD/Intel, but adds build complexity and
   size. *Recommend: decide after the spike's CPU timings.* If `large-v3-turbo` beats
   realtime on CPU here, ship CPU first.
4. **Pandora terms:** analysis downloads the next track whole even when the disk cache is
   off. Only text is kept and the audio is discarded after analysis. This is the same kind
   of fetch `prefetch` already does, just longer. OK?
5. Okay to run the step-1 spike now?

## Things not to do

- Don't bundle the model or runtime in the installer. Opt-in download is a requirement.
- Don't auto-publish generated timings to LRCLIB.
- Don't run inference on or near the audio thread, or at normal priority.
- Don't show transcription-from-nothing output as if it were real lyrics. Label it, gate
  it, keep it opt-in.
- Don't run unpinned downloaded executables. Verify the hash before the first run, every
  time the file changes.
