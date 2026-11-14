# Sharper word timing: the forced aligner in the app

## Goal

Ship the word-timing method that won `plans/forced-alignment-experiment.md` (round 2, "combo"):
on the 27 credible truth songs, words within 0.1 s go from 56% to 67%, p90 error from 0.40 s to
0.34 s. Words the speech engine didn't hear are placed by a CTC forced aligner (MMS_FA) between
the heard words around them; heard words are nudged to the aligner's time when the two agree
within 0.25 s.

It is an **opt-in extra download** (~370 MB), separate from the auto-sync engine download.
Without it, word timing stays exactly as v1.23.0 does it.

## Environment / context

- App: Tauri, `app/src-tauri`. Engine download and status: `app/src-tauri/src/autosync/component.rs`
  (one pinned manifest SHA-256 vouches for every file). Jobs: `app/src-tauri/src/autosync/mod.rs`
  (`Job::Words` hears line-timed songs, stores `StoredHeard` in `<cache>/lyrics-heard/`).
  Placement at display time: `app/src-tauri/src/word_timing.rs` (`fill`).
- Sidecar: `crates/autosync-sidecar` (JSON-lines over stdin/stdout, protocol 1, version 0.1.0,
  loads whisper.cpp Parakeet DLLs with `libloading`). Released by
  `.github/workflows/autosync-sidecar.yml` on an `autosync-v*` tag (repo is public: CI is free).
  `package.ps1` writes the manifest; `model.json` pins the speech model.
