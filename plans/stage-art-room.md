# More Room For The Album Art — Living Plan

Plan path: `plans/stage-art-room.md`

Status: **done and committed** (`491638a`, `edda67b`, `69cac22`, `c54eaef`). Nothing outstanding.

## Goal

The album art is height-bound: it gets whatever is left of `92vh` once everything
under it has its room, and on an ordinary window that leaves it small (183px at
1000×620). Reclaim vertical space from the stage so the art can grow, and fix a
lyric line that was being clipped.

Measured stack below the art at 1000×620, which is where every number in this plan
comes from:

| below the art | px |
| --- | --- |
| `.meta` top margin | 26 |
| station name row | 31 |
| title | 37 |
| artist | 32 |
| album | 24 |
| progress | 42 |
| transport | 100 |
| recently-played | 96 |
| **art gets what's left** | **183** |

## Environment / context

- `app/` — Vite + TS frontend for the Tauri v2 shell. `bun run build` = `tsc && vite build`;
  there is no lint config, the build is the gate.
- Verified in Chromium against `bun run dev -- --port 1430`, player state faked from
  the console. **Port 1420 is `strictPort` and shared with other worktrees — don't.**
- `--stage-reserve` in `#player` is the hand-maintained sum of that table. Anything
  added to or removed from the stage has to be put back into it in the same change.

## Decisions already made (don't re-ask)

Chosen by the user on 2026-09-17 from a measured four-way comparison:

- **The station name becomes a vertical spine to the left of the art** (+31px → art
  214). Their idea, and the right one: the station is the only thing in the stack
  that says *where you are* rather than *what is playing*.
- **Artist and album share one line**, `EMELINE · the devil on my bra strap`
  (+24px → art 238). It does *not* wrap — wrapping was the plan until it turned out
  to resize the art per track (see the findings); the album is clipped instead.
- **On a short, wide window the meta moves beside the art** instead of under it.
- **Not** the station chip overlaid on the artwork — it covers the thing we are
  trying to make bigger.
- Each fix is its own commit (user, mid-task).

## Findings / gotchas

- **The spine must not take layout width.** Anything that occupies a lane to the
  art's left either pushes the art off the centre line that `.progress` and
  `#history` sit on, or has to be mirrored by a phantom lane on the right, which
  costs the art ~68px of width on a tall window — the opposite of the point. It is
  absolutely positioned against `.art-wrap` instead, so the art's geometry and
  everything below it are untouched.
  Room to the art's left is `(column − art)/2 + 4vw` of page padding. Worst case is
  art == column (a tall window, no slack), which needs `4vw ≥ spine + gap ≈ 34px`,
  i.e. a viewport ≥ 850px. Two-column mode starts at 820px and the 820–850px band
  still has column slack, so there is no size where it collides.
- **The side-by-side layout only wins on a short *and* wide window, because of the
  `89vw − 420px` guard** that keeps the lyrics at a readable measure. Putting the
  meta beside the art widens the stage by `meta + gap` (324px), and that comes out
  of the lyrics — so it comes out of the art's own width budget. The break-even is
  `0.89w − 744 > 0.92h − 347`, i.e. **`w > 1.034h + 446`**: 1087px at h=620, 1563px
  at h=1080. Below that line the side-by-side layout makes the art *smaller*.
  Gated on `(min-width: 1200px) and (min-aspect-ratio: 7/4)`, which brackets that
  line from the safe side: past 623px tall the ratio already implies it, and below
  623 the 1200px floor does. Measured: 248 → 324px at 1600×620, 380 → 481 at
  1400×790, 672 → 748 at 1920×1080.
- **`.meta` needs `min-width: 0` before it will take a flex basis.** Its automatic
  minimum size is its min-content contribution, and `#title` is `white-space:
  nowrap` — so the meta demanded the full pixel length of the song title, could not
  fit beside the art, and wrapped onto a line of its own. The symptom is a flex
  item that simply refuses to sit next to its sibling however the numbers add up.
- **Letting the byline wrap would have made the art resize per track.** A long
  artist+album pair wraps to two lines on a 340px column, which puts 20px back
  into the stage and shrinks the art by that much — on those tracks only. So the
  album is clipped rather than wrapped, and `attachTip` grew an empty-string case
  so the tooltip only appears when something was actually clipped.
- **The reserve is about 20px optimistic, and always has been.** `1.15 ×` the title
  clamp and `1.3 ×` the artist clamp under-measure the real line boxes (29.4 vs 37,
  20.8 vs 32 at 1000×620). Nothing breaks: `.art-wrap` is a flex item and simply
  shrinks by the difference, which is why the art measures a few px under what the
  formula predicts. Left alone — it is pre-existing and self-correcting.
- **The playing lyric line was being clipped, not overflowing.** `#lyrics` has
  `overflow-y: auto`, and a non-`visible` overflow on one axis forces the other to
  `auto` — so `.line.active`'s `scale(1.02)` from the left edge lost its last ~11px
  (2% of the pane) off the right. Fixed by reserving exactly that 2% as right
  padding, off one `--line-grow` shared by the padding and the transform.

## Progress log

- [x] Lyric line clipping — `491638a`.
- [x] Station spine, `--stage-reserve` 327 → 296. Art 183 → 214 at 1000×620 — `edda67b`.
- [x] Artist · album on one line, `--stage-reserve` 296 → 272. Art 214 → 238 — `69cac22`.
- [x] Side-by-side meta on short+wide windows — `c54eaef`.
- [x] Verified at 700×900 (narrow), 860×1000 (the spine's tightest fit, 9px clear
      of the window edge), 1000×620, 1199×620 and 1200×620 (both sides of the
      threshold), 1400×790, 1600×620, 1920×1080. `bun run build` clean throughout.

## Things not to do

- Don't give the spine a layout lane — see the finding above; it was considered and
  it loses the art more width than the row it saves.
- Don't apply the side-by-side layout on merely-short windows; below
  `w > 1.034h + 446` it shrinks the art.
- Don't edit `--stage-reserve` and the stage's contents in different commits.
- Don't add a `title=` attribute to the spine — `attachTip` in `main.ts` is how this
  app does hover detail.
- Don't start a dev server on port 1420 from this worktree.
- Don't let the byline wrap; see the finding above.
- Don't lower the side-by-side threshold "because the window looks short enough".

## Noticed but not touched

In the narrow (<820px) layout the art is sized `min(34vh, 0.7 × --stage-w)`, and
`--stage-w` still carries the `89vw − 420px` term — a guard about keeping the
*lyrics column* readable, in a layout that has no lyrics column. At 700×900 that
term wins and the art comes out at 142px when the `34vh` that was presumably meant
would have given 306. Pre-existing (the pre-`f53b696` spelling, `70%` of a stage
that was itself `min(…, 89vw − 420px)`, behaved identically), so it was left alone
rather than folded into this pass.
