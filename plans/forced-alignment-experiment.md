# Forced alignment on a vocal stem: does it time words better?

## Goal

Cameron, 2026-11-12, after the word-timing batch: "I feel like we should be able to do better
auto timing..." — then "yes" to an offline experiment.

Today's word timing (`plans/word-timings-for-line-timed-lyrics.md`) transcribes the full mix with
Parakeet and matches what it heard to the known lyrics. Only words it heard right get a real
time (median 73% per song); the rest are spread by syllables. Non-English lines (dopamine's
French) get nothing. The same weakness limits line auto-sync (`plans/auto-synced-lyrics.md`,
whose phase 2 already names vocal isolation and CTC forced alignment, never started).

Question: on songs with known word times, how much better are word starts from
(a) today's method, (b) today's method on an isolated vocal stem, (c) CTC forced alignment of the
known text (MMS_FA class) on the vocal stem, inside the known line windows? Measure first; the
app changes only if a variant clearly wins, and shipping it (bigger engine download) needs
Cameron's yes.

## Environment / context

- CPU only (no NVIDIA GPU), 12 cores, compute budget 4 slots (`srmctl run --slots N --low`).
- Disk: C: 62 GB free (2026-11-12).
- Scratch: `<scratchpad>` = `C:\Users\camer\AppData\Local\Temp\claude\bc125a2150c32671\a45acb2b-fa82-4d65-ad1d-676d8c45c571\scratchpad`
  - `truth.json`: the 39 ground-truth songs (`src`, `file`, `mp3`, `name`); `heard/<TR_id>.json`
    = Parakeet on the mix; `tools/hear.py`, `tools/cases.py`.
