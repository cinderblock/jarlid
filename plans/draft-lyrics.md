# Draft lyrics: the speech engine writes them, a person reviews them

## Goal

For songs LRCLIB has **no lyrics for at all**, let the auto-sync speech engine write a timed
draft, and make reviewing that draft — then publishing it to LRCLIB — the fast path.

Cameron, 2026-10-26, on hearing that adding lyrics meant typing them into the editor: "I'm
not gonna be typing most of the words. the point was the ai engine does most of it", and
publish only "after thorough review". AI does most of the work; a person checks every line
before anything leaves the machine.

This is Step 8 of `plans/auto-synced-lyrics.md` ("transcription-from-nothing"). That plan
has the engine, sidecar, corpus and bench background.

## Environment / context

- **No engine change needed.** The sidecar already returns every heard word with start, end
  and a confidence `p`, on the track's clock, with window overlaps resolved
  (`crates/autosync-sidecar/src/protocol.rs`, `Event::Words`). Alignment already runs in the
  app; drafting can too. So drafts ship with an ordinary app release, with no new
  `autosync-v*` engine release and no re-pinned manifest.
- App side: `app/src-tauri/src/autosync/mod.rs`. `run()` currently returns early when LRCLIB
  has no plain lyrics (`let Some(plain) = lyrics.plain else { return Ok(()) }`). That is the
  hook.
- Parakeet v3 output has capitals and sentence punctuation, plus per-word `p`. Example,
  *Kryptonite*: "…troubled mind." / "…sands of time," / "watch the world flow(0.48) through
  the talks(0.44) of the mood" for "float to the dark side of the moon". **Low-p words are
  where the mistakes are**, which is exactly what a reviewer needs pointed out.
- Corpus for measuring: `%LOCALAPPDATA%\jarlid-autosync-spike\corpus\<track>\` (`plain.txt`,
  `synced.lrc`) with the shipped engine's cached output in
  `results\parakeet-cpu-q8_0w\<track>.txt`. Load with the bench's `parakeet_words`.
- Editor: `app/src/lyric-editor.ts` (inline; timestamp mode and correction mode; per-line check;
  Publish). Publishing a song with no LRCLIB record is a new-record publish.

## Decisions already made (don't re-ask)

- **AI writes, a person reviews, then publish** (Cameron, 2026-10-26). Never auto-publish.
- **Built in the app, not the sidecar**, for the same reason as alignment: improvements ship
  with app updates.
- Space in a timing pass must not also pause/resume the song (Cameron, 2026-10-26) — fixed in
  `0ff4248`.
- Cameron left the design to me ("whatever you like").
- Drafts toggle is **on by default** under auto-sync: auto-sync itself is the opt-in (it is
  the download), and a draft is only ever shown labelled as one.
- **Review state survives a save.** A saved edit of a half-reviewed draft keeps
  `Lyrics.draft` (`{doubt, reviewed}` per line) and is still a draft; it stops being one only
  when every line with words is ticked. `publish_lyrics` refuses while the saved edit is a
  draft, as a backstop to the editor's locked Publish button.

## Design

1. **Drafting** (`crates/autosync/src/draft.rs`, pure, tested, benched):
   heard words → lines. Break at long pauses, at sentence punctuation followed by a pause,
   and when a line gets too long; insert a blank (instrumental) line for long gaps. Keep each
   word's `p` so the UI can mark doubtful words.
2. **Gate for "there is singing here":** an instrumental gives few or no words. Below a word
   count / density threshold, store "nothing to draft" and keep showing "No lyrics found".
3. **Storage:** its own stored result kind beside the aligned ones, with provenance
   (engine, drafter version, the heard words), so a newer drafter can redo it without
   hearing the song again, the same way the aligner does.
4. **Display:** precedence stays local edit > LRCLIB > auto-aligned > plain > **draft** >
   nothing. A draft is clearly labelled, never passed off as lyrics: status "Draft · heard
   by the speech engine, not checked", a distinct style, and a "Review draft" action.
5. **Review:** the pencil opens the editor seeded with the draft's lines *and* timings
   (correction mode), with doubtful words marked. Every line has to be checked before
   Publish is offered for a draft (the "thorough review").
6. **Setting:** a toggle under Auto-synced lyrics, "Draft lyrics for songs with none".

## Plan / steps

1. [x] `draft.rs` + a corpus bench (`examples/draft.rs`): word error rate against LRCLIB's
   plain text, and how well draft line starts match the real line starts. Tuned (`5b73775`).
2. [x] App: run drafting when LRCLIB has nothing; store (`lyrics-draft/` in the cache dir,
   with the heard words and `p`); live `autosync://lines` with `draft`/`doubt`;
   `fetch_lyrics` serves it, or `noDraft` (`noSinging` / `unclear`) (`032c89c`).
3. [x] Pane: italic draft rows, dotted underline on doubtful words, status line with review
   progress; flags hidden for drafts; Settings toggle (`032c89c`, count fix `dda07e3`).
4. [x] Editor: seeded from the draft with doubt marks; a ✓ per line; the playing line marked
   and kept in view; Publish reads "N lines left to review" until done; Enter mid-line splits
   (timed second half gets an interpolated start), Backspace at a line's start joins it to the
   line above. Pure logic in `app/src/edit-lines.ts` with tests (`032c89c`).
