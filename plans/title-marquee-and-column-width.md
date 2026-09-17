# Stage Column Width + Title Marquee — Living Plan

Plan path: `plans/title-marquee-and-column-width.md`

Status: **done and committed**. Nothing outstanding.

## Goal

Two reports from a screenshot of the running app, both about the left (stage) column:

1. **The text under the art was clipped to the art's width**, not the column's. On a
   short window the art shrinks a long way — in the reported case to 169px inside a
   340px column — and the title, artist and album shrank with it, leaving ~85px of
   dead column on each side and a song title cut off after three words.
2. **The title marquee needed room and motion.** It scrubbed on hover, but reaching
   the very first or very last character took a pixel-perfect pointer, and the
   fixed right-edge fade dimmed the end of the title exactly when you finally got
   there. And it only ever moved under the pointer — on a touch screen, or just
   sitting there, a long title was permanently truncated.

## Environment / context

- `app/` — Vite + TypeScript frontend for the Tauri v2 shell. `bun run build` =
  `tsc && vite build`. There is no lint or prettier config in the repo; the build
  is the gate.
- Files changed: `app/src/styles.css`, `app/src/main.ts`.
- Verified in Chromium against `bun run dev -- --port 1430`, with the player state
  faked from the console (there is no Tauri bridge in a plain browser). **Port 1420
  is `strictPort` and shared with every other worktree — do not start a dev server
  on it from here.**

## Decisions already made (don't re-ask)

- **The stage is the column; only the art, the progress bar and the history strip
  are the art's width.** The alternative — teaching `.meta` the track width — means
  hard-coding the grid's 340px floor a second time in a place that can't see it.
- **Those three keep sharing one `--stage-w`**, per the earlier alignment pass: they
  used to repeat `min(42vh, 100%)` four times and drifted apart. One grouped rule,
  one property.
- **An overflowing title pins left (`text-align: left`), a short one stays centred.**
  Centring an overflowing inline-block shows its middle and hides *both* ends.
- **The fade follows the clipped side.** Fading the right edge unconditionally is
  what made the end of a long title unreadable. `--fade-l`/`--fade-r` are registered
  with `@property` so they can be transitioned and animated; unregistered custom
  properties are strings and jump between keyframes rather than interpolating.
- **The drift is a CSS animation, not a rAF loop.** Its endpoints and period come
  from JS as custom properties, so the walk is a constant px/s whatever the title's
  length, and the browser owns the ticking.
- **Constant speed, not constant duration** (`TITLE_SPEED = 36 px/s`), with a paused
  beat at each end of the travel (the paired keyframe stops) and a `Math.max(8, …)`
  floor on the cycle so a title that only just overflows doesn't twitch.

## Findings / gotchas

- **The narrow (<820px) layout reads `--stage-w` through a percentage too.** Its
  `.art-wrap { width: min(34vh, 70%) }` was 70% of a stage that *was* `--stage-w`;
  once the stage became the full column, that same 70% silently more than doubled
  the art. It is now written `min(34vh, calc(0.7 * var(--stage-w)))`, which is what
  it always meant. Percentages against the stage are a trap now that the stage is
  the column — spell out what the percentage is of.
- **The automation browser reports `prefers-reduced-motion: reduce`.** The drift is
  correctly disabled there, which looks exactly like the animation not working. To
  test the drift, inject a stylesheet re-declaring the two `animation` rules with
  `!important` rather than concluding the CSS is broken.
- **CSS animations beat inline styles in the cascade**, so a leftover inline
  `transform` doesn't fight the drift. The handover that *does* need care is the
  other direction: on `pointerenter` the drift's current position is read off
  `getComputedStyle().transform`, the class is dropped, and that value is written
  inline with the transition suppressed for one frame — otherwise the title snaps to
  its start before the first `mousemove` arrives.
- **Editing `main.ts` triggers a full page reload, not an HMR patch**, so any faked
  player state in the console is wiped. A `--title-cycle` of exactly `8.0s` (the
  floor) after an edit means the title is back to `—`, not that the measurement
  broke.
- Measured at 1000×620: stage/title 340px (was 183px), art 183px, progress 183px,
  travel 477px, cycle 35.9s. Scrub saturates at both ends by 18% of the box width
  (capped at 60px), so the first and last words are trivially reachable.

## Progress log

- [x] Read `plans/layout-alignment-pass.md` — the previous pass on this column, and
      the source of the `--stage-w` reserve sizing this builds on.
- [x] `.stage` → `width: 100%`; `.art-wrap`, `.progress`, `#history` → one grouped
      `min(100%, var(--stage-w))`; narrow-layout art rewritten against `--stage-w`.
- [x] Dynamic two-sided mask driven by `@property --fade-l/--fade-r`.
- [x] `measureTitle()` / `titleAt()` / `titleRelease()` in `main.ts`, replacing the
      inline `fits` toggle at both call sites (local and remote now-playing).
- [x] Idle drift keyframes + `prefers-reduced-motion` opt-out.
- [x] Re-measure on window resize (rAF-coalesced).
- [x] Verified in Chromium: layout at 1000×620 and 799×571 (narrow), drift sampled
      over a cycle, scrub saturation at both ends, fades crossing correctly.
      `bun run build` clean.

## Things not to do

- Don't reintroduce `title=` attributes or per-rule colour literals (standing rules).
- Don't write a percentage against `.stage` expecting it to mean the art's width —
  it means the column now. Use `var(--stage-w)`.
- Don't start a dev server on port 1420 from this worktree.
- Don't let `.progress`/`#history` drift off `--stage-w`; they belong to the record.
