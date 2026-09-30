# Jarlid logo

## Goal

Give Jarlid a logo of its own. Every icon in `app/src-tauri/icons/` is still the stock Tauri
placeholder (the yellow/cyan swirl) from the initial commit `b8e1627`, so the taskbar, window,
installer and Start menu all show Tauri's mark rather than Jarlid's.

## Environment / context

- Icon set: `app/src-tauri/icons/` (32x32, 128x128, 128x128@2x, icon.ico, icon.icns, icon.png,
  Square*Logo.png / StoreLogo.png for MSIX). Listed in `app/src-tauri/tauri.conf.json` → `bundle.icon`.
- `bunx tauri icon <source>` (in `app/`) regenerates the whole set from one PNG/SVG source.
- Renderers on this machine: `resvg` (`~/.cargo/bin/resvg`), Python 3.12 + Pillow 12.1.
- App palette (`app/src/styles.css`): dark bg `#0a0a0c`, dark accent `#6ec1ff`, light accent
  `#1372c4`, font Inter / Segoe UI.
- The name: *Jarlid* = the lid of the jar. Pandora's "box" was really a *pithos* (large storage
  jar); lifting its lid let everything out, and Hope stayed inside.
- The app draws no logo in its own UI (sign-in screen, settings); the icon is only the
  window/taskbar/installer icon. The taskbar thumbnail toolbar glyphs are separate, not affected.

## Decisions already made (don't re-ask)

- Concept 2, "music pushing the lid off": a pithos in the app's blue, three gold level bars
  rising from the mouth, the tallest tipping the lid up. Chosen by Cameron over light-spilling
  (1), sound arcs (3) and a J monogram (4). Concept sheet: `%LOCALAPPDATA%\Temp\jarlid-logo\concepts.png`.
- Dark tile at every size, so the icon keeps its contrast on both light and dark taskbars.

## Plan / steps

1. Draft concepts, render at 512 / 48 / 32 / 16 on dark and light backgrounds. Done.
2. Self-critique legibility at 16/32 px; iterate. Done (two rounds).
3. Show concepts to Cameron, get a pick. Done: concept 2.
4. Refine the pick; separate small-size art. Done.
5. Commit the sources and regenerate the icon set. Done: `a53a7b9`.
6. README "Develop" section documents the sources and `scripts/build-icons.py`. Done.
7. See it in the real taskbar with the next release build (not done: the installed Jarlid was
   running and another session held dev port 1420, so no dev instance was launched).

## Findings / gotchas

- Scratch work lives in `%LOCALAPPDATA%\Temp\jarlid-logo\` (`gen.py` round 1, `gen2.py` round 2,
  `concepts.png` side-by-side). Not in the repo; the chosen SVG gets copied in.
- Round 1: a jar with a flat, cut-off bottom reads as a bowl. It needs a tapered foot to read as
  a pithos. Content at ~60% of the tile vanished at 16 px; round 2 fills more of it.
- A soft radial glow looks good at 512 but turns to brown mud at ≤32 px. Keep hard shapes for the
  small sizes.
- Even round 2 is marginal at 16 px inside a tile. So 16-32 px get separate art, hand-assembled
  into the .ico rather than `tauri icon` downscaling one source. That art keeps the dark tile
  (run full-bleed) instead of going tile-less: a light-blue jar with no tile has no contrast
  on a light taskbar.
- Final layout: sources in `app/src-tauri/icons/source/` (`jarlid.svg`, `jarlid-small.svg`);
  `scripts/build-icons.py` runs `tauri icon` for the full set, overwrites `32x32.png` and
  `Square30x30Logo.png` with the small art, and builds `icon.ico` with Pillow from exact-size
  renders (16/20/24/32 small art; 40/48/64/256 full art). It only writes files already in
  `icons/`, because `tauri icon` also emits Android/iOS sets this app doesn't ship.
- In round 2, the gold bars' orange ends showed around the rim's rounded corners. Fix: end
  the bars inside the rim (bottom at y=520), not below it.
- Pillow's ICO writer stores PNG entries; `cargo check` (tauri-build + `generate_context!`)
  accepts it.

## Progress log

- [x] Concepts drafted (round 2: pithos + glow / + equalizer bars / + sound arcs; round 1 J monogram)
- [x] Pick made (concept 2)
- [x] Final + small art drawn; icon set generated; every ICO entry checked visually
- [x] `cargo check` passes with the new icons
- [x] Committed `a53a7b9`
- [ ] Seen live in the taskbar (happens with the next release; no version bump from this task)

## Open questions for the user

1. ~~Which concept?~~ Answered: 2.
2. Optionally, show the mark on the sign-in screen too? The app draws no logo in its own UI
   today. Not started; only if Cameron wants it.

## Things not to do

- Don't bump the version alongside the icon commit (see `8200fab` in CLAUDE.md).
