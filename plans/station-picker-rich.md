# The Station Picker Becomes The Stations Page — Living Plan

Plan path: `plans/station-picker-rich.md`

Status: **done and committed** (`5b35b54`, `895f151`, `bbb1745`, `cdcb1bb`, `e75f3f4`).
Nothing outstanding.

## Goal

From the user, 2026-09-17:

> "clicking the current radio station should animate rotating to horizontal and being
> part of the list. and list should have album art too, and be richer. Add some
> sorting too. could be a fuller-screen modal instead of this small popup"

The station name is a vertical spine up the album art's left edge (`edda67b`). Clicking
it opens a 280px dropdown with a flat, text-only list. The ask is that the spine
*becomes* a row in a fuller list — the rotation is the transition, not an ornament.

## Environment / context

- `app/` — Vite + TS frontend. `bun run build` = `tsc && vite build`; no lint config, the
  build is the gate. **Port 1420 is `strictPort` and shared with other worktrees** — use
  `bun run dev -- --port 1430`.
- `crates/` is its own cargo workspace (`audio`, `engine`, `pandora`); the Tauri app is a
  separate manifest at `app/src-tauri/`. **There is no `Cargo.toml` at the repo root.**
- `cargo fmt` with no arguments reformats code you did not touch — `rustfmt --check <file>`
  the files in your diff instead. `crates/pandora/src/models.rs` has two pre-existing
  rustfmt complaints (a double blank line at ~138, something in `picks_appropriate_art`);
  they are not ours and were left alone.
- The two station lists as found: `#station-panel` (the dropdown, in `main.ts`) and
  `#stations-page` (`stations-page.ts` — search, three groups, click-to-play, select mode,
  export/import).

## Decisions already made (don't re-ask)

Chosen by the user on 2026-09-17 from a measured three-way comparison:

- **One station UI, not two.** The spine opens the existing Stations page and that page
  gets the art, the sorting and the richer rows. The dropdown goes away.
- **Art comes from Pandora, with fallbacks.** `includeStationArtUrl: true` on
  `user.getStationList`, falling back to the last track Jarlid played from that station,
  then to a tile drawn from the name.
- **All four sort orders**: A–Z, recently played here, most played here, date created.
  Pandora's own order stays the default.

Inherited from `plans/station-galaxy.md`, and still binding:

- **The list stays one column.** The user asked for it explicitly: *"don't make my eyes
  scan in two dimensions at once"*. Finding a known name is a search task and a grid makes
  it slower. Art goes in the row, not in a card grid.
- **Grouping (Mixes / Your stations / Genre stations) replaced per-row tags.** A heading
  that says it once beats a badge repeated down every row. Don't reintroduce the badges.
- **Never use thumb counts as a proxy for listening.** They measure opinion, not time.

## Findings / gotchas

- **`TunerStation` had no art and no date, and the API does have both.** Two probes in
  `crates/pandora/examples` already pass `includeStationArtUrl: true`, so the flag is
  known-good in this codebase; `plans/station-galaxy.md` records `dateCreated` being seen
  on the same response. Neither is verified against a live account from here — both are
  written to degrade to nothing rather than break. (`5b35b54`)
- **Pandora timestamps are Java `Date`s serialised whole**: `{"time": 1378411431095,
  "year": 113, "month": 8, …}`, where every field but `time` is that instant written out
  again. `epoch_ms_opt` in `models.rs` takes `time` and tolerates a bare number.
- **There is no per-station play count or last-played anywhere in the API.** Confirmed in
  `plans/station-galaxy.md` against both `user.getStationList` and `station.getStation`.
  Two of the four sort orders therefore have to be counted locally, which also builds the
  counter the galaxy view wants for bubble sizing.
- **The recents list stores tracks, not the station they came from** (`history` in
  `localStorage`: art, title, artist, album, at). So "recently played" cannot be derived
  from what is already persisted — it needs its own record, keyed by token. The token was
  not available in the frontend either: `nowplaying` carries the station's *name*. The new
  `engine://station-active` event comes out of `save_last_station`, the one place both
  switching and resume-at-launch pass through with the token in hand.
- **Measure the flight's endpoints with a `Range` over the text, not with the elements.**
  The spine's box is the full height of the album art while its text is a short run in the
  middle; a row's `.sp-name` box spans the whole text column while its words sit at the
  left (centre 368 where the words are at 142). Both ends would have been visibly wrong.
