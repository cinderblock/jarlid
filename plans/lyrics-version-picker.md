# Lyrics version picker — see the other matches, choose the right one

## Goal

LRCLIB search returns up to 20 records per track and `pick_best` chooses one. It gets
some wrong. Let the listener see the other records that were found, try them against the
playing song, and pin the right one for that track.

Triggering case (2026-10-13): Madelline — "Dopamine". Many versions exist (English,
French, French + English, "Split Brain" — two languages at once, which LRC can't really
express).

## Environment / context

- Backend: `app/src-tauri/src/lyrics.rs` (`fetch_lyrics`, `lookup_lrclib`, `pick_best`).
  Commands registered in `app/src-tauri/src/lib.rs`.
- Frontend: `app/src/main.ts` (`loadLyricsFor`, `applyLyrics`, `revalidateLyrics`),
  `app/src/lyric-editor.ts` (edit mode owns the pane while active), markup in
  `app/index.html` (`.lyrics-head`), styles in `app/src/styles.css`.
- See `plans/lyrics-corrections.md` (overrides and publishing) and
  `plans/lyrics-cache-ttl.md` (cache expiry).

## Findings

- **Why the wrong pick happens:** for `track_name=Dopamine&artist_name=Madelline` LRCLIB returns
  20 records, all synced, and 18 of them are 165–166s long. `pick_best` ranked only by
  (has synced, duration gap), so it took whichever record came first in LRCLIB's order.
  That was id `33935652` (album "Dopamine (Split Brain Version)"). Title and album were never
  compared at all.
- `/api/search` is capped at 20 results.
- Search results carry the full lyrics, so a candidate can be previewed and chosen without
  another request.

## Decisions

- A choice is a **pin by record id**, stored in the *data* dir (`lyrics-picks/`), keyed
  like overrides by Pandora's naming. A pin is the user's own decision, like an override, so
  it lives in the data dir rather than the disposable cache. When a cache entry expires,
  the refresh fetches `/api/get/{id}` for the pinned record and skips the heuristic.
- A pin survives publishing a correction. LRCLIB republish adds a revision to the
  same track record (same identity fields), so the id stays valid.
- A local edit still wins over a pin. The picker says so and doesn't offer a choice while
  an edit exists.
- Picking previews **in the pane, live with the playhead**. Watching the lines light up
  against the song is the fastest way to judge a match.
- Separately, `pick_best` now also compares title and album. That fixes the common case
  without anyone opening the picker.

## Plan / steps

- [x] Investigate the Dopamine case against the live API.
- [x] Better ranking: `rank` = (title, same cut ≤3s, synced, album, gap). Commit `0aad0c1`.
      Unit tests use the real Dopamine result shape.
- [x] Backend: `lyrics_candidates` (track+artist searches, plus free-text `q`, run
      concurrently, deduped, other artists sorted last), `choose_lyrics`, `clear_lyrics_choice`,
      pin honoured in `fetch_lyrics` (refreshes go to `/api/get/{id}`; a 404 drops the pin),
      `pinned` output-only flag. Commit `b1f85f5`.
- [x] Frontend: `app/src/lyric-picker.ts`. Layers button beside the pencil, list with
      length vs playing, synced/plain, opening lines, and a "Showing now" / "Your choice" / "Best match" label.
      Click previews in the pane live; ‹ › or arrow keys step; Esc backs out; "Use this" pins;
      "Back to automatic" unpins. Commit `b1f85f5`.
- [x] `cargo test --lib` (106 pass), `bun run build` clean, all `$("id")` lookups resolve.
- [x] Checked live: `/api/get/{id}?` → 200, unknown id → 404 `TrackNotFound`.
- [ ] **Run in the real app against a multi-version track (needs Cameron).** Things to watch:
      list layout in the pane, preview highlighting, the pin surviving a restart, and
      "Back to automatic" re-matching.

## Notes

- Cached picks made before `0aad0c1` stay until their 24h TTL expires, so a wrong pick can
  survive a day after the ranking fix. Choosing a version replaces it immediately.

## Things not to do

- Don't store pins in the cache dir. A cache wipe would silently undo the user's choice.
- Don't let a preview overwrite `lastLyrics`. `revalidateLyrics` and the editor both key off it.

## Two-language records (2026-10-13, built after Cameron's go-ahead)

Decided: tag them in the picker **and** show them as two languages anywhere detection is confident.

- "Split Brain Version" seems specific to Madelline's "Dopamine" (`q=split brain` finds nothing else
  of the kind). Two-language *lines* are a general pattern, though.
- LRCLIB's Lyricsfile 1.0 (`server/src/lyricsfile.rs` in tranxuanthang/lrclib) has `lines[]` with only
  `text`, `start_ms`, `end_ms` and `words`, and nothing for a second voice or translation. So detection
  has to come from the text.
- Ways records carry it: `FR - EN` (35090140), `EN (FR)` (11043109), `FR (EN)` (33421579), and
  elsewhere two lines on one timestamp. ` / ` and ` | ` splits are handled too.

### Implementation

- `app/src/dual-lyrics.ts`, with `detectDual` and `guessLanguage`. A file is dual only if at least 60% of
  its lyric lines (and at least 4) split the same way into a translation-like pair. Translation-like
  means neither half is vocables, the lengths are comparable (skipped across scripts, since Korean is a
  third of the characters of the same English line), and the halves share at most a third of their
  3+-letter words. Pairs whose halves share words ("alike") count neither for nor against a file, but are
  still split in a dual file. If both sides guess as the same language, it's an echo and is rejected.
- `app/src/lrc.ts` holds `parseLrc` (moved out of main.ts) and `dualOf`, which the picker uses.
- Pane: `.line.dual` has `.l-main` plus a smaller, dimmer `.l-alt`. They stay one `.line`, so the
  highlighter's indexes don't change. The `#lyrics-dual` chip ("FR + EN") cycles both, swapped, first
  only and second only, saved per track in `localStorage` as `dual:<key>`. It is hidden in edit mode,
  where the raw lines show.
- Picker rows show "FR + EN" for dual versions.
- Tests: `app/src/dual-lyrics.test.ts` (`bun run test`), 12 cases using invented lines in each real
  record's shape, plus the backing-vocal traps. tsconfig excludes `*.test.ts`.

### Findings

- On 388 real LRCLIB records (two samples, including K-pop, J-pop and Latin), the synced and plain paths
  each detect the 3 Split Brain records and nothing else.
- The first cut missed 2 of 3: "Da-ra-da-da-da" wasn't treated as filler, and same-in-both-languages lines
  ("ma D-O-P-A-M-I-N-E" / "my D-O-P-A-M-I-N-E") counted against the file. Both fixed as above.
- LOVE PSYCHEDELICO "Place Of Love" is Japanese with English words mixed in, not a translation.
  It is correctly not dual.
- **Tooling trap:** editing TS through a bash heredoc into a Bun template literal turned a `\r?\n` regex
  into a literal CR/LF. Use the Edit tool for anything containing backslashes or apostrophes.

### Steps

- [x] Detector, tests, pane display, chip, picker tag, README.
- [ ] See it in the real app on Dopamine (Split Brain): readability of the subtitle line, chip cycling,
      and the highlight staying centred when rows change height.
- [x] **Released as v1.12.0** (`daaf953`, 2026-10-13), on Cameron's call, without waiting for the
      audio buffer change to be confirmed under load. The run concluded `success`, and the release
      has the setup `.exe`, the `.msi`, both `.sig` files and `latest.json`.
