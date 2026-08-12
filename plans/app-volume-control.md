# App Volume Control, Output Device & Settings Layout — Living Plan

Plan path: `plans/app-volume-control.md`

## Goal

Three things, in one working session:

1. **Jarlid's own output volume**, adjustable from the Settings page, so the music can sit
   *below* everything else on the machine. The motivating case, in the user's words: turn the
   main Windows output up so other apps' sounds are louder than the music, without the music
   becoming painful.
2. **Output device control** — pick an endpoint, or follow the Windows default *and actually
   keep following it*. Reported by the user as "we're seemingly not sending audio to the
   default audio device"; confirmed to be a device switch while the app was running, which a
   restart cleared.
3. **A responsive Settings page** — when the window is wide, use the right half: descriptions
   on the left, controls in a narrower right-hand column. The volume fader is the deliberate
   exception and keeps the full width, because it is judged by movements of a percent or two.

Plus a **temporary harness** for choosing the volume taper by ear (see DEV-CURVE below), which
is explicitly *not* to be committed as-is.

## Environment / context

- Repo: `C:\Users\camer\git\Personal Projects\Pandora` (the app is **Jarlid**).
- `crates/` is its own cargo workspace (`audio`, `engine`, `pandora`) — **there is no
  `Cargo.toml` at the repo root**; `cargo` run from the repo root fails. The app crate is a
  separate manifest again at `app/src-tauri/`.
- `app/` — Vite + vanilla TypeScript. `bun run build` = `tsc && vite build`. Dev server is
  port 1420, `strictPort`.
- Version: NOT touched by this work. See the version finding below.

## Decisions already made (don't re-ask)

- **It goes on the Settings page**, per the user's request — not in the player transport.
- **Attenuation only (0–100 %, 100 % = unity).** The point is to sit below the rest of the
  system; gain above unity would clip a stream whose headroom we don't control.
- **The taper lives in Rust, once** (`settings::Volume::amplitude`), and `native_volume`
  takes the *percentage* rather than a gain. Putting the curve in TypeScript would mean two
  copies of a load-bearing formula, free to drift.
- **`settings.json` stores the percentage the user chose**, not the computed gain, so the
  curve can be corrected later without silently reinterpreting everyone's saved setting.
- **Applying and persisting stay separate, matching the theme precedent.** `set_settings`
  only writes; the UI applies. Live on `input` (audible immediately, no disk), persist on
  `change` (fires on release). This is why dragging doesn't write the file per pixel.
- **This control governs *local* output only.** Remote/WiiM mode has its own slider that
  commands the renderer; the two are unrelated and must not be wired together.
- **Don't commit until the concurrent "patch feature" thread has landed** (user's
  instruction). Shared working tree: stage only our own hunks, never `checkout --` /
  `restore` / `reset --hard`. As of 2026-08-12 the user says other threads have settled, but
  the curve harness still has to come out before anything is committed.
- **Following the Windows default is the default, and is real.** `None` in settings means
  follow, and the engine polls the default endpoint's name once a second and moves playback
  when it changes. Pinning to a named device is the opt-out, not the other way round.
- **A chosen device that is absent falls back to the default without rewriting the setting.**
  Unplugging a DAC costs you the choice for as long as it is gone, not permanently. The
  Settings page says so out loud rather than silently showing the wrong thing.
- **The device list is not a constant.** It is enumerated from the backend each time the page
  opens; a stored device that is missing is still shown as an option so the UI never disagrees
  with the file.
- **Layout: description left, control right, at ≥760px.** Volume is the stated exception and
  spans the full card. The update-policy cards go to two columns at the same breakpoint.
- **DEV-CURVE is scaffolding, not a feature.** The user wants to hear several tapers and
  commit only the winner. Everything belonging to it is tagged `DEV-CURVE` so it can be found
  and removed in one pass.

## Findings / gotchas

