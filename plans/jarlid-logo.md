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

- **The jar is a small mason jar**, not a pithos. Cameron rejected the committed pithos logo
  (`a53a7b9`): "Those are lamps/vases... not jars." The pithos/vase silhouette is out entirely.
- (Superseded) Concept 2 of the pithos round, "music pushing the lid off", was picked and
  shipped in `a53a7b9`. The pipeline (two SVGs + `scripts/build-icons.py`) stays; the art gets
  replaced.
- **3D isometric, not flat.** Cameron rejected all three flat mason-jar concepts (M1-M3) with
  "None. more 3D isometric please." The jar is modeled in 3D (`iso.py` in the scratch dir):
  surfaces of revolution and boxes, isometric projection (elevation 35.26°, azimuth 45°),
  per-face lighting, emitted as SVG.
- **Contents: music notes escaping from being trapped.** Cameron: not a fan of the glowing jar,
  and the 3D bar charts aren't good either. Clear glass, notes inside the jar, others flying
  out of the open mouth. No glow and no bars.
- **Final pick: N3, "breaking free."** An isometric glass mason jar with the lid popped off to
  the upper right, one eighth note trapped inside, and a big beamed pair of notes escaping
  upper left along a dotted trail. Shipped in `86baeba`.
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
8. Mason jar redo, isometric 3D, notes escaping. Done: N3 picked, refined, committed `86baeba`.
   The renderer moved into the repo as `app/src-tauri/icons/source/jarlid-art.py`. It writes
   both SVGs, and `scripts/build-icons.py` runs it first.

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
- Mason-jar round (`gen4.py` → `mason-large.png` in the scratch dir): M1 gold bars in the glass
  pushing the lid off, M2 glowing jar with music notes escaping, M3 lid knocked ajar with light
  leaking out. Blue-tinted glass (like old Ball jars) plus a ridged metal screw band. Its
  silhouette reads at 16 px far better than the pithos did.
- Isometric round (`iso.py` → `iso.png`): I1 glass jar holding 3D gold bars (the tallest
  pushing the lid off), I2 glowing jar with the lid lifted, I3 glowing jar with the lid knocked
  ajar. Gotchas hit while building the renderer: a row of bars along world (x, -z) projects
  onto one screen point, so run it along (x, +z) to get a screen-horizontal row; up-facing lid
  faces blow out to white unless their diffuse/specular is turned down; a thin lid band reads
  as a coin (height 64 against radius 146 works); the glow fill has to cover the whole base
  ellipse or it shows a hard edge.
- Escaping-notes round (`notes.py`, imports `iso.py` → `notes.png`): notes are extruded 3D
  glyphs (stacked darker copies stepped down-right under a gradient face), with dotted gold
  flight paths. N1 has two notes trapped inside and three streaming out, lid popped off to the
  right. N3 has one note trapped and one big beamed pair breaking free. N2 (lid hinged
  half-open on the rim) looked like the lid was falling *into* the jar, so it wasn't shown.
  Rotating the lid about world z doesn't give a convincing hinge in this view.
- An image embedded in chat didn't reach Cameron at least once ("show them to me"). Open the
  sheet in his image viewer with `Start-Process <png>` as well.
- Final-art fixes: extruded notes built from a few coarse copies show stair-steps at full size,
  so extrude in sub-pixel steps (~0.6 px each). A raised ring on the lid top half-renders as
  a stray "C" (half its faces are culled), so use a flat raised disc instead. The lid needs the
  extra 24° screen spin from N3 or it looks like a coin on edge. The jar's floor shadow spilled
  below the tile, so everything is clipped to the tile shape.
- Small art (16-32 px): jar scaled 1.1 and full-bleed tile, glass edge 24 and mouth ring 18
  (a 32-wide ring read as a donut), note scale 2.05, no trail, no back rings.
- Pillow's ICO writer stores PNG entries; `cargo check` (tauri-build + `generate_context!`)
  accepts it.

## Progress log

- [x] Concepts drafted (round 2: pithos + glow / + equalizer bars / + sound arcs; round 1 J monogram)
- [x] Pick made (concept 2)
- [x] Final + small art drawn; icon set generated; every ICO entry checked visually
- [x] `cargo check` passes with the new icons
- [x] Committed `a53a7b9` (pithos; superseded)
- [x] Mason jar: flat concepts rejected; isometric concepts; notes concepts; N3 picked
- [x] N3 refined, generator in repo, icon set rebuilt, `cargo check` passes, committed `86baeba`
- [ ] Seen live in the taskbar (happens with the next release; no version bump from this task)

## Open questions for the user

1. ~~Which concept?~~ Answered: N3 (after the pithos concept 2 was rejected).
2. Optionally, show the mark on the sign-in screen too? The app draws no logo in its own UI
   today. Not started; only if Cameron wants it.

## Things not to do

- Don't bump the version alongside the icon commit (see `8200fab` in CLAUDE.md).
- Don't hand-edit `jarlid.svg` / `jarlid-small.svg`; they're generated by `jarlid-art.py`.
- Don't bring back the pithos/vase shape, the glow, or bar charts (all rejected).
