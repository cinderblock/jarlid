# Player Layout + Settings Alignment — Living Plan

Plan path: `plans/layout-alignment-pass.md`

Status: **done and committed** (`da733f4`, `96be6d9`, `5b790d7`). Nothing outstanding.

## Goal

Two reports from driving the real app, both about space and alignment rather than
behaviour:

1. **Player.** The album art only reached its column's full width if the window was made
   roughly 20% taller than the content needs. Once it was that tall there was a band of
   dead space below the transport/history and some above the art.
2. **Settings.** Controls sat at a different vertical offset in every card, and the
   update-policy radio dots sat above the middle of the titles they belong to.

## Environment / context

- `app/` — Vite + TypeScript frontend for the Tauri v2 shell. `bun run build` = `tsc && vite build`.
- Files changed: `app/src/styles.css`, `app/index.html`.
- Dev server: `bun run dev` in `app/`. **Port 1420 is `strictPort` and is shared with every
  other worktree** — see the gotcha below. This work ran on `bun run dev -- --port 1430`.
- Verification was done in Chromium against the dev server, with page state faked from the
  console, because there is no Tauri bridge in a plain browser.

## Decisions already made (don't re-ask)

- **The art's column shrinks and grows with the art.** "Art fills its column" can be met by
  growing the art or by shrinking the column; the column was a fixed `44%`, so the art had
  to move and the column had to follow it, or the horizontal gap just replaces the vertical
  one. The leftover width goes to the lyrics.
- **Reserve-based sizing, not `42vh`.** Everything under the art is close to a fixed pixel
  height, so the art's budget is `available height − that reserve`, not a fraction of the
  viewport. A fraction was the whole reason a taller window was needed.
- **The reserve is a CSS constant, not a measurement.** A `ResizeObserver` would be exact
  but the reserve depends on the column width (album/artist wrapping) which depends on the
  reserve — a feedback loop and a real risk of Chrome's "ResizeObserver loop" errors. It
  predicted 402px against an actual 404px, which is well inside tolerance.
- **The art cap is `46vw`.** Chosen by the user on 2026-08-16 from a measured three-way
  comparison. It only bites on a near-square window; at ordinary aspect ratios the height
  budget binds first and every candidate cap gives an identical result.
- **Settings controls align to their *title*, not to the middle of the description.** That
  is what makes the offset consistent between cards, since descriptions vary from one to
  four lines.

## Findings / gotchas

- **Port 1420 is contended between worktrees, and the loser attaches to the winner instead
  of erroring.** A Tauri dev build (`cargo run --no-default-features`) loads its frontend
  from `localhost:1420`; with `strictPort: true` only one vite can hold it. Another
  worktree's app spent an afternoon rendering *this* worktree's uncommitted CSS. If you
  need a dev server here, use `bun run dev -- --port 1430` and leave 1420 alone.
- **A percentage inside a custom property is resolved where it is *used*, not where it is
  declared.** `--stage-w` is written in `vw`/`vh` only, because it is used both in
  `grid-template-columns` (percent of the player) and inside `.stage` (percent of the
  column). Writing `46%` there would silently mean two different things.
- **Flex cannot shrink an aspect-ratio box squarely when its width is a definite `100%`.**
  The algorithm shrinks the height and the ratio is ignored once both axes are definite, so
  the art's size is a `min()` on the width rather than a flex behaviour.
- **`.set-radio input { margin-top: 2px }` was dead code.** It ties with the later
  `input[type="radio"] { margin: 0 }` on specificity (0,1,1 each) and loses on order, so it
  computed to 0px and the dot rode high. This trap is specific to rules declared *before*
  the shared input reset — `.seg input` and `.lyric-check input` come after it and are fine.
- **Line-box centring is not optical centring.** The line box carries the font's descender
  space below the words, so centring a control in it still leaves it high. Measured for the
  0.9rem title: line box 20.88px, baseline 16.44px from its top, cap height 10px — so the
  cap centre is 11.44px down and line-box centring is ~1px too high.
- **A fixed-width read-out invites the browser to break inside the value, and `±` is a
  break opportunity where a digit is not.** `±6%` wrapped to five lines in a 3.4ch box and
  made its row 105px tall. Pre-existing, arrived with the blend work in `b6040e2`, and only
  visible with Beat-matched selected.
- The measured "before" offsets between a setting's title and its control: Theme +9px,
  Audio device +30px, Sign out +21px, Check schedule +10px. All are 0px now.

## Progress log

- [x] Read `plans/settings-ui-polish.md` (the previous pass on this page) and the CSS.
- [x] Measured the misalignments off the two screenshots at 2×/6×.
- [x] Rebased onto `origin/master` (v1.6.0); `styles.css` was untouched there, `index.html`
      had gained the "Between songs" section, which the alignment work then covered.
- [x] Player: `--stage-w` reserve sizing; the four `min(42vh, 100%)` sites collapsed onto
      `.stage`.
- [x] Measured the art cap three ways at 1057×1052 and had the user choose 46vw.
- [x] Settings: title promoted to a grid sibling in all nine `.set-item`s; named grid areas.
- [x] Radio dot on the cap-height centre.
- [x] Tempo-pull read-out `nowrap` + width for `±16%`.
- [x] Verified: 0px title-to-control offset across all six side-by-side settings at 780px,
      1057px and 1683px, in both themes; no collisions; narrow (<760px) still stacks
      title → description → control. `bun run build` clean.
- [x] Committed as three focused commits.

## Things not to do

- Don't reintroduce per-rule colour literals or `title=` attributes (both are standing rules
  for this page).
- Don't let `.progress`/`#history` drift away from the art's width — that is exactly what
  four copies of `min(42vh, 100%)` invited.
- Don't start a dev server on 1420 from this worktree.
- Don't add a `ResizeObserver` to measure `--stage-reserve`; the feedback loop is real.