- **The entire backend volume path already existed** and was dead code. `Player::set_volume`
  → `AudioThread::Command::SetVolume` → `Engine::set_volume` → the `native_volume` Tauri
  command, registered in `lib.rs` — with **zero frontend callers**. The work was persistence,
  a UI control, and fixing how the gain was applied; not new plumbing.
- **The gain was applied in 16-bit fixed point** (`sample as i32 * volume / 1024`, clamped to
  `i16`) *before* the sample was converted to the device format. On the usual Windows path —
  WASAPI shared mode, which is `f32` — that threw away roughly a bit per halving, so a
  comfortable listening level would have played back as 11–12-bit audio. Gain now happens in
  `f32` before quantisation, so attenuating costs no resolution at all on that path.
- **A volume change is a step, and a step in amplitude is a click.** The callback read the
  volume once per buffer, so any change landed as a discontinuity at a buffer boundary. It
  now ramps with a one-pole filter over ~15 ms, derived from the device's own sample rate.
  The filter snaps to the target once the remainder is below a 16-bit LSB, so "muted" is
  really zero rather than an asymptote grinding on denormals forever.
- **`u8` + `#[derive(Default)]` would have defaulted the volume to 0 — silence.** Any
  settings file written before this feature existed has no `volume` key, so every existing
  install would have updated itself into silence. `Volume` is a newtype with a hand-written
  `Default` of 100, and there is a test asserting exactly that.
- **The filled part of a range track can't be styled in Chromium.** There is no equivalent of
  `::-moz-range-progress`, so the fill is a hard-stopped gradient on
  `::-webkit-slider-runnable-track` with the stop position passed in as `--fill` from script.
  Custom properties inherit into pseudo-elements, which is the only reason it works.
- **Bug caught by driving it, not by any check:** because `--fill` only exists once script
  sets it, the first paint drew the markup's `value="100"` as a *completely empty* track with
  the thumb at the far right. `render()` would have fixed it, but not on the path where
  `get_settings` fails — exactly when a control lying about its value matters most. Fixed by
  calling `reflectVolume()` once at module load.
- **No version change belongs to this work. Don't reintroduce one.** A 1.4.0 bump was made
  here and then backed out, having actively disrupted the concurrent thread. What happened:
  `8200fab` mixed a bump into an icon commit, leaving `package.json`/`tauri.conf.json` at
  1.4.0 and the Cargo files at 1.3.3. This task read that split as "the repo is mid-bump,
  finish it" and wrote 1.4.0 into the Cargo files — but the cast icon was a *patch*, so the
  other thread wanted 1.3.4 and had to walk the version back while an uncommitted 1.4.0 sat
  in the shared tree. All four sites read **1.3.4** now, and the Cargo files are clean in
  `git status`. The rule is written up in the new repo `CLAUDE.md`: a bump is its own
  version-only commit, and is never an agent's own initiative.
- **The default-device bug, diagnosed.** `Player::play_at` resolved the endpoint exactly once,
  at stream-open time (`cpal::default_host().default_output_device()`). cpal binds that
  endpoint for the life of the stream and does no stream routing, so when Windows' default
  moves and the *old* device is still present, nothing errors, the callback keeps running, and
  every watchdog branch reports perfect health — while the music plays to the speakers you just
  switched away from. Restarting the app re-resolves, which is exactly what the user observed.
  Polling the default endpoint's *name* is the only signal available short of implementing
  `IMMNotificationClient`, and at a 1 s cadence it costs one COM call per second.
- **A device change must not spend the track's recovery budget.** The rebuild path is shared
  with stall recovery, which gives up after `MAX_RECOVERIES` (3) and skips the track. Switching
  outputs four times in an evening would have retired the song. The new branch sits *after*
  `reason` (so a genuinely broken device still wins) and resets `recoveries`, matching how
  `Seek` already reasons about this.
- **"Which device is selected" and "which device is in use" are different questions**, and the
  UI needs both: "Windows default" doesn't say *which* device that is, and a chosen device that
  is unplugged is not the one making sound. Hence `Player::device_name()` published up through
  `AudioThread` → `Engine::output_device()` → `native_output_device`.
