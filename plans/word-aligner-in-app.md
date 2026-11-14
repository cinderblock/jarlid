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
2. **`crates/autosync::words`**: `LineWords` gains which words were heard (pins).
   **`crates/autosync::refine`**: `refine(lines, placed, emissions, params)` = anchored
   placement for unheard words in each gap (tail gap aligned with a trailing star), then heard
   words moved to the aligner's whole-line start when within 0.25 s, then the 0.03 s minimum
   gap. `window(line, duration)` = the span to emit over (−0.15 s / +0.35 s).
3. **Sidecar 0.2.0, protocol still 1** (added messages only): `loadAligner {model, runtime,
   threads}` (ort session; `runtime` = path of onnxruntime.dll, loaded once per process) and
   `emit {id, wav, spans:[[t0,t1],…]}` → one `emission {id, k, t0, ratio, frames, data}` per
   span (base64 f32 log-probs incl. star column; no data when under 0.1 s), then `done`.
4. **Manifest:** an optional `aligner: {files:[onnxruntime.dll], model:{…}}` section; the engine
   download fetches only `files` + `model` as before. The aligner installs into
   `<data>/autosync/aligner/` (not the engine dir, which engine updates replace).
5. **App:** setting `autoSync.aligner` (off by default). Settings: "Sharper word timing" switch
   under auto-sync, shown once the installed engine offers an aligner; on = download + use,
   off = delete. Status, progress and a retry button inline. Jobs: `Job::Words` refines right
   after hearing (same WAV); `Job::Refine` refines songs heard before, without hearing. Results
   per line in `<cache>/lyrics-refined/<fnv>.json` (`{version, model, created, lines:[{text,
   start, starts}]}`); `word_timing::fill` uses a line's refined starts while its text and
   start match. `word_timing::placement` is shared by the job and `fill`, so both start from the
   same base placement. Only songs with real synced/override lines; auto-synced songs are out
   of scope for now.
6. **Release:** the model is our export, so it is a release asset (`aligner-model-v1`,
   pinned in `crates/autosync-sidecar/aligner.json`). ONNX Runtime 1.31.1 is fetched by CI
   (`fetch-onnxruntime.ps1`, pinned zip hash) and re-hosted in the engine release.

## Plan / steps

1. [x] `ctc` module + tests against torchaudio reference (`144d708`).
2. [x] `refine` (`f5adae8`) — + port of anchored/combo; check on the truth set with the saved emissions
   (`emit-mix-int8mm` pickles → JSON) that Rust reproduces combomm's 0.06 / 0.34 / 67%.
3. [x] Sidecar: ort + `loadAligner` + `emit` (`8caa9ed`); protocol number unchanged (added
   messages only). The real sidecar on Wings scores the same as the saved emissions.
4. [x] Manifest/component: aligner section, separate install/status/remove (`e7895c8`);
   packaging (`97aa880`), dry-run checked: `onnxruntime.dll` lands only under `aligner`.
5. [x] App: setting, Settings switch, jobs, storage, `fill` (`e7895c8`). 140 app tests pass;
   `tsc` clean; ignored end-to-end test `sidecar_emits_for_the_aligner` passes against the
   real sidecar (aligner loads in ~1 s). **Not exercised:** the in-app download and switch,
   which needs the app on screen; Cameron's first use after release is that test.
6. [x] Cameron: "yes" (2026-11-13). Model published as release
   [aligner-model-v1](https://github.com/cinderblock/jarlid/releases/tag/aligner-model-v1)
   (not latest; ranged downloads work). Engine
   [autosync-v0.2.0](https://github.com/cinderblock/jarlid/releases/tag/autosync-v0.2.0)
   (`b2b3d2f`) built by CI; manifest SHA-256 `6fc9e75c…e3f7f9`, checked against a fresh
   download.
7. [x] Pinned in `component.rs` (`0134ef7`); the ignored `pinned_release` test passes against
   it. Existing installs re-fetch the ~45 MB of engine files and keep the speech model.
   App release [v1.24.0](https://github.com/cinderblock/jarlid/releases/tag/v1.24.0) (`86be96f`): run succeeded, installer, signature and `latest.json` published, it is the latest release.

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
- [x] Step 3: sidecar `loadAligner` / `emit` (`8caa9ed`, runtime path `0a22a51`).
- [x] Steps 4–5: app download, switch, jobs, storage (`e7895c8`); packaging (`97aa880`).

## Open questions for the user

Answered 2026-11-13 ("yes"): published, see step 6.

1. ~~Publish the model?~~ Confirmed from torchaudio's MMS_FA docs: Meta released the MMS
   forced-alignment weights under CC-BY-NC 4.0 (non-commercial, attribution). Jarlid is free,
   so distributing our int8 export with attribution fits, but the aligner could never ship in
   anything sold. The upload: a release `aligner-model-v1` on cinderblock/jarlid (not
   "latest"), one asset `mms-fa-int8.onnx` (355,119,008 bytes, sha256 `1f20360f…168e`), notes
   naming the source (torchaudio `MMS_FA`, Meta's MMS), the license, and that it is an int8
   (MatMul) ONNX export. Then the engine release. Recommended: yes.

## Things not to do

- Don't bundle the aligner into the auto-sync engine download.
- Don't mark an `autosync-v*` release "latest" (it would hide app updates).
- Don't bump the app version or tag an app release unasked.
