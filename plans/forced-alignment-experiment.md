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

(none yet)

## Progress log

- [ ] Step 1
- [ ] Step 2
- [ ] Step 3
- [ ] Step 4
- [ ] Step 5
- [ ] Step 6

## Open questions for the user

(none yet)

## Things not to do

- Don't score against Rapunzel's LRCLIB words as if they were truth (bunched at line starts;
  see the word-timings plan). Weigh Wings and Molly (hand-timed) first.