- **Put the flight's easing on the keyframes, not on the effect.** An effect-level easing
  warps the *offsets* as well as the motion, so a `cubic-bezier(0.2, 0.7, 0.2, 1)` had the
  copy 90% of the way there at 25% of the duration and began the opacity cross-fade — which
  sat at offset 0.78 — halfway through the flight. It read as a snap followed by a wait.
- **`fill: "forwards"` on a WAAPI flight that ends somewhere other than its start.** Without
  it the element reverts to its untransformed position for the frame between the animation
  finishing and the cleanup removing it.
- **A `finish` event is dispatched on a frame.** A window that is not on screen barely gets
  any, so cleanup that hangs only off `finish` can be arbitrarily late — click the spine,
  alt-tab, and the spine stays hidden until you come back. There is a `setTimeout` beside
  the listener and `land` is safe to run twice.

## Testing this in a browser (both traps cost real time)

- **The automation browser reports `prefers-reduced-motion: reduce`**, so the flight
  correctly does nothing and looks broken. Stub `window.matchMedia` for that one query
  before clicking.
- **A backgrounded tab barely renders**, so sampling an animation by wall-clock shows it
  frozen at frame 0 and then finished. Grab the `Animation` off `element.getAnimations()`,
  `pause()` it and set `currentTime` to scrub it deterministically instead.
- **`import("/src/stations-page.ts")` from the console is a *different module instance*
  from the one `main.ts` is using**, because Vite gives an edited module an HMR query
  (`?t=1789681541407`). Both render into the same DOM, so the symptom is a page that opens
  empty after you just handed it ten stations. Read the real specifier out of the served
  source first: `fetch("/src/main.ts")`, match `from "(/src/stations-page\.ts[^"]*)"`, and
  import that.

## Plan / steps

1. ~~Rust + types: `artUrl` and `dateCreated` on the station list.~~ `5b35b54`
2. ~~Per-station play stats in `localStorage`, keyed by token.~~ `895f151`
3. ~~Richer rows: art with its fallback chain, and a second line worth reading.~~ `bbb1745`
4. ~~The sort control, over the existing grouping.~~ `cdcb1bb`
5. ~~The spine opens the page and rotates into its row. Dropdown deleted.~~ `e75f3f4`

## Progress log

- [x] Explored both station lists, the Rust station path, and the two prior plans.
- [x] Asked the user the three questions the work actually turned on.
- [x] `artUrl` + `dateCreated` through `TunerStation` → `station_payload` → `StationInfo`,
      with tests for every timestamp shape — `5b35b54`.
- [x] Per-station stats, and `engine://station-active` to key them by token — `895f151`.
- [x] Rich rows: cover with a three-step fallback, and a second line — `bbb1745`.
- [x] Five sort orders inside the groups, remembered — `cdcb1bb`.
- [x] Spine → row flight; the dropdown and its CSS deleted — `e75f3f4`.
- [x] Verified: rows and metas at every combination of cover/stats/date present and absent;
      A–Z collation (case-insensitive, "2 Track" before "10 Track"); recent ordering with
      unranked stations falling to the end in Pandora's order; the flight scrubbed frame by
      frame from the spine's text centre to the row's words, rotation −90°→0°, scale 1→1.28,
      cross-fade in the last 20%; cleanup clearing both inline styles. `bun run build` and
      `cargo test -p pandora` clean.

## Open questions for the user

None outstanding.

## Things not to do

- Don't make the list a grid, and don't put the per-row tags back. Both are settled.
- Don't count thumbs as listening.
- Don't run `cargo` from the repo root, or bare `cargo fmt` anywhere.
- Don't start a dev server on port 1420 from this worktree.
- Don't add `title=` attributes — `attachTip` in `main.ts` is how this app does hover
  detail, and it now treats an empty string as "nothing worth saying".
- Don't reintroduce a second station list. The spine is the active row; that is the idea.

## Still unverified against a live account

`artUrl` and `dateCreated` are what the probes and `plans/station-galaxy.md` say
`user.getStationList` carries, but nothing here has run against a real Pandora session.
Both degrade to nothing if that is wrong: rows fall back to the last played track's art and
then to the generated tile, and "Newest first" puts every station in the unknown bucket —
which is to say, in Pandora's order. First run on a real account, check a station that has
never been played through Jarlid: if its row shows a cover and an "Added …" line, both
fields arrived.
