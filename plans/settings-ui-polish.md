# Settings UI Polish — Living Plan

Plan path: `plans/settings-ui-polish.md`

## Goal

The Settings page (added by the seamless-updates work) uses raw native controls: an OS
`<select>` for the check schedule, native radio buttons for the update policy, native
checkboxes on the Stations page. In WebView2 these render with the platform's own light
chrome and look pasted onto an otherwise hand-drawn dark UI.

Make every control in the app look like it belongs to the app, and make the parts that
*can't* be drawn by us (caret, time-input spinners, scrollbars, autofill) render dark
instead of light.

## Decisions already made (don't re-ask)

- **A real light theme, plus System/Light/Dark as a setting.** Asked on 2026-08-11 and
  first answered "dark-only"; the user reversed it mid-task ("also add a setting that lets
  users control system/dark/light theme"). Both themes now exist and `styles.css` is fully
  tokenized. Do not re-derive the dark-only decision from the earlier part of the thread.
- **`--wash` is how the light theme was affordable.** Nearly every surface in this app is a
  translucent overlay on the blurred album art, so one token holding the overlay's *ink*
  (`255,255,255` vs `18,22,32`) converts the whole UI while keeping each surface's original
  alpha. Shadows are the exception — black at 0.55 is dirt on a light surface — so they get
  three tokens of their own (`--sh-soft` / `--sh` / `--sh-deep`).
- **`data-theme` on `<html>` is always the *resolved* theme.** "System" is resolved in
  `theme.ts`, never in CSS, so no rule is written twice under a media query.
- **The theme lives in `settings.json` like every other setting**, with `localStorage` as a
  paint cache only (an inline script in `<head>` reads it before the first frame; Rust's
  answer arrives a moment later and corrects it if they disagree).
- **`color-scheme: dark` on `:root`** is the one-line fix for everything we cannot style:
  the `<select>` popup, `input[type=time]` spinners, scrollbars, focus rings, autofill.
- **Replace the schedule `<select>` with our own listbox**, rather than just restyling the
  closed control. The popup list of a native select cannot be styled at all — only its
  colour scheme — and the app already has this idiom (`#mode-panel`, `#station-panel`).
- **Radios and checkboxes are drawn from scratch** (`appearance: none` + `::after`), not
  merely `accent-color`d. `accent-color` tints the dot but leaves the OS ring and sizing.
- **`:has()` is fair game** — WebView2 is evergreen Chromium and `styles.css` already
  uses it (`.sp-row:has(.sp-why)`).
- **The custom select lives in its own module** (`app/src/select.ts`) so the next dropdown
  doesn't reinvent it.

## Environment / context

- `app/` — Vite + TypeScript frontend for the Tauri v2 shell. `bun run build` = `tsc && vite build`.
- Relevant files: `app/index.html` (settings markup), `app/src/styles.css`,
  `app/src/settings-page.ts`, `app/src/stations-page.ts` (checkboxes), new `app/src/select.ts`.
- No native title tooltips anywhere in this app — `attachTip` in `main.ts` is the tooltip.

## Plan / steps

1. [x] `:root`: add `color-scheme: dark` and shared tokens (`--panel`, `--line`, `--field`,
   `--hover`, `--ink`, `--danger`).
2. [x] Shared control styles: custom radio, custom checkbox, custom select, field/`.btn`
   styling, global `:focus-visible` ring.
3. [x] `select.ts` — accessible listbox (combobox pattern: arrows, Home/End, Enter/Space,
   Escape, type-ahead, click-outside, `aria-selected`/`aria-expanded`).
4. [x] Rework the Settings markup: sections as cards, policy options as selectable cards,
   schedule row using the custom select.
5. [x] `settings-page.ts` — drive the custom select; show an unrecognised stored interval
   truthfully ("Every 3 hours") instead of silently snapping to 30 minutes.
6. [x] Light theme: tokenize `styles.css`, add the `:root[data-theme="light"]` block.
7. [x] `theme.ts` + `Theme` in `settings.rs` + the Appearance segmented control.
8. [x] Verify: `bun run build`, `cargo test`, and drive it in Chromium.

## Findings / gotchas

- `settings-page.ts` closes the page on `Escape` via a `window` keydown listener. The
  dropdown must `stopPropagation()` on its own Escape or closing the list would also close
  Settings. The select's handler is attached to its root, which is on the bubble path
  *before* `window`, so this works.
- The old `<select>` fallback silently rewrote an unknown interval to 30 minutes on the
  next save (`render()` in `settings-page.ts`). A custom list can just show the real value.
- `#sp-row input` has `pointer-events: none` (the whole row is the click target) — custom
  checkbox styling must not depend on `:hover` of the input itself.
- **`appearance: none` silently kills the indeterminate dash.** The Stations page sets
  `allBox.indeterminate` for a partial selection; once the box is drawn by us, "some" looked
  exactly like "none" until `:indeterminate::after` was written. Caught by driving it, not by
  any check. Anything else that relies on a UA-drawn state needs the same treatment.
- **Disabling a control takes focus with it.** `persist()` disables everything while it
  saves, which dropped a keyboard user back to the top of the page after every change;
  `persist()` now restores `document.activeElement` afterwards.
- `setTheme` needs `core:window:allow-set-theme` in `capabilities/default.json` —
  `core:default` does not include it, and without it only the page themes, not the frame.
- Verification was done against the vite dev server in Chromium (same engine as WebView2),
  with the page's state faked in the console because there is no Tauri bridge in a browser.
  That covers the CSS and `select.ts` completely. It does NOT cover `setTheme` on the window
  frame, or the settings round trip — those need the real app.
- The Chrome screenshot tool returns a *scaled full-page* capture, so click coordinates
  taken off a screenshot do not land where you expect; drive elements from the console and
  use the `key` action for keyboard tests.

## Progress log

- [x] Read the existing settings page, styles, and the mode-panel idiom it should match.
- [x] Tokens + `color-scheme`; both theme blocks.
- [x] Custom radio / checkbox / select / field / segmented-control styles.
- [x] `select.ts` (keyboard, type-ahead, click-outside) and its wiring.
- [x] `theme.ts`, the no-flash boot script, `Theme` in `settings.rs`, the capability.
- [x] `bun run build` clean; `cargo test --lib settings` 10/10.
- [x] Driven in Chromium: both themes, the player over the art wash, the Stations list,
      the dropdown by mouse and by keyboard. Two bugs found and fixed that way (see
      Findings).

## Open questions for the user

1. Should the **light theme's ambient wash be stronger**? It is currently
   `brightness(1.3)` behind a 0.82 scrim, which reads as a pale tint of the cover art.
   Dark mode's wash is much more present. My recommendation: live with it for a few albums
   before changing it — it is one token (`--art-filter`) either way.

## Things not to do

- Don't add `title=` attributes for the option descriptions — they are inline by design.
- Don't restyle the player transport (`button.icon`, `#bar`, history coverflow); this task
  is scoped to form controls, theming and the Settings/Stations chrome.
- Don't reintroduce per-rule colour literals. Anything new goes through the tokens, or the
  light theme silently rots.
