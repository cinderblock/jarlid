# Pausing the WiiM keeps it on screen — Living Plan

Plan path: `plans/remote-pause-keeps-speaker.md`

Status: **shipped in v1.11.2 (`5d7b1e4`); "Back to this computer" follow-up shipped in v1.11.3 (`a22b8ff`); untested on hardware.**

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

## Follow-up: no way back to local (2026-10-05)

User report: with the speaker paused and owning the screen, nothing returns to the local
player short of starting a station — the play button resumes the speaker.

Fix: a **Back to this computer** pill under the "Now playing on …" badge, shown only while
the speaker is paused and there is a local track to go back to. Clicking it leaves remote
mode in the UI (`setRemoteMode(false)` in `main.ts`) and calls the new `remote_release`
command, which sets `REMOTE_RELEASE`; the `remote://state` listener in `lib.rs` consumes it
and drops `remote_active`, so media keys and the taskbar buttons go back to local too.

The exit holds without any extra "dismissed" state: both rules only *enter* remote mode
when the speaker is playing, so a released, still-paused speaker stays off screen until it
plays again (e.g. resumed from the WiiM app). The button is hidden while the speaker plays
for the same reason — a release then would be undone by the next 1 s poll. It does not
start local playback; the local track comes back paused and Play then plays it here.

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
- [x] "Back to this computer" button + `remote_release` command; `bun run build`, `cargo check` pass.
- [x] Released as v1.11.3: Release workflow succeeded; installers, signatures and `latest.json` uploaded.
- [ ] Verify on hardware: pause speaker → button appears; click → local track on screen,
      Play/media key plays locally; resume from the WiiM app → speaker takes the screen again.
