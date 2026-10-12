# Lyrics cache: expire hits, remember misses briefly

## Goal

The disk cache in `app/src-tauri/src/lyrics.rs` had two opposite failures, raised
2026-10-12:

- **Hits never expired.** Once a track's LRCLIB lyrics were cached, that copy was shown
  forever. Corrections others published, a synced version added later, or a fix for a
  wrong match were never seen.
- **Misses were never remembered.** A track with no lyrics on LRCLIB cost one to three
  slow LRCLIB requests (10 s timeout each, with retries) on every play.

## Environment / context

- Backend: `fetch_lyrics` in `app/src-tauri/src/lyrics.rs`. Order: local override → disk
  cache (`app_cache_dir()/lyrics/<fnv>.json`) → LRCLIB `/api/get` → `/api/search`.
- Frontend: `loadLyricsFor` → `applyLyrics` in `app/src/main.ts`. `Lyrics` type in
  `app/src/lyric-editor.ts`.
- What already dropped a cache entry: publishing a correction and clearing a local edit.

## Decisions already made (don't re-ask)

- **Miss TTL: 2 hours** (user's choice). A fresh cached miss returns "No lyrics found"
  without touching the network. After that, the next play looks again.
- **Hit TTL: 1 day, stale-while-revalidate.** A stale hit is still shown right away (it is
  almost always still right). The frontend then asks for a refresh in the background and
  repaints only if LRCLIB now serves something different.
- **A refresh that finds nothing keeps the old hit.** LRCLIB never deletes records, so a
  miss after a hit is search variance, not news. The old hit's timestamp is renewed so
  it isn't re-checked on every play.
- **Old cache files carry no timestamp.** They're treated as stale, so they get one
  background refresh and pick up a timestamp. No wipe is needed.
- **A timestamp in the future (clock moved back) counts as stale.**
- **Flagging a record also drops its cache entry**, like publishing does.
- **The background refresh doesn't repaint while the lyric editor is open**, because the
  editor holds the record identity it would publish against. The refreshed copy is
  already on disk and shows on the next play.

## Plan / steps

1. Backend: `CacheEntry { cachedAt, ...Lyrics }` on disk, plus TTL checks, a `refresh`
   argument to `fetch_lyrics`, a `stale` flag on the returned `Lyrics`, and cache
   eviction in `flag_lyrics`.
2. Frontend: background revalidation when `stale`, and pass the key args to `flag_lyrics`.
3. Tests for freshness; run cargo test, clippy, and `bun run build`. Commit.

## Progress log

- [x] Backend. `CacheEntry` flattens `Lyrics` plus `cachedAt`, so old bare-`Lyrics` files
  still load. `lookup_lrclib` split out of `fetch_lyrics` and returns `Option<Lyrics>`.
  `stale` is `skip_deserializing` and is serialized only when true.
- [x] Frontend. `revalidateLyrics` in `main.ts` runs after a stale hit is painted. It skips
  the repaint if the track changed, the pane moved on, the editor is open, or nothing
  differs.
- [x] Checks: `cargo test --lib` passes (101 tests, including 4 new cache tests), rustfmt is
  clean on `lyrics.rs`, and `bun run build` passes. The clippy warnings in `lyrics.rs` are
  all in code that was already there.
- [x] README feature line updated.
- [ ] Not yet seen live in the running app (cache files in `%LOCALAPPDATA%`).

## Findings / gotchas

- An LRCLIB record with no text (an instrumental) is now cached as a miss with the 2 h TTL.
  Before, it wasn't cached at all.
- The `(cached)` suffix on `source` is added on read, never stored. The renew-on-miss path
  writes before adding the suffix, so it doesn't pile up.
