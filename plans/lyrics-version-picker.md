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

## Follow-up under discussion — dual-lyric (two-language) records (2026-10-13)

Not started; waiting on Cameron's go-ahead.

- "Split Brain Version" seems specific to Madelline's "Dopamine" (`q=split brain` finds nothing else
  of the kind). Dual-language *lines* are a general pattern, though.
- LRCLIB's Lyricsfile 1.0 (`server/src/lyricsfile.rs` in tranxuanthang/lrclib) has `lines[]` with
  `text`, `start_ms`, `end_ms` and `words`, and nothing for a second voice or translation. So any
  detection has to come from the text.
- Encodings seen in the Dopamine records: `FR - EN` (35090140), `EN (FR)` (11043109),
  `FR (EN)` (33421579). Translated LRC files elsewhere also use two lines with the same timestamp.
- The trap: parentheses usually mean backing vocals (`(ooh)`, echoes), not a second language.
  Treat it as dual only when the split is consistent across most lines of the file.
- Proposed display: two lines per row. The main voice is shown as usual, the second smaller and dimmer
  beneath it, and both light up together. The status line says "· 2 languages" with a swap/hide toggle,
  remembered per track.
