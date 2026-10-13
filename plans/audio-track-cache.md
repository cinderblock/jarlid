# Opt-in audio cache for repeated songs

## Goal

Pandora repeats songs, and every repeat downloads the whole track again (~3–5 MB at 128 kbit/s
MP3). Keep the songs we have already downloaded on disk, so a repeat is played from there instead.
This is **off unless the user turns it on**, and the user picks the size.

## Decisions already made (don't re-ask)

- **Opt-in.** Pandora's terms generally don't allow keeping their audio outside their own offline
  feature. The user accepted that as long as caching is off by default (2026-10-13).
- **The user picks the size in Settings, and each choice shows roughly how many songs it holds.**
  The estimate uses the real average size of what is already cached, and falls back to a nominal
  128 kbit/s × ~3.8 min song (~3.6 MB) when the cache is too small to average.
- **Download the *next* track in full while the current one plays, then play it from disk.** This
  costs no extra bandwidth on a first play: one complete download replaces the stream it would have
  been. Tee-ing the live stream into a file would also avoid that cost, but it means writing our own
  `IMFByteStream`, and that complexity isn't worth it. Downloading the *current* track in parallel
  with streaming it would double the bandwidth on every first play, which defeats the point.
  - Consequence: the first track after a station change (or a song skipped to before its download
    finished) is streamed and not cached. That is acceptable.
- **The cache key is `pandora_id`**, which is stable. The signed `audioURL` changes on every play.
  Tracks that carry a `key` (XOR-masked audio, never seen in practice) are never cached.
- **Evict the least recently played first.** A file's modified time is bumped each time it plays.
  Files that are playing now or queued next are never evicted.
- **The cache goes in the app cache dir** (`…/Local/com.camer.pandora-desktop/audio/`): it can be
  rebuilt, unlike the lyrics overrides, which live in the data dir.

## Plan

- [x] `engine::cache`: find / take (touches mtime) / store (`.part` then rename, length checked,
      format sniffed from the first bytes) / evict (LRU, pins current + next, sweeps stale
      `.part`) / stats / clear. Unit tests include a local HTTP server: a complete download, a
      truncated one and an HTML error page.
- [x] Engine: `set_audio_cache(Option<CacheConfig>)`. `advance_as`, `replay` and the blend
      prefetch play the cached path when there is one. The run loop downloads the next queued
      track once per track, on a spawned task. Music only: never adverts or keyed tracks.
- [x] App settings: `audioCache { enabled, maxMb }`, default off / 1024. Pushed at `attach` and on
      every `set_settings`. Commands `audio_cache_stats` and `audio_cache_clear` work signed out.
- [x] Settings UI ("Saved songs"): app-drawn checkbox, size picker 256 MB to 10 GB with
      "about N songs" on each option (measured average once 5+ songs are saved, otherwise 4 min at
      128 kbit/s), usage line, and a two-step Delete button. Turning saving off keeps the files
      and says so, with the Delete button. Checked in a browser preview with a mocked backend.
- [x] README.
- [x] Media Foundation plays a local `.mp3` and an ADTS `.aac` (`ff f1`) through `Player`
      (verified with `examples/device-buffer.rs`).
- [ ] Verify in the real app with a real station: the next track lands in
      `%LOCALAPPDATA%/com.camer.pandora-desktop/audio/`, a repeat plays from it, and with the
      cache off nothing is written. Not done here: it needs a Pandora playlist request, which
      would take the stream from the user's own running session.

## Findings / gotchas

- Pandora's CDN `Content-Type` was never checked: probing it needs a playlist request. Sniffing
  the bytes makes that irrelevant.
- Media Foundation already opens local paths (`prefetch` tests use a `.wav` on disk). It chooses a
  byte-stream handler by **file extension**, so cached files have to keep a real one (`.mp3`, or
  `.aac` for ADTS).

## Things not to do

- Don't download the currently streaming track in parallel just to cache it (it doubles bandwidth).
- Don't let a cache failure stop playback. Every cache error falls back to streaming the URL.

## Shipped

In v1.12.0 (2026-10-13). It was released before the open verification items above were
checked, at the user's call. Those items still stand, now against the released build.
