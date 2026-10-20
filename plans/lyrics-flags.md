# Lyrics flags — marking lyrics as wrong, whole song or a stretch of it

## Goal

Let the listener flag lyrics as having something wrong: the whole file, or certain timing
areas. Requested 2026-10-19: "Need a way to 'flag' lyrics as having something wrong. either
whole file or certain timing areas. eventually we'll need more... but we can start here".

Pandora is radio, so a flagged song can't be played again on demand. A flag is only worth
something if a later reader can find it without the song coming back round. That is why the
flags have a list, and why each one carries enough to stand on its own.

## Environment / context

- Lyrics backend: `app/src-tauri/src/lyrics.rs`. User-owned per-track state lives in the
  **data** dir, one hashed JSON per track keyed by `key_for(artist, track, album)`:
  `lyrics-overrides/` (edits), `lyrics-picks/` (version pins). The cache dir holds
  disposable things: `lyrics/` (LRCLIB), `lyrics-auto/` (auto-sync results, which carry
  `heard` words and the aligner version; see `autosync/mod.rs` `Stored`).
- Frontend: `app/src/main.ts` (pane rendering, `highlightLine`, `applyLyrics`),
  `app/src/lyric-editor.ts` (edit mode owns the pane), `app/src/lyric-picker.ts`
  (version picker owns the pane), `app/index.html` `.lyrics-head`.
- The existing **"Report…"** button in edit mode is an *upstream* LRCLIB flag
  (`flag_lyrics`, `POST /api/flag`): public, one-way, whole record only. Not this feature.

## Decisions already made (don't re-ask)

Answered 2026-10-19:

- **Purpose: both** a review list for the listener *and* auto-sync feedback. Flags are local
  and carry full provenance (source, LRCLIB record id, a fingerprint of the text shown,
  auto-sync aligner version). Flags on auto-synced lyrics also snapshot what the engine
  heard, so they can become aligner bench cases later, even after the cache is wiped.
- **Gesture: quick key + line pick.** `F` (or the flag button) flags the line playing now,
  instantly, without interrupting anything. The flag button also opens a flag mode in the
  pane: click / shift-click lines to set a range, flag the whole song, add a note, remove a
  flag. Each flag stores both the lines and the time span.
