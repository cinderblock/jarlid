# The Stations Panel — Living Plan

Plan path: `plans/stations-modal.md`

Status: **done and committed** (`25223c1`, `4d350ae`, `adef618`, `0134e25`).
Nothing outstanding.

Follows `plans/station-picker-rich.md`, which turned the station dropdown into this page.

## Goal

Four things, from the user on 2026-09-17:

1. Put the station name back where it was — the vertical spine was being hidden by the
   album art.
2. Make the Stations view a ~90% modal, so the player is still visible and visibly
   *playing* behind it.
3. Animate the opening.
4. On wide screens, lay the list out as a table.

## Environment / context

- `app/` — Vite + TS. `bun run build` = `tsc && vite build`; no lint config, the build is
  the gate. Dev server: `bun run dev -- --port 1430` (**never 1420 — shared**).
- `--stage-reserve` in `#player` is a hand-maintained sum of everything under the album
  art. Anything added to or removed from the stage goes into it in the same commit.

## Decisions already made (don't re-ask)

- **"Bookend" meant the station name.** Nothing in the app is called that; the user
  confirmed which element on 2026-09-17. It is back on its own row above the song title,
  and the album art is 31px shorter for it — a cost stated when the choice was offered.
- **The quarter-turn is gone with it.** The rotation existed because one end of the flight
  ran vertically. It is now conditional on the name actually being vertical rather than
  rotating a horizontal name to horizontal.
- **The scrim is dimmed, not blurred.** The whole point of the 10% margin is watching the
  player keep playing; a blur destroys exactly the motion it is meant to reveal.
- **The Settings page still fills the window.** It is somewhere you go, not something you
  consult while the music plays. Only the Stations view became a panel.
- **A table is not the grid the user vetoed.** `plans/station-galaxy.md` records "don't
  make my eyes scan in two dimensions at once", which is about finding a known name among
  cards. One station per row with its facts aligned beside it is one dimension.
- Each of the four asks is its own commit (standing instruction from the user).

## Findings / gotchas

- **The panel is revealed with `clip-path`, and that is load-bearing, not stylistic.** The
  station name flies from the player to its row and measures that row the moment the panel
  opens. Any entry animation that lays the rows out again — a scale, a slide — lands the
  name where the row no longer is. `clip-path: inset(9% 12%)` → `inset(0)` reveals without
  moving content: verified frame by frame, the first row's name is at the same coordinates
  at 0ms and at 320ms.
- **Closing runs off a timer, not `animationend`.** That event is dispatched on a frame,
  and an off-screen window barely gets any — the panel would sit half-closed until you
  came back. Same reason the flight's cleanup has a timeout beside its `finish` listener.
- **`em` column widths do not line a table up.** `em` is the element's own font size, so
  the 0.72rem heading row and the 0.92rem data rows computed different tracks from the same
  declaration. One `rem` track list in a custom property on `.sp-col` fixes it.
- **The rows carry a 2px transparent left border** for the active marker, so the heading
  row needs the same one or every heading sits two pixels left of its column.
- **The breakpoint has to be a container query.** The panel is 90% of the window; a media
  query at 1000px fires when the panel is 900px, which is a whole column out.
- **`.sp-row` is shared with the import preview**, which appends rows straight to `#sp-list`
  rather than into `.sp-col`. Scoping the table rules to `.sp-col .sp-row` is what keeps
  the preview's quite different children out of the grid.
- **Splitting four interleaved asks into four commits**: the blocks were carved out to a
  temp dir, committed in order, and restored one at a time, with `bun run build` between
  each. The final build hash matched the verified state, which is what proves the split did
  not change the result.

## Progress log

- [x] Station name back on its own row; `--stage-reserve` 272 → 303; flight's rotation made
      conditional — `25223c1`.
- [x] 90% panel over a dimmed, unblurred scrim; `.page-panel` split from the scrim —
      `4d350ae`.
- [x] clip-path open/close, reduced-motion opt-out, close on a timer — `adef618`.
- [x] Container-query table past 900px: Plays / Last played / Added — `0134e25`.
- [x] Verified: panel exactly 90%×90% at 1100×820 and 1500×900; scrim `rgba(8,8,10,0.55)`;
      reveal scrubbed at 0/80/160/240/320ms with the rows immobile; flight measured from
      the player's words (285,525) to the row's words (222,358) with rotation 0 throughout;
      table columns aligned to the pixel against the heading row; an 855px panel falls back
      to the stacked layout with its second line. `bun run build` clean at every step.

## Things not to do

- Don't animate the panel's entry with anything that moves its contents — see the finding.
- Don't blur the scrim.
- Don't reintroduce the vertical spine; it was tried and put back.
- Don't switch the table's breakpoint to a media query.
- Don't start a dev server on port 1420 from this worktree.

## Testing this in a browser

The three traps from `plans/station-picker-rich.md` all still apply — the automation
browser reports `prefers-reduced-motion: reduce` (so both the reveal and the flight
correctly do nothing), a backgrounded tab barely renders (so scrub animations via
`getAnimations()` and `currentTime` rather than sampling by wall-clock), and
`import("/src/stations-page.ts")` from the console is a different module instance from the
one `main.ts` holds unless you read the HMR-timestamped specifier out of the served source.