- Experiment venv + stems: `C:\Users\camer\.cache\jarlid-align\` (durable, outside Temp).
- Scorer: `crates/autosync/examples/words.rs` (error of every word but each line's first,
  against truth; median, p90, share ≤ 0.1 s / ≤ 0.25 s).

## Decisions already made (don't re-ask)

- Line starts from LRCLIB or Cameron stay fixed; only words inside lines are timed.
- Experiment offline in Python first; nothing ships without numbers and a yes.

## Plan / steps

1. Baseline on all 38 truth songs: hear the 30 not yet heard (Parakeet, mix).
2. Venv: torch + torchaudio (MMS_FA) + demucs, CPU.
3. Vocal stems for the 39 (htdemucs, `--two-stems vocals`).
4. Variant (b): Parakeet on the stems → same `words` scorer.
5. Variant (c): MMS_FA per line window on the stem (and on the mix, to separate the two effects).
6. Compare; write up; recommend.
7. int8 ONNX aligner: size and accuracy. ← done; waiting on Cameron (open questions)

## Findings

- Truth set is 38 songs: `truth.json`'s 39 minus Grim Reaper's saved edit, whose word times are
  the ones this work generated (its LRCLIB record stays in). Molly has no cached audio.
- MMS_FA model: 1.18 GB download (torch hub cache, `C:\Users\camer\.cache\torch\hub`).
- First run, Wings, FA on the **mix**: median 0.10 / p90 0.48 / 53% within 0.1 s, against
  today's method on the same song 0.06 / 0.16 / 82%. Worse on the mix; stems not yet tried.
- **Baseline, today's method, all 38 songs** (2026-11-13 3:20 AM PST): median 0.11 s, p90
  0.50 s, 47% within 0.1 s, 76% within 0.25 s — worse than the first 8 songs suggested
  (0.08 / 0.40 / 58%). Spread is wide: Wings 0.06 / 0.16, Good Girl Era 0.06 / 0.13, but
  Sweet Dreams 0.61 / 1.13, Elevator Eyes 0.34 / 0.67, Running with the Wolves p90 2.79.
  Some of the worst may be LRCLIB truth that is itself off (as with Rapunzel); check those
  by ear before trusting them either way.
- **FA on the vocal stem, first 20 songs** (2026-11-13 5:45 AM PST, `cases/fa-stem-early/`):
  median 0.09 / p90 0.64 / 54% ≤ 0.1 s, against baseline 0.10 / 0.45 / 50% on the same songs.
  Very precise where it works (d e a d 0.04 / 0.08 / 93%; Good Girl Era 0.03 / 0.10 / 91%),
  badly wrong where it fails (Thriving median 0.42).
- **Where FA fails: lines of vocables.** Thriving's "(la- la- la- …)" / "(da- da- …)" lines get
  confidence ≈ 0 on every word and are squeezed ~0.6–1.1 s early; its ordinary lines are within
  0.1 s. FA's own per-line confidence flags these.
- **Five songs' LRCLIB word times look early, not our methods late:** Rapunzel, Sleepwalker,
  SEX MACHINA, boys will be boys, Fired My Therapist. Both methods sit 0.12–0.28 s after
  "truth" yet agree with each other within 0.04 s (signed medians), and they hear the audio
  independently. Scored separately as "credible" = the other 15.
- **Hybrid, per line: FA when its line's mean confidence ≥ 0.1, else baseline.**

  | variant | 20 songs (med / p90 / ≤0.1 / ≤0.25) | 15 credible |
  | --- | --- | --- |
  | baseline | 0.10 / 0.45 / 50% / 78% | 0.08 / 0.34 / 58% / 86% |
  | FA stem | 0.09 / 0.64 / 54% / 72% | 0.06 / 0.61 / 65% / 81% |
  | **hybrid** | 0.08 / 0.43 / 58% / 79% | **0.06 / 0.30 / 70% / 88%** |
  | best-of (oracle) | 0.05 / 0.32 / 69% / 86% | |

  Agreement-based selection (FA when within 0.2 s of baseline) does less: 0.08 / 0.45 / 56%.
- **All 38 songs (2026-11-13 7:00 AM PST).** The suspect-truth check run over all of them flags
  11: the five above plus Chlorine, Elevator Eyes, i don't cry when i'm high, life's a bitch,
  Sweet Dreams (0.6 s), Time of my Life. In each, FA and baseline agree (signed median within
  0.05 s) and both sit 0.12–0.6 s after LRCLIB's word times. Not an audio-version offset: the
  LRC line tags of the flagged songs scatter around the first heard word like every other
  song's (about ±0.3 s across the whole set). LRCLIB's in-line word times are simply bunched
  early in those records.
- **Result on the 27 songs with credible truth** (error of every word but each line's first):

  | variant | median | p90 | ≤ 0.1 s | ≤ 0.25 s |
  | --- | --- | --- | --- | --- |
  | today (Parakeet on mix + `words`) | 0.09 | 0.40 | 56% | 84% |
  | FA on mix | 0.07 | 0.63 | 60% | 78% |
  | FA on vocals | 0.07 | 0.60 | 61% | 79% |
  | **hybrid on vocals** (FA when line confidence ≥ 0.1, else today) | **0.06** | **0.36** | **67%** | 85% |
  | hybrid on mix (same rule) | 0.06 | 0.39 | 65% | 85% |
  | best of FA-vocals and today, per word (oracle) | 0.04 | 0.21 | 77% | 92% |

  All 38 (bad truth included): today 0.11 / 0.50 / 47%, hybrid vocals 0.08 / 0.48 / 55%.
- **The aligner is the gain; vocal isolation adds ~2 points.** Hybrid on the mix gets most of
  it. Separation is the expensive step: htdemucs took 153 min for 38 songs on 3 CPU threads
  (~4 min a song); FA took 25–32 min for 38 (~45 s a song).
- The confidence threshold barely matters between 0.05 and 0.1; 0.2 is slightly worse.
- The oracle's 0.21 s p90 says a better per-word chooser has headroom; the line-confidence rule
  is the simplest that works.
- **Parakeet on the vocal stem** (today's method, stem instead of mix): 27 credible songs
  0.09 / 0.36 / 55% / 85% vs 0.09 / 0.40 / 56% / 84% on the mix; matched words 5,085 vs 4,926.
  Hybrid with Parakeet-on-stem as the fallback: identical to hybrid with Parakeet-on-mix.
  Vocal isolation is not worth its cost for word timing.
- **Shippable aligner:** the bare wav2vec2 network exports to ONNX (opset 17) once torchaudio's
  wrapper is peeled off (`_Wav2Vec2Model.forward` builds a shape list the exporter can't
  trace); the caller does layer-norm on the audio, log-softmax, and appends a zero "star"
  column. fp32 1,262 MB; dynamic int8 (`quantize_dynamic`, QInt8) **317 MB**. Same frame
  argmax as torch on a test clip. Hybrid on the mix, 27 credible songs: fp32 0.06 / 0.39 / 65%
  / 85%, **int8 0.06 / 0.41 / 64% / 84%**. int8 keeps nearly all the gain. It ran 52 min for 38
  songs on 3 threads (~80 s a song) vs torch fp32's 25 min, under a contended budget; speed
  not yet measured cleanly. `tools/export_onnx.py`; `FA_ONNX=<path>` makes `fa.py` use it.
- **What the gain is and isn't.** Hybrid moves words from "close" to "very close" (≤ 0.1 s:
  56% → 64–67%; median 0.09 → 0.06 s). It barely moves the tail (p90 0.40 → 0.36–0.41 s), and
  the tail is what a listener notices. The oracle (best of FA and today per word: p90 0.21 s)
  says a better per-word chooser could reach the tail. Not measured at all: songs Parakeet
  barely hears (the 5 below the app's gate, dopamine's French lines), where FA would replace
  syllable spreading outright — likely the biggest visible win, but there's no truth for them.
- The compute budget is often full with other threads' builds: a 1-slot demucs test waited
  39 min, then ran out a 30-min tool timeout. Heavy stages now run from one hidden driver
  (`tools/run_experiment.py`, launched with `pythonw`, log `<scratchpad>/experiment.log`).

## Progress log

- [x] Step 1: all 38 heard.
- [x] Step 2: venv at `C:\Users\camer\.cache\jarlid-align\venv` (torch/torchaudio 2.8.0 CPU,
  demucs 4.1.0). Scripts in `<scratchpad>/tools`: `separate.py`, `fa.py`, `score.py`,
  `run_experiment.py`. Steps 1, 3, 4, 5 run in order from `run_experiment.py` (launched
  2026-11-13 2:50 AM PST).
- [x] Step 3: stems for 38 (153 min).
- [x] Step 4: Parakeet on stems.
- [x] Step 5: FA on stems and mix.
- [x] Step 6: results above.
- [x] Step 7: int8 ONNX, 317 MB, keeps the gain.

## Open questions for the user

1. What looked off when you watched it (words early/late, wrong word lit, songs with no
   word highlighting)? Decides whether the tail or the unheard songs come first.
2. Next: (a) one more offline round on the tail and on unheard songs (a per-word chooser;
   checking FA on dopamine and the 5 below-gate songs by ear), then decide on shipping; or
   (b) ship the hybrid now: int8 aligner in the sidecar via ONNX Runtime, +317 MB engine
   download, ~1 min CPU per song once. Recommended: (a) — the shipped gain today is mostly
   invisible, and the download is big.

## Things not to do

- Don't score against Rapunzel's LRCLIB words as if they were truth (bunched at line starts;
  see the word-timings plan). Weigh Wings and Molly (hand-timed) first.