- Flags are stored in the data dir (user's own work), never the cache.

## Design

### Storage (`app/src-tauri/src/lyric_flags.rs`)

`<data dir>/lyrics-flags/<fnv of key_for(...)>.json`, one file per track:

- `TrackFlags { artist, track, album, duration, flags: [Flag] }`. Pandora's naming is
  stored inside the file, so the Settings list (and any later tool) can enumerate the
  directory without reversing hashes.
- `Flag { id, created, span: Option<Span>, tags: [string], note, shown: Shown }`.
  `span: None` is the whole song.
- `Span { first, last, start, end, text }`. `first`/`last` are pane rows, inclusive,
  counted the way the pane counts them (two-language pairs are one row). `start`/`end`
  are on the **lyrics' clock** (playhead + the `[`/`]` offset), from the first row's stamp
  to the next row's, or the song's end. `None` for untimed lyrics. `text` copies the
  flagged rows' words so the flag reads on its own.
- `Shown` is the whole set of lyrics on screen when the flag was made: `source`, record
  `id`, `synced`, `plain`, `auto`/`overridden`/`pinned`, the `offset` in effect, and for
  auto-synced lyrics `autosync: Evidence { engine, aligner, coverage, heard }` taken from
  the `lyrics-auto/` cache record. That is everything the bench needs to re-run the
  aligner on the song without the audio.
- The evidence is the backend's to make: on save it is carried over by flag id from the
  stored file, or read fresh for a new flag. The frontend never has to round-trip it.
- `tags` are strings (`timing`, `words`, `version` so far), so adding kinds later needs no
  migration. This is the "we'll need more" extension point.
- Saving an empty list deletes the file.

### Whether a flag still applies

A flag is about the text it was made on. It applies to what's shown when `shown.synced`
and `shown.plain` match exactly. Otherwise (edited since, LRCLIB revised it, a newer
aligner re-timed it) it is "on an earlier version": not drawn on the rows, counted in the
flag bar, and listed in Settings. Nothing deletes it automatically: the listener decides
whether the fix fixed it.

### UI

- **Flag button** in `.lyrics-head`, beside the pencil and versions, same quiet style.
  Shown when there are lyrics and neither the editor nor the picker is open. Lit when the
  track has flags that apply.
- **`F`** flags "the line playing now" without opening anything. If that row started under
  `QUICK_GRACE` (1.5 s) ago, the row before is included, since you notice after you hear it.
  A blank row steps back to the last row with words. Before the first line, it flags the
  first. Feedback goes through the status line, like `[`/`]`. On plain (untimed) lyrics
  there is no "now", so `F` opens flag mode instead.
- **Adjacent or overlapping stretches on the same text merge**, and so do two with only blank
  rows (a verse break) between them. So tapping `F` line by line through a bad patch builds
  one stretch, not a dozen. The merged flag keeps the oldest id, its tags and notes combined.
- **Flag mode** (the flag button): the pane keeps playing and highlighting. Click a row to
  flag it, shift-click to stretch the selected flag to there, and click a flagged row to
  select its flag. The bar has tag chips, a note, Remove, "Whole song", and Done. Changes
  save as they happen. Esc closes it. Auto-scroll holds off for a few seconds after the
  listener scrolls or clicks, so it doesn't snatch the row they're reaching for.
- **Rows** that a flag covers get a wavy underline (spell-check squiggle: "something's
  wrong here"), quiet normally and stronger in flag mode. The selected flag's rows get a
  background.
- **Settings → Flagged lyrics:** every flagged track, newest first, with what was flagged,
  where the lyrics came from, and Remove. Plus "Show folder" for the files themselves.

## Plan / steps

1. [x] Backend: `lyric_flags.rs` (`lyric_flags`, `save_lyric_flags`, `all_lyric_flags`,
       `lyric_flags_reveal`), `autosync::evidence`. Unit tests.
2. [x] Frontend: pure logic in `app/src/flags.ts` (+ `flags.test.ts`); UI in
       `app/src/lyric-flags.ts`; hooks in `main.ts`; markup + CSS.
3. [x] Settings: "Flagged lyrics" list.
4. [x] README.
5. [x] Checks: `cargo test --lib` (120 pass), `bun test` (33 pass), `bun run build`, all 159
       `$()` ids resolve.
6. [x] Driven in a browser against a mocked Tauri bridge (see Findings).
7. [ ] **Try it in the real app (Cameron).** Things to watch: `F` on a live song, the squiggle's
       visibility in both themes, flag mode while the song plays, a flag on an auto-synced song
       (the file in `lyrics-flags/` should carry `autosync.heard`), the Settings list. ← current

## Findings / gotchas

- **Saving before the load would have wiped flags.** The frontend saves its whole list, so an
  `F` pressed before the track's flags were read from disk would have written `[new flag]` over
  the file. Flagging is now off until the load succeeds (`loaded` in `lyric-flags.ts`), and
  `lyric_flags` returns an error for a file it can't parse rather than "nothing flagged".
- **Flag feedback in the status line goes stale**, unlike the `[`/`]` nudge's "offset +0.25s",
  because the flag can grow or go a moment later. The status reverts after 3 s to what it said
  before the first of a run of flags (`flagNote` in `main.ts`).
- Flagging during a live auto-sync pass is fine: auto-synced flags match on the plain words,
  not the timings, so they stay on their rows as the timings fill in or are re-timed. The
  `autosync` evidence is only filled in once a stored result exists; a flag saved before the
  pass finished gets it on its next save, not otherwise.
- **Browser harness** (how the UI was checked without starting a second player beside the
  real one): a temp Vite config with `root` = `app/` and a `transformIndexHtml` plugin that
  injects a mock `window.__TAURI_INTERNALS__` (`invoke`, `transformCallback`, and
  `plugin:event|listen` recording handlers so a test can emit `engine://nowplaying` /
  `engine://playhead`). The mock needs `update_status` to return an object, or `onPlayhead`
  throws. Read the mock per request, not at config load, or edits need a server restart.
  Checked there: `F` with the grace rule, merging, flag mode (select, shift-click stretch,
  tags, note, whole song, remove, Esc), auto-scroll hold, Settings list and two-step Remove
  clearing the pane's marks, `F` ignored while Settings is open.

## Progress log

- [x] 2026-10-19: design settled with Cameron (both purposes; quick key + line pick).
- [x] 2026-10-19: built, unit-tested, checked in the browser harness.

## Open questions for the user

None. Later ideas, not started: ticks for flagged stretches on the progress bar; removing a
single old-version flag from Settings (today it's the whole song's flags); exporting flagged
auto-sync songs straight into the aligner bench corpus.

## Things not to do

- Don't save flags for a track before its file has been read (see Findings).
- Don't take `shown.autosync` from the frontend; the backend carries it by flag id.

- Don't add a `.line` element to the pane for anything that isn't a lyric line
  (`highlightLine` indexes `.line` positionally).
- No `title=` tooltips, no native controls; `attachTip` and app-drawn controls only.
