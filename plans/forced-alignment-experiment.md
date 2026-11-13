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

1. Baseline on all 39 truth songs: hear the 31 not yet heard (Parakeet, mix). ← current
2. Venv: torch + torchaudio (MMS_FA) + demucs, CPU.
3. Vocal stems for the 39 (htdemucs, `--two-stems vocals`).
4. Variant (b): Parakeet on the stems → same `words` scorer.
5. Variant (c): MMS_FA per line window on the stem (and on the mix, to separate the two effects).
6. Compare; write up; recommend.

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
- The compute budget is often full with other threads' builds: a 1-slot demucs test waited
  39 min, then ran out a 30-min tool timeout. Heavy stages now run from one hidden driver
  (`tools/run_experiment.py`, launched with `pythonw`, log `<scratchpad>/experiment.log`).

## Progress log

- [ ] Step 1: hearing the 30 unheard truth songs (8 were heard before), running.
- [x] Step 2: venv at `C:\Users\camer\.cache\jarlid-align\venv` (torch/torchaudio 2.8.0 CPU,
  demucs 4.1.0). Scripts in `<scratchpad>/tools`: `separate.py`, `fa.py`, `score.py`,
  `run_experiment.py`. Steps 1, 3, 4, 5 run in order from `run_experiment.py` (launched
  2026-11-13 2:50 AM PST).
- [ ] Step 3
- [ ] Step 4
- [ ] Step 5
- [ ] Step 6

## Open questions for the user

(none yet)

## Things not to do

- Don't score against Rapunzel's LRCLIB words as if they were truth (bunched at line starts;
  see the word-timings plan). Weigh Wings and Molly (hand-timed) first.
