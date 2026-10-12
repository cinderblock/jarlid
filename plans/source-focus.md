# One owner for "which source do the controls drive" — Living Plan

Plan path: `plans/source-focus.md`

Status: **shipped in v1.11.5 (`1f23b8e`); untested on hardware** (2026-10-12).

## Goal

User report (2026-10-12), after v1.11.4:

1. Pausing local playback sometimes hands the screen to the WiiM, even when the WiiM is paused.
2. Sometimes the UI shows the local pause button but media keys go to the house speakers.
3. Wants a local / speaker toggle that is **always visible**.
4. Wants a **miniature playback view** of the house speakers while they play and the local
   player owns the screen.

## Cause (1 and 2)

Remote-vs-local was decided twice — `updateMode()` in `app/src/main.ts` (screen, play
button, Space) and the `remote://state` listener in `app/src-tauri/src/lib.rs`
(`remote_active`: SMTC media keys, taskbar thumbnail buttons). They used the same rule, but
**the backend only re-evaluated it when `remote://state` fired, and `upnp.rs` emits that only
when the polled state changes.** A paused WiiM never changes, so:

- WiiM paused (sticky remote) → user starts a local station → the UI drops remote mode on the
  first local playhead motion, but the backend never hears anything and keeps
  `remote_active = true`. Media keys / taskbar buttons go to the WiiM (symptom 2).
- In that state a play/pause **Toggle** key computes its command from `playing_now`, which is
  the *WiiM's* (paused) state, so it sends `play` to the WiiM. The WiiM starts, local keeps
  going; when the user then pauses local, 3 s later the level-triggered rule ("renderer
  playing and local idle 3 s") hands the screen to the WiiM (symptom 1).
- Independently of the stale flag, the level-triggered rule means any time the WiiM is
  playing in the background, pausing local hands it the screen.

## Design

### Single owner: `app/src-tauri/src/focus.rs`

One `Focus` state, fed directly by the sources (not via event listeners):

- `native.rs` playhead ticker (every 250 ms) → `focus::local_playhead(app, position)`;
  motion-derived "moving" like everywhere else (>0.05 s jump = moved, idle after 1.6 s).
- `upnp.rs` poll loop (every 1 s, every successful poll, after emitting `remote://state`) →
  `focus::remote_state(app, playing, has_track)`; also on unreachable (no track).
- Commands `remote_focus` (get) and `remote_focus_set(remote)` (the toggle);
  `remote_play_station` (cast) chooses the speaker on success.

On change it emits `remote://focus { remote }`. The UI's `remoteMode` follows it; SMTC/taskbar
routing reads `focus::is_remote()`; `crate::remote_active()` (used by updates.rs) delegates.

### Rules — edges, not levels

- **Whichever source *starts* playing takes focus**: local starting (idle → moving) → local;
  the speaker starting (paused → playing) → speaker, *only if local isn't playing*.
- **Pausing never moves focus.** Neither does a source that is merely still playing.
- Speaker loses its track / becomes unreachable → local.
- Toggle: explicit choice. Choosing the speaker when it has no track yet is remembered
  (`pending`) and applied the moment it gets one (cast race).
- First poll after launch counts as an edge, so a speaker already playing at startup takes
  the screen if local is idle (same as before).

### UI

- `#source-toggle`: segmented control at the top of the meta column (replaces the
  "Now playing on …" badge and the "Back to this computer" pill). "This computer" | device
  name. Shown whenever a renderer has been discovered. Each segment shows a small live
  indicator when that source is audibly playing. Speaker segment is disabled with inline
  "· idle" when it has no track.
- `#mini`: compact card (art, title, artist · where) for the *other* source while it plays
  (symmetric: the speakers while local has focus, and this computer while the speakers have
  it). Own play/pause button drives that source directly; clicking the card switches focus.
  After pausing from the mini it stays (so it can be resumed) until focus changes.

## Decisions already made (don't re-ask)

- Backend owns the decision; the UI never computes focus itself. (Two copies of the rule is
  what produced this bug and the v1.11.2/1.11.3 follow-ups.)
- Edge-triggered takeover replaces level-triggered + sticky. The mini view makes a playing
  background source visible, so it doesn't need to grab the screen.

## Findings / gotchas

- `upnp.rs` emits `remote://state` only on change (`if st != last`) — any listener-driven
  rule on top of it is blind while the speaker sits paused.
- Tauri 2 queues an `emit` made from inside a Rust listener (handlers lock is held), so
  feeding focus from the source loops directly avoids ordering surprises.
- SMTC metadata was never re-pushed when leaving remote mode (it kept the WiiM's metadata
  until the next local track). Fixed by caching both sources' last metadata and applying the
  focused one on every focus change.

## Progress log

- [x] focus.rs + 10 unit tests (incl. the reported bug: local starting while the speaker sits
      paused and no renderer poll arrives).
- [x] native.rs / upnp.rs feed it; lib.rs routing + SMTC re-push on focus change; commands
      `remote_focus` / `remote_focus_set` replace `remote_release`; cast chooses the speaker.
- [x] UI: toggle, mini view, follow `remote://focus`; badge + "Back to this computer" removed.
- [x] README remote-mode bullet.
- [x] `bun run build`, `cargo check`, `cargo test --lib` (97 passed) all pass.
- [x] Headless-Chrome mockup of toggle + mini (state faked, no backend) — layout OK.
- [x] Released as v1.11.5: Release workflow succeeded; installers, signatures and `latest.json` uploaded.
- [ ] Verify on hardware: pause speaker → start local station → media keys/taskbar control
      local; pause local with speaker playing → screen stays local, mini shows speaker;
      toggle both ways while paused; mini pause/resume; cast from Stations takes the screen.

## Open questions for the user

1. Mini player resuming *this computer* brings it on screen (local starting takes focus);
   resuming the *speaker* from the mini while local plays does not. Consistent with the rule,
   but say if either should behave differently.

## Things not to do

- Don't reintroduce a second copy of the focus rule in the UI.
- Don't make pausing either source move focus.