- **The dB read-out is computed in Rust, not TypeScript.** It only feeds a label, so the
  temptation to do the arithmetic in the frontend is real — but a read-out that disagreed with
  what is audible would invalidate the entire curve comparison, which is the one failure mode
  that matters here. `volume_gain` is engine-free so it also works before sign-in.
- **`rustfmt` follows `mod` declarations.** Checking `app/src-tauri/src/lib.rs` reformats
  `diagnostics.rs`; checking `crates/audio/src/lib.rs` reformats `media_foundation.rs`. Both
  are pre-existing, untouched code. Check *individual files* and only fix hunks that are yours
  — this is the trap `CLAUDE.md` already warns about, and it fires immediately.
- **Three CSS rules died with the layout rework** — `.set-note`, `.set-row`, `.set-row-inline`
  had no remaining markup. Removed rather than left to rot.
- Related: re-read `git log` before assuming a baseline. HEAD moved three commits mid-task.
- The 14 console exceptions on the vite dev server (`transformCallback` / `listen`) are
  pre-existing and expected — there is no Tauri bridge in a plain browser. None come from the
  settings page.

## Progress log

- [x] Mapped the audio path and the settings UI (two exploration passes).
- [x] `crates/audio`: `Shared.volume` → `AtomicU32` of `f32` bits; gain applied in `f32` with
      a per-sample ramp; conversions take `f32`; added `Player::volume()`.
- [x] `settings.rs`: `Volume` newtype (default 100, square-law `amplitude()`), added to
      `Settings`, wire-format test updated, 3 new tests.
- [x] `native.rs`: apply the saved level in `attach()`; `native_volume` now takes a percent.
- [x] `index.html` + `styles.css`: a `.slider` component (track fill, thumb, focus ring,
      disabled, Firefox pseudo-elements) and the Volume section.
- [x] `settings-page.ts`: read/render/persist, live-apply on `input`, rollback re-applies.
- [x] README: new bullet + the Settings bullet.
- [x] Version left alone at 1.3.4 (a bump was made, then reverted — see Findings).
- [x] Wrote the repo's first `CLAUDE.md` with the version-bump and shared-tree rules.
- [x] `bun run build` clean; `cargo test --lib` 55/55; `crates` workspace builds clean.
- [x] Driven in Chromium: both themes, initial paint, drag, keyboard step. One bug found and
      fixed that way (the `--fill` first paint).
### Output device (2026-08-12)

- [x] `crates/audio`: `Output` enum, `output_devices()`, `default_output_name()`,
      `resolve_device()` with fallback, `Player::play_on`, `Player::device_name()`.
- [x] `crates/engine`: `Command::SetOutput`, `output` state that survives rebuilds, the
      1 s default-device poll and its non-fault rebuild branch, `Published.device`,
      `AudioThread::set_output`/`output_device`, `Engine` methods, `Output` re-export.
- [x] `settings.rs`: `output_device: Option<String>` + a test that an absent device is kept.
- [x] `native.rs`: `native_output_devices`, `native_set_output`, `native_output_device`,
      and `attach()` applying the saved endpoint.
- [x] UI: an Output section with the device select and a live "playing on …" line.

### Settings layout (2026-08-12)

- [x] `.set-item` / `.set-item-text` / `.set-item-ctl` grid; single column narrow, two
      columns from 760px; `.set-item-wide` for the fader; `.set-policy-grid` two-up.
- [x] `.set-section` widened from 560px to 1040px and centred.
- [x] All five sections re-marked-up; dead rules removed.

### DEV-CURVE harness (2026-08-12) — built, used, and removed

- [x] `Curve` enum (linear, x^1.5, x², x³, 40 dB, 60 dB), `Volume::amplitude_with`,
      `volume_gain` command, `volume_curve` setting, a picker and a live gain/dB read-out.
- [x] Auditioned by the user. Verdict: "the x^3 or 60dB tapers are best. dB is probably the
      right thing to use" → **60 dB constant-dB fader chosen.**
