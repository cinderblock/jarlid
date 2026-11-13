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
7. int8 ONNX aligner: size and accuracy.
8. Round 2: anchored FA for unheard words; heard words refined; int8-MatMul model. ← done; waiting on Cameron (open questions)

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
### Round 2 (2026-11-13, after Cameron: "I think it was some assumptions about where the unheard words must be")

- **The tail is the unheard words.** On the 27 credible songs, words Parakeet heard (and the
  `words` aligner matched) have error 0.08 / 0.21 / 64% / 93%; words it didn't hear (placed by
  syllables / line fill) 0.14 / 0.83 / 41% / 67%. 78% of today's > 0.3 s errors are unheard
  words. Whole-line FA does no better on them (0.16 / 1.01): the same audio is hard for it.
- **Anchored FA** (`tools/anchored.py`, no model run, uses per-line emissions `fa.py` saves
  with `FA_EMIT_DIR`): heard words are pins; each run of unheard words is CTC-aligned on the
  emission between the pins around it (after the last pin: to the line's end, with a star).
  Accept a word when its FA score ≥ 0.05, else keep today's. Unheard words: 0.10 / **0.70** /
  49% / 73%.
- Where the unheard words are: 1,329 in lines with nothing heard, 784 between heard words
  (0.07 / 0.54 after anchoring), 624 after the last heard word (0.15 / 0.72).
- **Copying a repeated line's rhythm** (a wholly unheard line takes the word offsets of an
  identical line that was heard) loses to anchored FA on the 86 lines that have such a twin:
  0.10 / 0.92 vs 0.09 / 0.41. Singers phrase repeats differently. Dropped.
- **Heard words refined by FA:** move a heard word to FA's whole-line time when they're within
  0.25 s. Combined with anchored FA ("combo"), 27 credible songs:

  | variant | all words (med / p90 / ≤0.1 / ≤0.25) | unheard words |
  | --- | --- | --- |
  | today | 0.09 / 0.40 / 56% / 84% | 0.14 / 0.83 / 41% / 67% |
  | round 1 best (line hybrid, int8 mix) | 0.06 / 0.41 / 64% / 84% | 0.11 / 0.81 / 47% / 70% |
  | anchored only | 0.08 / 0.33 / 59% / 86% | 0.10 / 0.70 / 49% / 73% |
  | **combo** (anchored + heard refined within 0.25 s) | **0.06 / 0.34 / 66% / 86%** | 0.10 / 0.70 / 49% / 73% |

  Thresholds barely matter (refine window 0.15–0.25 s; FA score 0.05–0.1).
- All with the **int8** aligner on the **mix**: no vocal isolation needed.
- **Speed, measured cleanly** (Wings, 3 threads, model load included): torch fp32 59 s, ONNX
  fp32 49 s, ONNX dynamic int8 (all ops) **120 s** — int8 convolutions in the feature
  extractor are slow on this CPU. **ONNX int8 on MatMul only** (`op_types_to_quantize=
  ['MatMul']`): **355 MB, 34 s**; all 38 songs in 14 min (~22 s a song).
- **int8-MatMul keeps the accuracy:** combo on it, 27 credible songs, 0.06 / 0.34 / 67% / 86%
  (vs 0.06 / 0.34 / 66% / 86% with all-op int8). This is the candidate to ship:
  `C:\Users\camer\.cache\jarlid-align\onnx\mms_fa.int8mm.onnx`.
### Dopamine (2026-11-13, Cameron: "split version or alternating version?")

- Only the **French + English** (alternating-lines) version is in play: its override
  `545b7f6f3922d6ba.json`, audio `TR_91277714.mp3`. The **Split Brain Version**
  (`ed7cdf4a9199e46a.json`) has only plain lyrics on LRCLIB and no cached audio, so the app
  gives it no timing at all; word timing never touches it.
