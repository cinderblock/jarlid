# The Stations Panel — Living Plan

Plan path: `plans/stations-modal.md`

Status: **one item open.** `25223c1` was a misreading and has been undone; the spine is
vertical again and the original complaint — it is hidden by the album art on the user's
machine — is unreproduced and waiting on a screenshot. Settings became an 80% panel.

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

- **"Bookend" meant the station name — and the user wants it vertical.** Nothing in the
  app is called that; asked which element they meant, they picked the spine. The option's
  description had baked in a remedy ("put it back on its own row"), and picking the
  element got read as picking the fix. It was not: on 2026-09-18 the user said they never
  asked for the horizontal row. `25223c1` is reverted by `fd656a4`; the spine is
  back as in `edda67b`. **Don't offer a remedy inside an identification question.**
- **The flight's quarter-turn is conditional** on the source running vertically, so it
  survives the spine coming and going without further edits.
- **The scrim is dimmed, not blurred.** The whole point of the 10% margin is watching the
  player keep playing; a blur destroys exactly the motion it is meant to reveal.
- **Settings is a panel too, at 80%** (user, 2026-09-18). Same scrim, same reveal, same
  close; one custom property sets the size per page.
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

- [x] ~~Station name back on its own row~~ — `25223c1`, **a misreading, reverted by
      `fd656a4`**. The spine is vertical again and `--stage-reserve` is 272 again; the
      flight's rotation stays conditional.
- [x] Settings as an 80% panel sharing the Stations scrim, reveal and close — `26f6866`.
- [ ] The spine hidden by the art on the user's machine: unreproduced, waiting on a
      screenshot (see the open question).
- [x] 90% panel over a dimmed, unblurred scrim; `.page-panel` split from the scrim —
      `4d350ae`.
- [x] clip-path open/close, reduced-motion opt-out, close on a timer — `adef618`.
- [x] Container-query table past 900px: Plays / Last played / Added — `0134e25`.
- [x] Verified: panel exactly 90%×90% at 1100×820 and 1500×900; scrim `rgba(8,8,10,0.55)`;
      reveal scrubbed at 0/80/160/240/320ms with the rows immobile; flight measured from
      the player's words (285,525) to the row's words (222,358) with rotation 0 throughout;
      table columns aligned to the pixel against the heading row; an 855px panel falls back
      to the stacked layout with its second line. `bun run build` clean at every step.

## Open questions for the user

1. **How is the spine hidden?** At every size reproducible here (1500×900, 1920×1080
   side-by-side, 860×1000, 700×900) it does not overlap the art, is on screen, and is the
   top element at its own text. The likeliest explanation is not geometry but legibility:
   the spine is `--faint` text sitting inside the art's `0 30px 80px` drop shadow, and on
   a big, bright cover it may simply vanish. A screenshot from the real app would settle
   it in one look.

## Things not to do

- Don't animate the panel's entry with anything that moves its contents — see the finding.
- Don't blur the scrim.
- Don't put the station name back on a horizontal row. The user wants the spine.
- Don't switch the table's breakpoint to a media query.
- Don't start a dev server on port 1420 from this worktree.

## Testing this in a browser

The three traps from `plans/station-picker-rich.md` all still apply — the automation
browser reports `prefers-reduced-motion: reduce` (so both the reveal and the flight
correctly do nothing), a backgrounded tab barely renders (so scrub animations via
`getAnimations()` and `currentTime` rather than sampling by wall-clock), and
`import("/src/stations-page.ts")` from the console is a different module instance from the
one `main.ts` holds unless you read the HMR-timestamped specifier out of the served source.
