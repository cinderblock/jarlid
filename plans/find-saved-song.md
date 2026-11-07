# Find a saved song and play it

## Goal

Let the listener search the songs already saved on disk (the opt-in audio cache, "Saved songs" in
Settings) for the station they are on, and jump straight to one. Asked for on 2026-11-06.

## Environment / context

- Cache: `crates/engine/src/cache.rs`, dir `%LOCALAPPDATA%/com.camer.pandora-desktop/audio/`, one
  file per song named from `pandora_id` (`TR_12345.mp3`). See `plans/audio-track-cache.md`.
- Engine: `crates/engine/src/lib.rs` (`State.current`, `queue`, `source_of`, `cache_next`).
- App commands: `app/src-tauri/src/native.rs`; registered in `app/src-tauri/src/lib.rs`.
- UI: `app/index.html`, `app/src/main.ts`, new `app/src/find-song.ts`, `app/src/styles.css`.

## Decisions already made (don't re-ask)

- **Only songs saved while on this station** are offered. Feedback (thumbs) uses the current
  station token with the song's stored track token; a song from another station would thumb the
  wrong station. So the engine refuses a song not recorded against the current station.
- **Only when Saved songs is on.** Settings promises that turning it off stops the cache being read.
  The panel says so and links to Settings instead.
- **Local playback only.** The cache is this computer's; the button hides while the screen shows the
  network speakers.
- **After the jumped-to song, the station carries on** with the queue it already had. The jump does
  not touch the queue, so nothing is re-requested from Pandora.

## Design

- The cache files carry no metadata, so each song gets a sidecar `<stem>.json` (`SavedSong`:
  title, artist, album, length, art, track token, rating, source station id, and the station tokens
  it was saved on). Written whenever a song is downloaded into the cache, found there as next, or
  played from it, so songs cached before this feature appear once they come round again.
- Eviction deletes a song's sidecar with it, and sweeps sidecars whose audio is gone.
- `Fetched.at` becomes `Option<Instant>`: a jumped-to song was not issued by a playlist request, so
  its failure must not retire the queue's batch.
- `Engine::play_saved(id)`: checks station, takes the file, makes it `current`, plays it, announces
  `TrackStarted`. The `Track`'s `audio_url` is the file path (the stored signed URL has expired).

## Progress log

- [x] cache.rs: `SavedSong`, `remember`, `saved_songs`, sidecar eviction, tests
- [x] engine: `play_saved`, remember on cache hits/stores, `Fetched.at` optional
- [x] native.rs + lib.rs: `saved_songs`, `native_play_saved`
- [x] UI: topbar magnifier, `find-song.ts` panel, `song-search.ts` (accent/punctuation-folding
      match, title-prefix ranking, tested), empty / off / no-match states, Ctrl+F, arrows + Enter
- [x] README
- [x] Checks (2026-11-06): `cargo test -p engine` 26 pass, app `cargo test --lib` 132 pass,
      `bun test` 65 pass, `bun run build` clean, `rustfmt --check` on touched files
- [x] Headless browser harness with a mocked Tauri bridge (15 checks pass: list, search, Enter
      plays the cursor row, Ctrl+F/Escape, off state → Settings, empty state, 420px light theme)
- [ ] **Real app check (Cameron)**: with Saved songs on, listen a few songs, open Find a song,
      jump to one, check it plays from disk, the station continues after it, and a thumb on it
      registers. Not done by an agent: it needs a Pandora session and would take the stream.

## Findings / gotchas

- Songs saved before this change have no sidecar and can't be listed: nothing else on disk maps a
  `pandora_id` to a title. They gain one the next time they are queued or played from the cache.
- Browser harness: temp Vite config (`root` = `app/`) injecting a mock `__TAURI_INTERNALS__`,
  driven by `playwright-core` with the cached `chrome-headless-shell`, off screen. The mock's
  null `build_id` raises an unrelated "Something went wrong" toast; ignore it.

- Unverified: whether a track token from an earlier session still accepts feedback. If Pandora
  rejects it, the thumb rolls back with an error toast, which is the existing behaviour.

## Things not to do

- Don't offer songs from other stations (see feedback above).
- Don't store the signed `audioURL` in the sidecar: it is a credential and is expired anyway.
