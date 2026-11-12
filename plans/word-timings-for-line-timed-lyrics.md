# Word timings for lyrics that only have line timings

## Goal

Cameron, 2026-11-11: "can you add automated per-word timings to songs that have good lyrics but
that are missing per-word (or even per lyric) timings? Or just manually go through our corpus.
I just manually timed lines for Grim Reaper, but per word timing would be better."

Every song whose words we trust should highlight word by word: LRCLIB line-synced songs,
auto-synced songs, and the listener's own line-timed edits. Line times that a person or LRCLIB
set stay as they are; the generated word times only fill in *within* each line.

## Environment / context

- App data: `%APPDATA%\com.camer.pandora-desktop\` (`lyrics-overrides\` = saved edits,
  `autosync\engine-0.1.0\jarlid-autosync.exe` + `models\ggml-parakeet-tdt-0.6b-v3-q8_0.bin`).
- Cache: `%LOCALAPPDATA%\com.camer.pandora-desktop\` (`lyrics\` = LRCLIB records, `lyrics-auto\`
  = auto-sync results incl. `heard`, `audio\TR_*.mp3` + `.json` = cached tracks).
- Grim Reaper (bludnymph): override + LRCLIB cache `8eb80b179b32f360.json`, audio
  `TR_106121814.mp3`.
- Word timing data model: `Lyrics.timing` (`lyricsfile::TimedLine`, words with starts);
  editor `app/src/edit-lines.ts` (`mergeTiming`, `fromTiming`, `toTiming`); pane highlight uses
  `wordTimesFor`.
- Offline engine driver (scratch): `<scratchpad>/tools/hear.py <out> <mp3>...` speaks the sidecar
  protocol (`crates/autosync-sidecar/src/protocol.rs`), CPU, 3 threads, below-normal priority.

## Decisions already made (don't re-ask)

- Lyrics text and line starts from LRCLIB or the listener are the truth; generated word times
  never move a line (same principle as `plans/autosync-lyrics-first.md`).

## Findings

- **Corpus (2026-11-11):** LRCLIB cache 1,642 records. Line-timed without words: 166 with
  cached audio, 1,240 without. With word timings already: 39 (37 with audio) — usable as ground
  truth, along with Cameron's hand-timed Wings (`a19cf2bd9aa2a87a`) and Molly (`2f0445e3b67c1db5`).
  Overrides: Grim Reaper (6/57 lines with words), your presence (0/52), dopamine (0/67).
- **Grim Reaper's LRCLIB record already has word times on all 54 lines**, but its Lyricsfile line
  starts disagree with its own LRC tags (timing 7.93 vs LRC 8.39, 10.17 vs 10.08). `mergeTiming`
  only attaches words when start matches within 0.011 s, so the editor dropped 48 of 54 lines'
  words on load. Bug in `mergeTiming` (and `wordTimesFor`, same rule).
- **ISSUES.md "A line's start can move without its first word" — root cause:** `setNow` and
  `markAt` in `app/src/lyric-editor.ts` set `lines[i].t` directly instead of via `setStart`, so a
  tapped line keeps its old word times. Grim Reaper's 6 surviving lines all show it (line 13.30,
  words from 13.79).
- **This is widespread, not Grim Reaper's alone.** Of the 39 cached LRCLIB records that carry
  word times, most have a Lyricsfile that is a *different transcription* from the record's own
  LRC: lines joined or split, ad-libs added ("Get Outta My Heart": 0 of 52 lines match;
  "Better People to Leave on…" 0 of 30). The pane (`wordTimesFor`) and editor (`mergeTiming`) only
  use words on lines whose start and text match exactly, so those songs show almost no word
  highlighting even though LRCLIB has it. Fix: treat a Lyricsfile's words as evidence, like
  heard words, and align them onto the LRC's own words (same `words` module, `Params::for_people`).
- **Grim Reaper, engine vs LRCLIB's words, both mapped onto Cameron's line starts:** engine
  matched 134 of 235 words (first words aside), LRCLIB's Lyricsfile 228; they agree to median
  0.23 s, p90 0.99 s. The engine runs ~0.15–0.2 s earlier than LRCLIB's words with the aligner's
  0.15 s `lead`, so `lead` may want to be ~0 for words. To be settled on the ground-truth set.
- Observation (not acted on): Cameron's tapped line starts for Grim Reaper sit ~0.3–0.5 s before
  the first word the engine heard (e.g. "In a super freaky sitch": tap 9.42, engine's "a"
  9.92). `TAP_LATENCY` in `lyric-editor.ts` may be over-correcting. Line starts are his call.
- `hear.py` gotcha: a path list written by Python in text mode on Windows ends lines in a carriage return;
  ffmpeg then says "Error opening input: Invalid argument". Strip paths.
- **Evaluation, first 8 ground-truth songs** (7 LRCLIB Lyricsfiles + Cameron's Wings; error of
  every word but each line's first; "base" = syllables alone, nothing heard):

  | lead | median | p90 | ≤0.1 s | ≤0.25 s |
  | --- | --- | --- | --- | --- |
  | 0.15 (the line aligner's) | 0.16 | 0.47 | 29% | 77% |
  | **0** | **0.09** | 0.48 | **55%** | 81% |
  | base | 0.23 | 0.92 | | |

  Wings alone (hand-timed, the real truth): median 0.08, p90 0.44, 69% ≤ 0.1 s. The engine's
  word starts land on sung word starts; the line aligner's 0.15 s lead is about *line* stamps.
- **Some LRCLIB Lyricsfiles are not truth.** Rapunzel's bunch words at line starts while the
  engine hears an even rhythm ("Just you and me in our little reality": LRCLIB 3.45→4.75,
  engine 3.44→5.76). Weigh results by Wings first.
- **The p90 is unheard lines.** Wings' worst lines are ones the engine heard nothing of: spread
  at the song's pace (0.24 s/syllable) they end early; Cameron's last word sits ~0.5 s before the
  next line. Trying: place the last word by the share of its line this song usually fills.
- Auto-synced songs store only an LRC (`Stored.lrc`) even though the aligner matched words.

## Design for the app (step 5)

- Backend, on the way out of `fetch_lyrics`, for any lyrics with an LRC whose lines lack word
  times: build `timing` from the LRC's lines (ends = next tag) with words from
  `autosync::words::time_words`. Evidence: the record's own Lyricsfile words if any
  (`for_people`), else what the engine heard for this track. Lines that already have exactly
  matching words keep them. Starts equal the LRC tags, so the pane's exact-match lookup works.
- Hearing: `autosync::run` today returns early when there's an LRC. New job: an LRC without
  full word times, and nothing heard yet → hear and keep only the heard words (new
  `lyrics-heard/` cache dir; `heard()` falls back to it). Same auto-sync on/off setting, no new
  UI.
- Auto-synced songs: their stored `heard` already exists; same path.

## Plan / steps

1. Small fix, own commit: tap paths shift words with the line (`04805dc`). The `mergeTiming`
   start-mismatch is handled in the backend instead (design above).
2. `crates/autosync`: `words` module — given timed lines and heard words, a start per word,
   constrained to its line's window; unmatched words interpolated. Unit tests.
3. Evaluate against the ground-truth songs (heard via `hear.py`), tune.
   Harness: `cargo run --release -p autosync --example words -- <case.json>…` (from `crates/`);
   cases built by `<scratchpad>/tools/cases.py`.
4. Grim Reaper: fill word times into the saved edit (backup first).
5. App: generate word times for line-timed songs (LRCLIB synced, auto-synced, overrides) when the
   engine hears them; serve them in `timing`.
6. Batch the cached line-only LRCLIB songs offline (scheduled 3 AM 2026-11-12). ← current

## Progress log

- [x] Inventory, root causes for Grim Reaper's lost words.
- [x] Step 1: tapped lines move their words (`04805dc`); ISSUES.md entry closed.
- [x] Step 2: `crates/autosync/src/words.rs` + `examples/words.rs` (`4af5fb3`).
- [x] Step 3 (on 8 songs): lead 0; unheard lines place their last word by the song's usual
  line fill (Wings p90 0.44 → 0.16 s; all 8: median 0.08, p90 0.40, 58% ≤ 0.1 s).
  30 more ground-truth songs not yet heard (optional, see questions).
- [x] Step 4: Grim Reaper's saved edit filled, 2026-11-11 PST: all 57 lines have words (engine +
  LRCLIB Lyricsfile evidence, 230/235 placed by evidence). Original kept as
  `lyrics-overrides\8eb80b179b32f360.json.before-word-fill.bak`. Works in the installed app.
- [x] Step 5: app (`7902917`): `word_timing::fill` on the way out of `fetch_lyrics`; `Job::Words`
  hears line-timed songs once (auto-sync on), stores `lyrics-heard\`; pane reloads on
  `autosync://words`. Tests + clippy pass. Not in a release yet.
  Coverage check: the 45 cached LRCLIB records with Lyricsfile words place 94% of their LRC's
  words (11,549 / 12,209) from their own Lyricsfile, no hearing needed.
