# Riding out an audio-device outage (driver update)

## Goal

A graphics driver update (2026-10-07) restarted the display adapter, which takes its HDMI/DP
audio endpoints down with it. Playback died, lost its place in the song, skipped a run of tracks
as "unplayable" while the driver came back, and only then recovered. Make a device outage cost
silence for as long as the device is gone, and nothing else: same track, same position, no skips.

## Diagnosis (from code)

The stall-recovery work (`plans/audio-stall-recovery.md`) treats every failure as either "the
track" or "nothing", and a device outage gets misfiled as the track's fault twice over:

1. **Device errors spend the track's recovery budget.** The cpal error callback sets
   `device_error`; the watchdog rebuilds on the next 50 ms poll and counts it against
   `MAX_RECOVERIES = 3`. With the endpoint flapping during a driver restart, three rebuilds burn
   through in well under a second and the track is published `failed`.
2. **A rebuild that can't open a device is read as an expired URL.** `Player::play_on` resolves
   the device first; with no endpoint present it errors, and the build step's `Err` arm assumes
   "signed link expired", drops `current` (losing the position) and publishes `failed`.
3. **`failed` makes the engine retire the whole batch** (`retire_batch_of_current`) and advance.
   The next track hits the same missing device, fails the same way, and so on. That's the run of
   skipped songs.

"Playback stalled" (queued audio nobody is consuming) is a device symptom too, and it was
also counted against the track.

## Decisions

- Classify failures by **whose fault**: a new `audio::Error::Device` for everything on the output
  side of `play_on` (resolve, config, build stream, start stream). Only stream-side failures
  can fail a track.
- Device trouble keeps `current` and its `resume_at`, never publishes `failed`, and retries on a
  backoff (500 ms doubling to 5 s) for as long as it takes. Skipping tracks can't fix a missing
  speaker, so there's nothing to gain by giving up.
- While waiting, a cheap probe (`audio::output_ready`) gates each retry, so an absent device
  doesn't cost a CDN request per attempt. Pandora's one-stream-per-account rule makes idle
  re-opens worth avoiding.
- If the device comes back after the signed URL has expired, re-opening fails as a *stream*
  error and the engine skips as before. That's never worse than today.

## Progress

- [x] `Error::Device`, `output_ready()` in `crates/audio`
- [x] Audio-thread device backoff; device reasons don't count against `MAX_RECOVERIES`
- [x] Build, clippy, tests
- [ ] Real-world confirmation: next driver update, or disable/re-enable the output device in
      Device Manager mid-song. Should go silent, then resume at the same spot with no skips.

## Things not to do

- Don't fold device failures back into `MAX_RECOVERIES`; that's the bug.
- Don't retry `play_on` blindly every poll while no device exists: each attempt after the device
  resolves opens a CDN connection.