- [x] Stripped entirely. `grep -rn DEV-CURVE` over sources is clean (only `app/dist/`, which
      is gitignored build output). `Volume::amplitude` is now the 60 dB fader and is the only
      path again.

### Verification

- [x] `cargo check` on both workspaces; `cargo test --all` in `crates` (4 audio + 21 pandora);
      `cargo test --lib settings` 15/15; `bun run build` clean.
- [x] `rustfmt --check` on each touched file individually — only my own hunks fixed.
- [x] After stripping: `cargo test --lib` 63 passed / 0 failed, `bun run build` clean,
      `rustfmt --check` clean on every file I touched.
- [x] Settings layout driven in Chromium at both widths. The narrow pass was done by raising
      the real breakpoint at runtime (`rule.media.mediaText`) rather than faking narrow rules,
      because Chrome would not shrink the viewport below ~862 px however small the *window*
      was set — `innerWidth` and `outerWidth` disagree, and the media query follows the former.
- [x] **Settings round trip verified in the real app**, from `settings.json` on disk
      (`%APPDATA%\com.camer.pandora-desktop\settings.json`): the user's chosen `"volume": 46`
      reached the file, and `"outputDevice": null` is written for "follow the default". The
      same file had been written by the *harness* build with a `volumeCurve` key; the stripped
      build read it, ignored the unknown field and rewrote it clean — so removing the harness
      needed no migration, which was worth confirming rather than assuming.
- [ ] **Still not verified**: the ~15 ms gain ramp and the `f32` path by ear, and the
      device-follow rebuild (needs an actual default-device switch while playing). Both need
      a person; neither is claimed.
- [x] Committed.

## Gotcha that cost the user a working app

**Editing `index.html` and `settings-page.ts` in separate turns breaks the running dev app.**
Removing the curve picker's markup first left `settings-page.ts` calling
`createSelect($("set-volume-curve"), …)` on an element that no longer existed. `$` casts a
`null` from `getElementById` without complaint, so `createSelect` threw at *module scope* —
which kills the entire UI bundle. The webview went blank while Rust carried on, so audio and
the media keys still worked and it looked like an unrelated fault. Vite's HMR does not
recover from a module-level throw; the fix is a reload once the sources agree again.

When removing a control, change the TS and the HTML in the same beat, or expect a blank
window in between.

## Open questions for the user

1. ~~Which taper?~~ **Answered: the 60 dB constant-dB fader.** The deciding argument was that
   it gives a uniform 0.6 dB per 1 % step — every position on the slider is an equal,
   audible change — where a power law is 0.26 dB per step at the top (dead under the hand)
   and nearly 3 dB at the bottom (twitchy). Cost accepted: below ~15 % is past −50 dB and so
   effectively dead travel.
2. The remote/WiiM slider (`#remote-vol`) is still the app's one platform-drawn control, with
   its own minimal styling. It could now adopt `.slider` and lose the duplicate. Deliberately
   left alone as out of scope; worth doing separately.
3. `CLAUDE.md` at the repo root is untracked and was written by this task. Asked whether to
   delete it; no answer yet, so it has been left exactly as it is.

## Things not to do

- Don't add `title=` attributes — this app has its own tooltip (`attachTip` in `main.ts`).
- Don't introduce colour literals in CSS; everything goes through the theme tokens.
- Don't wire this control to the remote/WiiM volume. Different device, different meaning.
- Don't let the gain be applied anywhere but the callback — the audio thread's copy is the
  one that must never be surprised by a lock or an allocation.
- **Don't commit the DEV-CURVE harness.** It is scaffolding for one decision.
- Don't run bare `cargo fmt`, and don't "fix" the rustfmt diffs in `diagnostics.rs`,
  `media_foundation.rs`, `player.rs`'s decode thread or `audio_thread.rs`'s stall chain —
  they are pre-existing and belong to nobody in this task.
- Don't charge a device change to the track's recovery budget.