- Shared logic: `crates/autosync` (`words::time_words` is today's placement).
- Experiment code to port (scratchpad of the experiment session, also described in the
  experiment plan): `tools/fa.py` (per-line windows, PAD_BEFORE 0.15 s, PAD_AFTER 0.35 s, star
  both ends), `tools/anchored.py` (pins + gap alignment, th 0.05), the combo step (heard words
  moved to the aligner's start when within 0.25 s), `tools/export_onnx.py`.
- Candidate model: `C:\Users\camer\.cache\jarlid-align\onnx\mms_fa.int8mm.onnx` (355 MB, ONNX
  opset 17, input `audio` [1, samples] already layer-normed, output `emission` [1, frames, 28]
  logits; the caller does log_softmax and appends a zero star column). Python venv with torch,
  torchaudio, onnxruntime: `C:\Users\camer\.cache\jarlid-align\venv` (for reference outputs).
- `ort` crate 2.0.0-rc.15 (ONNX Runtime 1.31 API), with `load-dynamic`: the DLL is downloaded
  with the aligner, never linked. rustc here is 1.100.

## Decisions already made (don't re-ask)

- Build it (Cameron, 2026-11-13: "yes").
- Opt-in, its own download, not bundled with the auto-sync engine (Cameron, 2026-11-13).
- The full-mix audio, no vocal isolation (three tests showed stems don't pay).
- int8 on MatMul only (fastest on CPU, same accuracy).
- Aligner confidence threshold 0.05; refine window 0.25 s (stricter gates tested worse).
- Alignment math lives in the app (`crates/autosync`), like the line aligner: the sidecar only
  runs the network and returns per-line emissions. Improvements then ship with the app.
- No version bump or app release unless Cameron asks ("publish" means release).

## Design

1. **`crates/autosync::ctc`** (pure Rust, tested against torchaudio outputs):
   MMS_FA labels (`-` blank, `a i e n o u t s r m k l d g h y b p w c v j z f ' q x`, `*` star),
   `romanize` (casefold, NFKD, strip marks, `’`→`'`, keep dictionary letters, drop `-` and `*`),
   `log_softmax` + star column, `forced_align` (CTC Viterbi with blank, like
   `torchaudio.functional.forced_align`), token spans → per-word (start frame, mean prob).
2. **`crates/autosync::words`**: `LineWords` gains which words were heard (pins). New
   `refine(lines, today, emissions, params)` = anchored placement for unheard words in each gap
   (tail gap aligned with a trailing star), then heard words moved to the aligner's whole-line
   start when within 0.25 s, then the 0.03 s minimum gap.
3. **Sidecar 0.2.0, protocol 2:** `loadAligner {model, threads}` (ort session, onnxruntime.dll
   from the engine dir) and `emit {id, wav, spans:[[t0,t1],…]}` → one `emission {id, k, t0,
   ratio, frames, data}` per span (data: base64 f32 log-probs incl. star column, or null when
   the span is too short), then `done`. Cancel as for `transcribe`.
4. **Manifest:** an optional `aligner: {files:[onnxruntime.dll], model:{…}}` section; the engine
   download fetches only `files` + `model` as today. A separate install fetches the aligner part.
5. **App:** setting `autosync.aligner` (off by default), a switch on the Settings page beside
   auto-sync, shown once the engine is installed, with its size inline. `Job::Words` (and a new
   pass for songs already heard) runs `emit` for the song's lines and stores the per-line
   aligned starts with the heard words; `word_timing::fill` uses them when the lines still match.
6. **Release:** the model can't come from a URL pinned elsewhere (it's our export), so it's a
   release asset. Engine release `autosync-v0.2.0` built by CI; model asset uploaded once.

## Plan / steps

1. [x] `ctc` module + tests against torchaudio reference (`144d708`).
2. [x] `refine` (`f5adae8`) — + port of anchored/combo; check on the truth set with the saved emissions
   (`emit-mix-int8mm` pickles → JSON) that Rust reproduces combomm's 0.06 / 0.34 / 67%.
3. [x] Sidecar: ort + `loadAligner` + `emit` (`8caa9ed`); protocol number unchanged (added
   messages only). The real sidecar on Wings scores the same as the saved emissions.
4. [ ] Manifest/component: aligner section, separate install/status/remove; `package.ps1`
   lists `onnxruntime.dll` under `aligner`, not `files`. ← current
5. [ ] App: setting, Settings switch, job wiring, storage, `fill` uses it; end-to-end locally
   with a development manifest.
6. [ ] Engine release + model asset (ask before publishing; see open questions).
7. [ ] Pin the new manifest in `component.rs`; README; app release only when asked.

## Findings / gotchas

- `-` is the MMS_FA CTC blank: a hyphenated word must have its `-` removed before tokenizing
  or the whole alignment fails (found in the experiment on dopamine).
- torchaudio's `_Wav2Vec2Model` wrapper can't be traced; the export is of `model.model`, so
  layer_norm before and log_softmax + star column after are the caller's job.

- The Python experiment's combo had a bug the Rust port doesn't: on a line with a hyphen
  ("copy- paste", 147 such lines in the truth set) its whole-line alignment threw, fell back to
  0.25 s steps with score 0, and the combo step still snapped heard words to those. Rust:
  0.06 / 0.35 / 67% / 85% vs the experiment's 0.06 / 0.34 / 67% / 86%. On hyphen-free lines
  the two differ by > 0.05 s on 53 of 11,068 words (the Rust check reads f16-saved emissions).
- Check command: `WORDS_EMIT=<scratch>/emit-rust WORDS_VARIANT=rust-combo cargo run --release
  -p autosync --example words -- <cases>`, then `score2.py base rust-combo`.
- **Wings is not independent truth for word times.** Its word starts were built from the
  engine's heard words (`plans/lyric-sections.md`, 2026-11-10), so heard words score 0.000 there
  and anything that moves them looks worse (Wings 82% → 66% within 0.1 s). The plans calling
  Wings "hand-timed" mean its *lines*. Per step on the 27 songs: anchored alone 59% / p90 0.34;
  heard-word refinement alone 64% / 0.40; both 67% / 0.35. The refinement helps 24 of 27 songs.
- **Caveat:** LRCLIB's word times may themselves be machine-made (the 11 excluded songs clearly
  are), which could flatter an aligner. There is no fully independent word-level truth; Cameron
  judging by eye in the app is the real test.
- Real sidecar: Wings (47 lines) took 59 s on 3 threads at low priority on a busy machine;
  model load 4.8 s. ORT: `onnxruntime-win-x64-1.31.1.zip` (sha256 `5869e26b…4b2c`); its
  `lib/onnxruntime.dll` is 17.3 MB (sha256 `cb2fa518…db0c`), cached in
  `C:\Users\camer\.cache\jarlid-align\ort-1.31.1`. Test driver: `<scratch>/tools/sidecar_emit.py`.

## Progress log

- [x] Plan written (2026-11-13).
- [x] Steps 1–2: alignment and placement in `crates/autosync`, verified against the experiment.
- [x] Step 3: sidecar `loadAligner` / `emit` (`8caa9ed`).

## Open questions for the user

1. **Model license.** Confirmed from torchaudio's MMS_FA docs: Meta released the MMS forced-alignment weights under CC-BY-NC 4.0
   (non-commercial, attribution). Jarlid is free and open, so distributing our int8 export
   with attribution should be fine, but it does mean the aligner can't ship in anything sold.
   I'll show you the exact asset and notes before uploading anything.

## Things not to do

- Don't bundle the aligner into the auto-sync engine download.
- Don't mark an `autosync-v*` release "latest" (it would hide app updates).
- Don't bump the app version or tag an app release unasked.
