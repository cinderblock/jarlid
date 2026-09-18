# Unsynced lyrics follow the song

## Goal

Plain lyrics (no LRC timestamps) used to sit parked at the top of the pane for the whole song.
No line can be highlighted without timestamps, but the pane can still drift through the text in
proportion to how far through the track we are, so the line being sung is on screen.

## Decisions already made

- **Estimate, not a highlight.** Nothing is lit; only the scroll position moves. Lighting a
  guessed line would claim a precision we do not have.
- **Time is warped by a lead-in and a tail.** Vocals rarely start at 0:00 or run to the last
  second. Constants `PLAIN_LEAD_S` (12 s), `PLAIN_TAIL_S` (15 s), each capped at 6 % of the
  track (`PLAIN_EDGE_FRAC`). Progress is linear across the remainder. Tunable, all in
  `app/src/main.ts` next to `followPlainLyrics`.
- **The reader wins.** A wheel, touch, or pointer-down on the pane stops the follow for
  `PLAIN_HOLD_MS` (4 s of stillness), then it resumes from wherever they left the pane.
- **Own easing, not CSS smooth scroll.** The playhead ticks ~4×/s and each tick moves the target
  a few pixels; `scroll-behavior: smooth` restarted per tick looks steppy. A rAF glide from a
  fractional copy of scrollTop (`plainPos`) keeps it continuous even though the DOM rounds
  scrollTop to whole pixels.

## How it is wired

- `followLyrics(position, duration)` is the single playhead entry point: synced lyrics go to
  `highlightLine`, plain ones to `followPlainLyrics`. Called from both the local playhead
  handler and the remote (network player) 400 ms interpolation tick.
- `stopPlainFollow()` runs on every repaint of the pane (synced render, plain render, the
  "unavailable" and "no lyrics" branches) so a stale glide never fights a new song.
- Editing mode is skipped entirely, same as the synced highlighter.

## Progress

- [x] Proportional follow with lead/tail warp, rAF glide, user-scroll hold.
- [x] Both playhead paths (local, remote) go through `followLyrics`.
- [x] README feature bullet.
- [x] `bun run build` (tsc + vite) passes.
- [ ] Not exercised in the running app this session: needs a signed-in session on a track that
      LRCLIB only has plain lyrics for. Things to eyeball: the drift speed feels right, the pane
      does not fight the wheel, and a new song starts at the top.

## Possible follow-ups (not done, not asked for)

- Let `[` / `]` nudge the plain estimate the way they nudge the synced offset.
- Apply the same user-scroll hold to synced lyrics, which still re-centre on every line change.