5. [x] README (feature bullet + architecture note).
6. [ ] **First live run** in a real build, on a song LRCLIB has nothing for. ← current
   Only the frontend was exercised in a browser with a stubbed Tauri bridge (see Findings);
   the Rust path from hearing to a stored draft is covered by unit tests, not yet by a real
   song.
7. [ ] Release — only when Cameron says so (per the project's version-bump rules).

## Findings / gotchas

- **Bench** (`cargo run --release -p autosync --example draft -- <corpus> <results>
  parakeet-cpu-q8_0w`; `DRAFT_QUIET=1` for the summary; `DRAFT_<FIELD>=` overrides). Writes
  `<track>.draft.lrc` beside the model output with doubtful words in `[[brackets]]`.
- **Shipped defaults, 56 tracks** (4 get no draft, see below): **81% of drafted words are
  right** (2,995 of 16,116 wrong). WER against LRCLIB's text is 36%, which also counts words
  never heard and LRCLIB lines that aren't sung as written. 61% of real lines have a draft line
  starting within 0.5 s, 72% within 1 s; 59% of draft lines start within 0.5 s of a real one.
  Doubt marks (p < 0.8) cover 22% of words, catch 63% of the wrong ones, and 53% of marked
  words are wrong.
- **Line breaking plateaus around F1 59 at 0.5 s.** Pause, punctuation-pause, max words and
  max seconds trade recall for precision; no combination beat ~59. Sweep (punct / max s /
  max words / pause → lines, R.5, P.5, F1): baseline 0.3/6/12/0.7 → 2,687, 55, 59, 56.9;
  0.15/4/12/0.9 → 3,200, 61, 57, 58.9; 0.0/6/12/1.0 → 3,361, 62, 57, 59.4. Line count
  near the real 2,899 was the tie-breaker.
- **Lead matters more than the rules:** lines starting 0.25 s ahead of their first word
  lifted F1 from 58.9 to 60.9 (0.15 → 60.4, 0.3 → 60.9, 0.45 → 57.9, 0.6 → 46.5). Same
  direction as the aligner's 0.15 s.
- **A per-track offset doesn't explain the misses** (rec1 72% vs 72% with each track's
  median offset removed), unlike the alignment bench.
- **Doubt threshold sweep** (precision / recall of marks / share of words marked): 0.5 →
  68/30/8%, 0.6 → 62/42/13%, 0.7 → 58/53/18%, 0.8 → 53/64/23%, 0.9 → 47/75/31%. Chose 0.8:
  a missed mistake costs the reviewer more than an extra mark.
- **Mean confidence predicts a useless draft.** Every track under 0.75 mean `p` had WER ≥
  58%: Radiohead *Let Down* (9 words heard), Smashing Pumpkins *1979* 0.666 / 66%, Metallica
  *Wherever I May Roam* 0.676 / 74%, *Master of Puppets* 0.692 / 58%. Gate:
  `min_confidence = 0.75`, reported as `NoDraft::Unclear`; under 25 words is
  `NoDraft::NoSinging`.
- **UI checked in a browser with a stubbed Tauri bridge** (2026-10-26): Vite served `app/`
  with a script injected ahead of `main.ts` that fakes `__TAURI_INTERNALS__` (canned
  now-playing, a Kryptonite draft from the bench, a ticking playhead, an in-memory override
  store). Kept outside the repo, in `%LOCALAPPDATA%\Temp\draftwork\preview\`
  (`tauri-stub.js`, `vite.preview.config.mjs`; run Vite from `app/` with
  `JARLID_APP=<app dir>` and `--config` pointing at it; port 1530). Verified: pane styling and
  status; the editor's ✓, Publish lock and label; split with interpolated start; join with the
  caret at the join; save mid-review, close, reopen restores ticks; a full review saves as a
  plain edit ("Synced lyrics · edited"); the `noDraft` status; a live draft preview
  ("drafting 2 of 9"). Found and fixed: the status count included blank lines (`dda07e3`).
  The config can't `import "vite"` from outside the project; it exports a plain object.
- **Untested risk: songs in languages Parakeet doesn't know.** No such track in the corpus.
  Expectation is low confidence → `Unclear`, but it needs a live check. Also no instrumentals
  in the corpus to confirm `NoSinging`.

## Progress log

- [x] 2026-10-26 — Space-in-timing-pass fix (`0ff4248`).
- [x] 2026-10-26 — Confirmed the sidecar already gives everything drafting needs.
- [x] 2026-10-26 — `crates/autosync/src/draft.rs`, the corpus bench, tuning (`5b73775`).
- [x] 2026-10-26 — App, pane, editor review, settings (`032c89c`); status count fix
      (`dda07e3`); README.
- [ ] First live run on a real song with no LRCLIB lyrics.
- [ ] Release (Cameron's call).

## Open questions for the user

1. **First live run is yours**: with auto-sync on, play songs LRCLIB has nothing for (or wait
   for one) and review a draft. Worth noting which songs end up "too unclear", especially
   non-English ones.
2. **Release?** Nothing is bumped. Say when, and whether it's a minor (new feature → 1.16.0
   is my recommendation).

## Possible follow-ups (not started)

- A key to tick the line that just played, so a review can be done without the mouse.
- Re-hearing with vocal isolation (parent plan's Step 7) would help the `unclear` songs most.

## Things not to do

- Don't auto-publish a draft, or offer Publish before every line is checked.
- Don't show a draft as if it were real lyrics.
- Don't change the sidecar protocol for this; it isn't needed.
