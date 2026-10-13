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
- [ ] Backend: `lyrics_candidates`, `choose_lyrics`, `clear_lyrics_choice`, pin honoured in
      `fetch_lyrics`, `pinned` output flag. Better `pick_best` ranking (own commit).
- [ ] Frontend: versions button in `.lyrics-head`, candidate list, live preview, use / back /
      return to automatic.
- [ ] `cargo test`, `bun run build`.
- [ ] Run in the real app against a multi-version track (needs Cameron).

## Things not to do

- Don't store pins in the cache dir. A cache wipe would silently undo the user's choice.
- Don't let a preview overwrite `lastLyrics`. `revalidateLyrics` and the editor both key off it.
