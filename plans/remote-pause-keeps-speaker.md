# Pausing the WiiM keeps it on screen — Living Plan

Plan path: `plans/remote-pause-keeps-speaker.md`

Status: **shipped in v1.11.2 (`5d7b1e4`); untested on hardware.**

## Goal

User report (2026-10-04): playing on the warehouse WiiM, pressing pause made the UI flip back
to the local player, so pressing play again started local playback instead of resuming the
speaker.

## Cause

Remote mode was defined as "the renderer is *playing* and local has been idle 3s", in two
places that must agree:

- `app/src/main.ts` `updateMode()` — owns the screen and where the play button/Space go.
- `app/src-tauri/src/lib.rs` `remote://state` listener — `remote_active`, which routes SMTC
  media keys and the taskbar thumbnail buttons.

The moment the WiiM reported `pause`, `playing` went false, both dropped out of remote mode,
and the next Play went to the local engine.

## Fix

Remote mode is sticky: it is *entered* only when the renderer plays, but once in, it stays
while the renderer still has a track and local playback hasn't moved in 3s. Exits: local
playback starts (e.g. picking a station on the Stations page), or the renderer loses its
title / becomes unreachable.

## Findings / gotchas

- LinkPlay `getPlayerStatus.status` is `play`/`pause`/`stop`/`load(ing)`; a paused WiiM keeps
  its title, which is what makes the stickiness hold.
- Unknown: whether a WiiM left paused for a long time eventually clears its title (which would
  hand the screen back on its own). Not observed either way.
- `rustfmt --check src/lib.rs` follows `mod` into other files that are already unformatted;
  only the lib.rs hunk was checked/fixed.

## Progress log

- [x] Both rules made sticky; README remote-mode bullet updated.
- [x] `bun run build`, `cargo check` pass.
- [x] Released as v1.11.2: Release workflow succeeded; installers, signatures and `latest.json` uploaded.
- [ ] Verify on the warehouse WiiM: pause → screen stays; play → speaker resumes; media key
      play/pause while paused goes to the speaker; picking a local station still takes over.