- **Parakeet v3 hears French.** Mid-song French lines ("C'est bien chouette…", "Si toi, tu
  te barres", "À quel point…") are fully heard. French words heard: 33 of 97; English 98
  of 123. What it misses: the soft intro ("Tu es tout ce que je veux" ×2, "Je veux, je
  veux"), "Tu me rends un peu folle / Et ça prend…", "Est-ce que tu m'élèves…", "Tu
  m'contrôles…", and the parenthesised backing vocals at the end.
- **The aligner does fine on French** where the singing is audible: "Et ça prend trop peu
  d'effort" 0.50, "Tu me rends…" 0.28, "Est-ce que…" 0.26 line confidence (English lines run
  0.2–0.7). Anchored placement places 61 of the 87 unheard words.
- **Neither engine finds the intro or the outro backing vocals** (aligner confidence
  0.02–0.08). Those stay on the syllable spread; hand-timing is the only real fix.
- **Vocal stem made it worse here:** 119 lyric words heard vs 131 on the mix (lost "Tu es
  tout ce que je veux" at 1:47, "J'déteste…", "Mais rien…"). Third time stems don't pay.
- ~~Spelled-out words mislead the aligner.~~ **Wrong, corrected by looking at the
  spectrogram:** "Ma D.O.P.A.M.I.N.E" is sung as the *word* "dopamine", stretched (the engine
  heard "My day, okay, I mean" = "Ma do-pa-mine", from 5.7 s). The aligner put it at 5.75 s,
  which is right; today's syllable spread put it at 7.4 s. No letter-name expansion needed.

### Looking at spectrograms (2026-11-13, Cameron: "look at the waveforms to see the syllables?")

Tool: `<scratchpad>/tools/plot.py <stem.wav> <case.json> <t0> <t1> <out.png> label=out.json…`
(mel spectrogram of the vocal stem, loudness, heard words in green, one tick row per
placement). Plots in `<scratchpad>/plots/`.

- **Misheard words carry syllable timing on foreign lines.** Dopamine's intro "Tu es tout ce
  que je veux" was heard as "To it, two squash, your book, book", whose times sit on the
  visible syllable onsets. Rule tried (`tools/misheard.py`): in a gap of unheard lyric words,
  if the unmatched heard words there are as many, pair them in order. **On the 27 truth
  songs it doesn't help:** on top of combo 0.06 / 0.35 / 66% vs 0.06 / 0.34 / 67% (slack 1:
  0.36 / 65%). On English songs the leftover heard words are ad-libs and backing vocals. It
  also made dopamine's "Je veux, je veux" worse (today's even spread matched the onsets
  there). Not shipping.
- **Dopamine's outro** (backing vocals over the lead, 134–147 s): the aligner crams words
  ("et tout ce que je" inside 0.2 s); today's even spread looks more plausible. A stricter
  aligner score gate would keep the spread, but **on the truth set it hurts:** anchored
  th 0.05 / 0.1 / 0.15 / 0.2 → p90 0.34 / 0.34 / 0.35 / 0.36, unheard ≤0.1 s 49 / 47 / 45 / 44%.
  So low-score placements still beat the spread on average; dopamine's outro is the
  exception. Keep th 0.05.
- **Eyeballing vs the aligner:** the aligner reads the audio at 20 ms frames; reading a
  plot is ~0.1 s at best and can't hear. Looking is good for diagnosis (it caught the
  "dopamine" mistake above) and for hand-fixing a few hard passages, not as a step in the
  app.
- **Bug found and fixed in the scratch tools:** `-` is the aligner's CTC blank, so any
  hyphenated word ("Da-ra-da", "D-O-P-A-M-I-N-E", "Ooh-ooh") made the whole line's alignment
  throw. Romanization now drops `-`. Only 2 hyphenated words in the truth set, so the scores
  above stand. The app's romanizer must do the same.
- Gotcha: `grep -l dopamine heard/*.json` found a *different* song whose lyrics mention
  dopamine (TR_37416125). Find a track's audio by artist + title in `audio/*.json`.

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
- [x] Step 8: round 2: combo 0.06 / 0.34 / 67% on 27 songs; int8-MatMul model 355 MB, ~22 s a song.
- [x] Dopamine deep-dive (mix, stem, aligner): findings above; scratch in `<scratchpad>/dopa/`.
- [x] Spectrogram look: misheard-word timing and stricter gates tested, neither ships.

## Open questions for the user

Answered 2026-11-13: what looked off was where unheard words were assumed to be; "another
round" → round 2 above.

1. Build it into the app? What it would take:
   - Engine (sidecar): ONNX Runtime (`ort` crate, its DLL ~15 MB) running the int8-MatMul
     MMS_FA model (+355 MB download, a new engine version); a new op that returns per-line
     emissions (or does the alignment itself).
   - `crates/autosync`: CTC forced alignment (a small Viterbi over the line's tokens with a
     star token), MMS romanization (Latin: NFKD + strip marks; other scripts later), the
     anchored placement and the heard-word refinement.
   - App: `Job::Words` also runs the aligner (~25 s CPU a song, once); `word_timing::fill`
     uses the result.
   - **Opt-in, its own download** (Cameron, 2026-11-13): not part of the auto-sync engine
     download. Auto-sync on still fetches only today's engine; the aligner (+~370 MB) is a
     separate switch beside it, and without it word timing stays as v1.23.0 does it.
   - Text: drop `-` (CTC blank) when romanizing.
   - Then the same aligner can serve line auto-sync (`plans/auto-synced-lyrics.md` phase 2).
   Recommended: yes. Several days of work; ships as a minor release.
2. Songs the engine barely hears (the 5 below the 0.35 gate) have no truth to score against;
   they'd be judged by you, by eye, once it's in the app. Dopamine is looked at above.
3. Hand-time dopamine's intro and outro from the spectrogram into its override (backup
   beside it)? Offered, not asked for.

## Things not to do

- Don't score against Rapunzel's LRCLIB words as if they were truth (bunched at line starts;
  see the word-timings plan). Weigh Wings and Molly (hand-timed) first.
