# One button for stations, and cast on every row — Living Plan

Plan path: `plans/stations-cast-combined.md`

Status: **done.** Feature in `bd3f021`; v1.9.3 bumped and tagged after it.

Follows `plans/stations-modal.md`, which put the station name top-right beside the
all-stations button.

## Goal

Three asks from the user on 2026-09-19:

1. The station name pill and the "all stations" button both open the Stations panel. Combine
   them into one control.
2. The "cast" (speakers) button folds in too: every station row on the Stations page gets its
   own cast button, and the separate preset popover goes.
3. Cut a new patch version (v1.9.3) when done — its own version-only commit, then the tag.

## Environment / context

- `app/` — Vite + TS. `bun run build` = `tsc && vite build`; the build is the gate. Rust side
  at `app/src-tauri/` (`cargo check` there, not the repo root).
- Casting is WiiM/LinkPlay presets: Jarlid cannot start an arbitrary Pandora station on the
  speaker. `remote_presets` lists the device's presets (name, source, art, number);
  `remote_cmd` `preset:N` fires one. Generic UPnP renderers have no presets.
- The release workflow runs on a pushed `v*` tag; the version commit is what the tag points at.

## Decisions already made (don't re-ask)

- **One pill: list icon + station name.** The name is empty before anything plays, and hidden
  in remote mode, so the pill collapses to the round icon button on its own. No fallback
  label — the icon is the label.
- **A row's cast button plays the preset whose name matches the station** (case-insensitive,
  trimmed; presets whose `source` names another service are skipped). Every row still gets
  the button; one with no matching preset is dim and clicking it says how to fix that in the
  WiiM Home app, in the panel's status line, rather than silently doing nothing.
- **Casting pauses local playback.** Asking the speakers to play means moving playback there;
  otherwise two sources run at once and the display stays on the local engine.
- **Cast buttons only appear when a LinkPlay renderer is discovered** and its presets were
  fetched. They hide in select mode, where the page is about picking.
- **Non-Pandora presets are no longer reachable from Jarlid.** The old popover listed every
  preset; the combined design only lists stations. Accepted loss, flagged to the user.
- Feature is one commit; the version bump is a separate version-only commit (`CLAUDE.md`).

## Plan / steps

1. [x] Plan written.
2. [x] `index.html`: merge `#stations-btn` into `#station` (icon + `#station-name` span);
   remove `#speakers-btn` and `#speakers-panel`.
3. [x] `main.ts`: name painting and flight use the span; drop the stations-btn and preset
   panel code; forward the remote device name to the stations page.
4. [x] `stations-page.ts`: rows become `div[role=button]` (a button cannot hold a button);
   fetch presets on device change and on open; per-row cast button; wide layout column.
5. [x] `styles.css`: pill with icon, remote-mode hides the name only, cast button styles,
   drop the popover styles, extra grid column.
6. [x] README: the preset-list sentence.
7. [x] `bun run build`; commit feature; version bump commit; tag; push.

## Findings / gotchas

- `.sp-row` was a `<button>` containing an `<input>` in select mode (already invalid nesting);
  the cast button forces the switch to a `div` with keyboard handling.
- Opening the page re-queries presets, and the station name is mid-flight into its row at
  that moment. Re-rendering on every answer detached the landing row; the page now
  re-renders only when the preset list actually changed.
- Verified in a plain browser via Vite on port 1430: the pill is exactly 34×34 with no
  name and grows to a pill with one. The cast buttons could not be exercised there — they
  need a WiiM answering `remote_presets` — so that path is unverified against hardware.

## Progress log

- [x] Explored the code, wrote this plan.
- [x] Feature implemented, built, committed as `bd3f021`.
- [x] v1.9.3 version-only commit and tag.

## Things not to do

- Don't bump the version inside the feature commit.
- Don't run bare `cargo fmt`.
