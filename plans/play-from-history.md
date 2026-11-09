# Play from history, and a longer history

Plan path: `plans/play-from-history.md`

## Goal

Asked for on 2026-11-09: "I should be able to easily play recently played songs, and scroll
further back in the play history."

Two halves:

1. **Play** a song from the recently-played history — from the strip's cover card and from a
   full history list — whenever its audio is saved on this computer.
2. **Go further back** than the strip's 40 covers: keep a much longer history and give it a
   page of its own that scrolls through all of it.

## Environment / context

- History today: `main.ts` "recently-played gallery". `history: HistItem[]` in `localStorage`
  key `history`, newest-first, capped at 40. Click a cover → `#hist-modal`, info only.
- Saved songs (opt-in audio cache): `crates/engine/src/cache.rs` sidecars (`SavedSong`, with
  the station tokens each was saved on). `Engine::play_saved(id)` plays one from disk, but only
  if it was saved on the station playing now. UI: `find-song.ts` (`plans/find-saved-song.md`).
- Pandora gives no on-demand playback: a song can only be replayed if its audio is on disk.
- Commands live in `app/src-tauri/src/native.rs`, registered in `app/src-tauri/src/lib.rs`.

## Decisions already made (don't re-ask)

- **Only songs saved on disk can be played.** Anything else is listed but says why it can't
  play. No attempt to ask Pandora for a specific track.
- **Songs from another station play by switching to that station**, then playing the saved
  song first; the station's own queue follows. Thumbs then go to the station the song belongs
  to, which is the reason `play_saved` refuses cross-station songs today — switching keeps
  that guarantee instead of breaking it. (My call; flagged to Cameron.)
- **History entries gain `id` (pandora id) and `station` token** going forward, so matching to
  saved songs is exact. Older entries fall back to title + artist.
- **The strip keeps showing the most recent 40**; the full list (cap 1000) lives on a History
  page. The strip's coverflow measures every child on each scroll frame, so it should not hold
  1000 covers.
- History stays in `localStorage` (not worth a migration); writes are wrapped so a full quota
  trims the list rather than throwing.

## Plan / steps

1. [x] Engine: `play_saved_on(name, token, id)` switches station and plays the saved song;
       `all_saved_songs()`; `station_token()`. `cache::all_saved`.
2. [x] native.rs: `id` and `stationToken` on `nowplaying`; `saved_index` command;
       `native_play_saved` takes optional `stationName` / `stationToken`.
3. [x] UI: `play-history.ts` (pure: store, matching, day grouping; 14 tests) and
       `history-page.ts` (the page: search, days, play on click, arrows / PgUp / PgDn / Enter).
4. [x] Strip: card gets "Play again" / "Play on <station>" and "All history"; an
       All-history tile at the strip's oldest end; a clock button in the top bar; Ctrl+H.
5. [x] Checks (2026-11-09): engine 26 pass, app `cargo test --lib` 132 pass, `bun test` 79
       pass, `bun run build` clean, `rustfmt --check` clean on touched files. Headless
       harness 22/23 (the one miss was the harness's own expectation; see findings).
6. [x] README; commit.
7. [ ] **Real app check (Cameron)**: with Saved songs on, play a few songs on two stations,
       then from History play one from this station (queue continues after it) and one from
       the other (station switches, the pill and the mode follow, thumbs land there).

## Findings / gotchas

- **The station token has to travel on `nowplaying`.** `station-active` is emitted by the
  command after the switch returns, and the engine's `TrackStarted` can be turned into a
  `nowplaying` before that, so the history would have recorded the *previous* station.
- A history row with `aria-disabled="true"` cannot be clicked by Playwright without
  `force: true` — correct behaviour, harness detail.
- Search matches every word anywhere (as Find a song does), so "song 3" also finds
  "Artist 3"; the harness's first expectation was wrong, not the page.
- The sticky day heading uses `var(--page)` as its background; on the light theme that is a
  faintly lighter band over the translucent panel. Judged acceptable; revisit if it bothers.
- Harness: `dist/` served by a tiny node server, mocked `__TAURI_INTERNALS__` via
  `addInitScript`, `chromium_headless_shell-1248`, off screen. The "Something went wrong"
  toast is the mock answering `null` to unrelated commands.

## Progress log

- [x] Engine + commands, tests.
- [x] History store, page, strip, card, top-bar button.
- [x] Verified headless in dark, light, and at 420px wide.
- [ ] Cameron's real-app check (step 7).

## Open questions for the user

1. Cross-station play switches the station (my call, so thumbs stay correct). If you'd rather
   it never switch, only same-station songs would be playable from History.

## Things not to do

- Don't reverse the stored history array (see `plans/recents-direction.md`).
- No `title=` tooltips; no native controls.
- Don't run bare `cargo fmt`.
