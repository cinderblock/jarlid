# Auto-sync: trust the lyrics, only add timings

## Goal

Cameron, 2026-10-19, after living with v1.13.0's auto-synced lyrics: "words are misheard.
choruses get marked out of order. Lyrics are always monotonic in their order... need to trust
the lyrics we get from the internet more and try to exclusively add timing info, not re-make
the lyrics."

Make the aligner treat LRCLIB's plain lyrics as the truth: every line is sung, in order, and
takes time to sing. The speech model's output is only evidence of *when*, never of *what*.

Parent plan: `plans/auto-synced-lyrics.md` (engine, sidecar, corpus, bench).

## Environment / context

- Aligner: `crates/autosync/src/align.rs`. App use: `app/src-tauri/src/autosync/mod.rs`
  (`hear_and_align` re-aligns after every window; `lrc_of` → `Alignment::timed`).
- Bench: `cargo run --release -p autosync --example bench -- <corpus> <results> <config>`
  from `crates/`. Corpus and cached model output are in
  `%LOCALAPPDATA%\jarlid-autosync-spike\` (`corpus/`, `results/parakeet-cpu-q8_0w/`, which is
  the shipped engine's configuration). `BENCH_DUMP=<track>` prints heard words and line anchors.
- Cameron's real results: `%LOCALAPPDATA%\com.camer.pandora-desktop\lyrics-auto\*.json`
  (stored LRC), paired by file name with `lyrics\*.json` (the LRCLIB plain text).
- Analysis scripts used for this task (scratch, not in the repo): `/tmp/asx/*.py`.

## Findings

- **The text is already LRCLIB's, verbatim.** All 32 stored results that passed the gate have
  exactly the plain text's non-blank lines, in order, with non-decreasing times. Nothing
  re-makes the lyrics. What reads as "misheard" and "out of order" is *timing*: lines lit up
  against the wrong part of the song.
- **Baseline** (aligner at `bd7e650`, parakeet q8_0 windowed, 56 tracks, 2,895 lines): median
  0.19 s, p90 2.75 s, ≤0.3 s 59%, ≤1 s 78%. Offset removed: median 0.15, p90 1.76.
- **Error taxonomy** (lines off by > 1 s after removing each track's LRCLIB offset; 392 of
  2,725): 252 interpolated lines, 94 lines anchored on ≥ half their words, 46 anchored on
  fewer.
- **Failure 1: a heard phrase split across two copies of a line.** *Something Just Like This*:
  the model heard "I want some bites like this" once, at 73 s. "I want" anchored line 19 ("Oh,
  I want something just like this") and "like this" anchored line 24 (same words), so lines
  19–24 (31 words) were squeezed into 0.4 s. Word-level Needleman–Wunsch scores the split and
  the unsplit alignment the same: the same lyric words go unmatched either way. Nothing says
  31 words can't be sung in 0.4 s.
- **Failure 2: whole lines on the wrong occurrence.** Eagles *One of These Nights* lines fully
  matched 20–36 s from where they belong; Metallica *Master of Puppets* +111 s; Keane −5 s.
- **Failure 3: unheard lines spread evenly over instrumental breaks.** Weezer *Island in the
  Sun*: three "Hip-hip" lines, unheard, were spread over 101→135 s (+8 to +13 s). The true
  lines came one bar apart right after the last sung line. Song lines tend to land on bars,
  so a line's duration is closer to constant than proportional to its word count.

## Design (as built, `1162e29`)

1. **Whole-song word alignment, time-aware.** Needleman–Wunsch as before (heard words free at
   both ends), plus: lyric words the model didn't hear must fit into the silences between
   heard words, `unheard_word_seconds` (0.4 s) each plus `SLACK` (0.25 s) per silence. Each
   word beyond that costs `crowding` (−2) on top of the gap. Runs of skipped lyric words are a
   separate DP state, computed per column with a sliding-window max (fits) and a running max
   (beyond), so it stays O(n·m). This is the "every line is sung, in order, and takes time"
   rule, and it makes Failure 1's split cost ~50 points.
2. **Line pace.** Median gap between consecutive anchored lines of the same stanza
   (`Alignment::line_seconds`); words × word pace + 0.5 s when fewer than three pairs.
3. **Placing unanchored lines by stanza** (blank lines separate stanzas). Between two anchors
   of one stanza: spread evenly. Otherwise: the rest of the earlier anchor's stanza follows it
   a step at a time; the start of the later anchor's stanza leads into it; whole stanzas
   between sit in the middle of the time left. Doesn't fit: spread evenly. Before the first
   anchor / after the last: step out at the pace, squeezed into `0..duration` if needed.
4. **Bench scores what ships** (`dfa903b`): `Alignment::placed`, i.e. every line the pane
   shows, not only anchored and interpolated ones.

### Rejected: per-line placements chained by a DP

Tried first (kept at `%TEMP%\asx\align-chain.rs` during the task, not committed): every place
each line could be sung, from a per-line semiglobal alignment, then a DP over placements in
lyric order with the time-to-sing rule. It fixed the split, but lost the arbitration the
whole-song alignment gives for free: a line aligned alone reaches into its neighbour's words
("Said you were made for me (only for me)" took the "for me" of "Think you were made for
me"). Patching that with head/tail cuts and looser de-duplication got it to p90 3.79 s; the
time-aware whole-song alignment got 3.43 s with fewer regressions, so the chain was dropped.

## Decisions already made (don't re-ask)

- Lyrics text is never altered; the aligner only produces times. (Cameron, 2026-10-19.)
- The aligner change ships with an app update; no engine release needed (alignment runs in
  the app, `plans/auto-synced-lyrics.md` "As built").

## Results

Bench, parakeet q8_0 windowed, 56 tracks, scoring every line the pane shows:

| Aligner | p90 | ≤0.3 s | ≤1 s | ≤3 s | Gated p90 | Gated ≤1 s |
| --- | --- | --- | --- | --- | --- | --- |
| v1.13.0 (`bd7e650`) | 4.27 | 59% | 79% | – | 2.02 | 85% |
| Per-line chain (rejected) | 3.79 | 59% | 79% | – | 1.82 | 86% |
| Time-aware NW, 0.1 s/word | 3.43 | 60% | 80% | – | 1.45 | 87% |
| + stanza placement | 2.98 | 60% | 81% | 89% | 1.52 | 87% |
| **+ 0.4 s/word (shipped, `1162e29`)** | **2.45** | **61%** | **82%** | **90%** | **1.41** | **87%** |

Gated = coverage ≥ 60%, 43 tracks. Offset removed, gated: p90 0.94 s, ≤1 s 90%, ≤3 s 96%.

Per track: *Something Just Like This* p90 11.35 → 0.58, *Anti-Hero (Kungs)* 18.79 → 3.54,
*Butterflies Pt. 2* 8.47 → 1.10, *Last Resort* 5.45 → 1.63, *Kryptonite* 6.54 → 0.92,
*Island in the Sun* 12.73 → 2.48.

Sweeps: `crowding` −0.5…−4 makes little difference (−2 kept). `unheard_word_seconds` 0.06,
0.1, 0.15, 0.2, 0.25, 0.3, 0.4, 0.5 → overall p90 3.11, 2.98, 2.86, 2.75, 2.97, 2.72, 2.45,
2.75: noisy; 0.2 and 0.4 tie on gated songs, 0.4 much better on barely-heard ones.
`SLACK` 0 / 0.25 / 0.5 → 2.61 / 2.45 / 2.53. A per-line "matches must be within one line's
time" filter changed nothing at any setting and was removed.

Still ambiguous, by nature: when the model heard a repeated phrase once and either copy of
the line fits the timing (*Something Just Like This* lines 23/24 at 0.1 s/word), and where the
break falls around a wholly unheard stanza (*Island in the Sun* after, *Diary of Jane*
before). *The Diary of Jane* is the one real regression, p90 1.05 → 6.48 s: its unheard bridge
comes after a 16 s solo, which the old even spread happened to suit and centring halves. The
model heard nothing at all in either gap, so only an audio cue (vocal activity) could tell
the two apart.

## Plan / steps

1. [x] Diagnose from Cameron's stored results and the corpus bench.
2. [x] Bench scores `placed()` output (`dfa903b`).
3. [x] Time-aware alignment, stanza placement, tests from real data (`1162e29`).
4. [x] Results stored by the old aligner are re-made (`142f1a1`): stored records carry the
       aligner version (`autosync::align::VERSION`, now 2) and the heard words, so a newer
       aligner re-times from them without hearing the song again. v1.13.0's records have no
       heard words, so those songs are heard again the next time they play.
5. [x] README (`d041fe8`).
6. [x] Released as v1.13.1 (`6c89528`, CI run 39325698339): installer, MSI, signatures and
       `latest.json` attached; `releases/latest` serves 1.13.1.
7. [ ] Live check in the installed app once it updates (Cameron). ← current

## Progress log

- [x] 2026-10-19: diagnosis (above).
- [x] 2026-10-19: bench scores the pane's output (`dfa903b`).
- [x] 2026-10-19: new aligner (`1162e29`). Tests: the split (from real heard words; fails on
      the old aligner), stanza pace, in-stanza spread.
- [x] 2026-10-19: stored results keep heard words and aligner version (`142f1a1`); README
      (`d041fe8`).
- [x] 2026-10-19: v1.13.1 released.
- [x] 2026-10-19: after updating to v1.13.1 the window was blank but for Stations. Not this
      task's code (no frontend change since v1.13.0): the update resumed playback before the
      window listened, and now-playing etc. were one-off events. Fixed by latching the state
      events and having the window ask after listening (`d0aa669`, `4fdd22b`). Needs a release
      to reach the installed app; waiting on Cameron.
- [ ] Live check in the installed app.

## Open questions for the user

1. ~~Cut a release?~~ Yes, 2026-10-19 ("deploy/release"): v1.13.1.
2. Later, if mistimed unheard stanzas still bother you: the sidecar could report vocal
   activity per window (it has the audio), which would settle where a break falls around a
   stanza the model heard none of. That needs an engine release.

## Things not to do

- Don't alter, drop, merge or reorder lyric lines. Lines with no words (e.g. "♪") are the only
  ones not shown, as before.
- Don't tune on one track; every change is judged on the full 56-track bench.