- [x] Overrides filled, 2026-11-11 9:20 PM PST, engine evidence only (their LRCLIB records have no
  word times), `.before-word-fill.bak` beside each: "your presence" (`2b6d3a9a6d967f83`) 52/52
  lines, 206/226 words matched; "dopamine" (`545b7f6f3922d6ba`) 51 lines with words (the rest
  are one word), 131/220 matched. The engine hears dopamine's French as English nonsense ("Tu es
  tout ce que je veux" → "To it, two squash, your book"), so its 22 French/spelled-out lines are
  spaced by syllables inside Cameron's line times.
- [x] Released in v1.23.0 (`6e333ab`), 2026-11-11 9:10 PM PST.
- [ ] Step 6: offline batch, scheduled. Windows task "Jarlid word-timing hearing batch", once at
  2026-11-12 3:00 AM PST (start-when-available, 14 h limit, deletes itself a day after expiry).
  It runs `pythonw -I <scratchpad>/tools/batch_heard.py`: 173 cached line-only LRCLIB songs, chunks
  of 6, each through `srmctl run --slots 3 --low` with no window, into
  `%LOCALAPPDATA%\com.camer.pandora-desktop\lyrics-heard\<LRCLIB cache file name>` (the app's
  `StoredHeard` format). One test song stored 9:16 PM PST (`02a86868d6ce368a.json`), ~4.5 min a
  song, so ~7–11 h. Log: `<scratchpad>/batch-heard.log`. Re-runnable: skips songs already heard.

## Open questions for the user

Answered 2026-11-11: release yes (v1.23.0); batch yes, start 3 AM when the computer is idle;
fill the two overrides yes (done).

## Things not to do

- Don't re-time lines while adding words.
- Don't write into `lyrics-overrides` without a `.bak` beside the original.
