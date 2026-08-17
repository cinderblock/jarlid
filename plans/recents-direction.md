# Recently-played direction — Living Plan

Plan path: `plans/recents-direction.md`

## Goal

Let the recently-played strip run either way, and make **newest on the right** the default,
which is what the Pandora app does. Today the newest is always leftmost.

## Environment / context

- Branch `t3code/recents-direction`, cut from `33ee7e7` (v1.6.1, == `origin/master`).
- Files in play: `app/src-tauri/src/settings.rs`, `app/src/settings-page.ts`,
  `app/src/main.ts`, `app/index.html`, new `app/src/recents.ts`.
- Dev server: **do not use port 1420** — it is `strictPort` and shared with other worktrees,
  where the loser silently attaches to the winner's server. Use
  `bun run dev -- --port 1430`.

## How it works today

- `history[0]` is the most recent; `pushHistory()` does `history.unshift(...)` and caps at 40.
- `renderHistory()` iterates the array in order and appends, so newest ends up leftmost.
- The strip is `display: flex; overflow-x: auto`, so **`scrollLeft: 0` happens to reveal the
  newest**. That coincidence is load-bearing and breaks the moment the order flips.
- `coverflow()` tilts items by distance from the strip's centre — symmetric, so it needs no
  change either way.
- The history array itself lives in `localStorage`, *not* in settings.json.

## Decisions already made (don't re-ask)

- **The preference lives in settings.json**, like every other setting — the precedent set by
  the theme in `plans/settings-ui-polish.md`. `localStorage` is a paint cache only, so the
  first frame is not drawn the wrong way round before Rust answers.
- **Store the order, not the array.** The stored history stays newest-first; only the
  rendering direction changes. Rewriting the array would make the setting destructive and
  would corrupt the list if it were ever applied twice.
- **An enum, not a bool.** `newestRight` / `newestLeft` reads unambiguously in settings.json
  and on the wire; `reversed: true` does not say reversed from what. Matches `Theme` and
  `BlendMode`.
- **Default is `NewestRight`**, per the request. This flips the strip for existing installs,
  which is intended.
- **It goes in the Appearance section** — it is a layout preference, and that section
  currently holds only Theme.

## Plan / steps

1. [x] `settings.rs`: `RecentsOrder` enum (default `NewestRight`), field on `Settings`, and a
   test that a settings file written before this existed still loads as `NewestRight`.
2. [x] `recents.ts`: the preference, its `localStorage` cache, and a change event.
3. [x] `main.ts`: `renderHistory()` honours the order and parks the scroll at the end that
   shows the newest.
4. [x] `index.html` + `settings-page.ts`: a segmented control in Appearance.
5. [x] Verify: both directions, that the newest is the one actually on screen after a new
   song lands, `bun run build`, `cargo test`.
6. [x] Commit.

## Findings / gotchas

- `scrollLeft: 0` revealing the newest is a coincidence of the *old* order. With newest on
  the right the strip must be parked at `scrollWidth`.
- **The park has to happen when the player is revealed, not when the strip is rendered.**
  `renderHistory()` runs at module load, while `#player` is still `hidden`; scrolling a
  zero-width element does nothing and reports no error, so the first park was silently lost
  and the strip sat at the *oldest* end. The next render only comes with a *new* song — and
  `pushHistory()` returns early when playback resumes on the track already at the top of the
  list, so it could stay wrong for the whole session. `showPlayer()` now does both.
- **A `ResizeObserver` is not a usable trigger for this.** Observing the strip and waiting
  for it to gain a box when its `display: none` ancestor is revealed delivered **no callback
  at all** — not even the initial one. Measured, not assumed; the observer's log was empty
  400ms after the reveal, while setting `scrollLeft` by hand at the same moment stuck fine.
- Verified end-to-end by driving the real segmented control with the strip on screen:
  `newestLeft` → DOM `0…11`, `scrollLeft 0`; `newestRight` → DOM `11…0`, `scrollLeft 197`
  (the maximum). The newest cover is on screen in both.
- The new row needs no layout work of its own — it inherits the `.set-item` grid, and its
  title-to-control offset measured 0px. The two-segment control is 250px in a 280px column.

## Progress log

- [x] Read the history rendering, the coverflow, and the settings plumbing.
- [x] Rust enum + field + two tests; full lib suite 87 passed.
- [x] `recents.ts`, render reversal, `showPlayer()`.
- [x] Settings control wired, including the rollback path and `setEnabled`.
- [x] `bun run build` clean; `rustfmt --check src/settings.rs` clean.

## Things not to do

- Don't reverse the stored `history` array — reverse only at render.
- Don't reach for `flex-direction: row-reverse`: it moves the scroll origin as well, which
  is exactly the thing that has to stay predictable here.
- No `title=` attributes; use `attachTip`.
