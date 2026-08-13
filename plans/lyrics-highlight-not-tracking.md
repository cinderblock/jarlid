# Lyrics stop highlighting the current line

**Status: OPEN — narrowed, not solved. Needs one observation from the app to finish.**

## Symptom (user, 2026-08-11)

> does pausing break lyrics synchronization? two songs in a row aren't showing highlighted current
> lyrics

Lyrics are on screen; the current line is not highlighted. Suspected by the user to follow a pause.

## What has been ruled out (with evidence, don't redo these)

1. **The lyrics lookup.** The on-disk cache
   (`%LOCALAPPDATA%\com.camer.pandora-desktop\lyrics`) records what LRCLIB actually returned. Both
   tracks from the window in question (08-11 13:15 and 13:20) have `synced` populated, from
   `lrclib/get`. So `syncedLines` was parsed and `renderSyncedLyrics` ran — this is *not* a
   plain-lyrics fallback, which would have no highlight by design.
2. **The CSS.** `.line.active` exists (`styles.css:778`) and uses `--fg` against `.line`'s
   `--faint`; both are defined with real contrast in *both* themes (`:root` and
   `:root[data-theme="light"]`). Not a casualty of the theme refactor.
3. **Remote (network player) mode.** This looked promising — pausing makes local playback idle,
   which is exactly the condition that flips the UI into remote mode — but remote mode is fully
   wired: a 400 ms interpolator (`main.ts:968`) drives the progress bar *and* `highlightLine(pos)`
   off the device's reported position. Not it.

## Where that leaves it

`highlightLine` is called unconditionally on every playhead tick (`main.ts:433`), and it early-returns
only when `syncedLines` is empty — which the cache says it is not. Everything else it does is
derived from **the position it is handed**. So the suspicion is that the reported position no
longer matches the audio.

**Prime suspect: the resume-with-seek added for `plans/audio-stall-recovery.md`.**
`Player::play_at` asks Media Foundation to seek to an offset and then *seeds the position clock to
the offset it asked for*, on the assumption the seek landed there. If `SetCurrentPosition` returns
`S_OK` but lands somewhere else — or silently no-ops on a network MP3 — audio and reported position
diverge permanently, and the highlight runs at a fixed error for the rest of that track. The unit
test `seek_skips_ahead` only proves seeking works on a local WAV; it says nothing about an HTTP MP3.

Note this suspect does not obviously explain *two songs in a row*, since a fresh track starts a
fresh player at offset zero. Either the user paused twice, or the cause is elsewhere — that
tension is unresolved and worth taking seriously rather than explaining away.

## The one observation that would settle it

While the highlight is broken, in the app:

- **Is the elapsed-time counter moving, and does it agree with the music?** If the timer runs ahead
  of what is audible, it is the seek-seeding bug above. If the timer is frozen while audio plays,
  the position is not being published at all — a different bug, in `audio_thread`'s publish path.
  If the timer is correct and the line still is not highlighted, the fault is in `highlightLine`
  or the DOM, and both hypotheses above are wrong.
- Secondarily: what does the lyrics status line say — "Synced lyrics" or "Lyrics"?

## If it is the seek

`play_at` should report where the seek *actually* landed rather than where it was aimed. Media
Foundation gives the true timestamp back on the first decoded sample (`ReadSample`'s
`plltimestamp`, currently passed `None`), so the honest fix is to read the first sample's
timestamp and seed `frames_played` from that.
