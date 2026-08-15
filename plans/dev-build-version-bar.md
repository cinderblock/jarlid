# The version bar tells the truth in a dev build

## Goal

In a **dev build**, the bottom-right version bar should say which commit is actually
running, rather than a version number that is only meaningful for a tagged release. A
release build is unchanged.

## Environment / context

- Repo `Pandora`, app **Jarlid**, primary worktree
  `C:\Users\camer\git\Personal Projects\Pandora`, branch `master`.
- The bar is `<button id="version">` (`app/index.html:32`), styled at
  `app/src/styles.css:387`. It doubles as the **update badge** — `renderVersion()` in
  `app/src/main.ts` overwrites its text whenever an update is known.
- `baseVersion` comes from `getVersion()` (Tauri API → `tauri.conf.json`), set at
  `main.ts:723`.
- `app/src-tauri/build.rs` already exists (generates taskbar glyphs from `index.html`) and
  already uses `cargo:rerun-if-changed`.
- "Dev build" means `debug_assertions`. That is the same gate the updater's background loop
  already uses: `lib.rs:556` has `#[cfg(not(debug_assertions))] updates::spawn(...)`.

## Decisions already made (don't re-ask)

1. **Format is `v1.5.1 · master@f15dc14`.** The version stays alongside the branch and hash,
   so the release the tree is based on is still readable without running git. Chosen over
   branch-and-hash-only and over raw `git describe` output.
2. **A dirty tree gets a trailing `*`** — `v1.5.1 · master@f15dc14*`. A dev build from a
   dirty tree is exactly the case where a hash alone misrepresents what is running.
3. **Detached HEAD drops the branch**: `v1.5.1 · f15dc14`.
4. **On the tag and clean → plain `v1.5.1`**, i.e. today's behaviour. The extra text is a
   signal that something is *off* the tag; it must not become permanent furniture.
5. **The click never installs in a dev build.** `update_action` is a registered command, so
   until now a debug build could be talked into downloading and installing a release over
   itself by clicking the bar — despite `spawn` being compiled out for exactly that reason.
   Closing the other way in.
6. **In a dev build the click copies the full sha instead** (added 2026-08-15, shipped in
   v1.5.3). The *full* one, not the abbreviated one on screen: the short hash is already
   legible and could be retyped, so copying it would be a pointless button. This is why
   `BuildId::git` is now populated even when the tree is exactly the tagged release — the bar
   stays quiet there, but the hash still has to be available to copy.
7. **No clipboard dependency.** `navigator.clipboard` with an `execCommand` fallback, rather
   than adding `tauri-plugin-clipboard-manager`. See Findings.
6. **Git is read at runtime, not in `build.rs`.** See Findings.
7. **Release builds are untouched.** Gated on `debug_assertions`, and fails soft.

## Plan / steps

1. [ ] `app/src-tauri/src/build_info.rs` — a `build_id` command returning `{ dev, git }`,
   where `git` is `Some { branch, hash, dirty }` only when the tree is *not* exactly the
   tagged release.
2. [ ] Register the command and the module in `lib.rs`.
3. [ ] `update_action` returns early under `debug_assertions`.
4. [ ] `main.ts` — fold the suffix into `baseVersion`; make the bar inert and drop the
   click handler in a dev build; tip explains why.
5. [ ] Verify: `cargo test`, `cargo clippy`, `bun run build`, `rustfmt --check` on my files.
6. [ ] Drive a real dev build and read the bar.

## Findings / gotchas

### Runtime git, not `build.rs`

`build.rs` is the conventional home for this, but it cannot answer the question here:

- **The dirty flag is not a build input.** Editing a file must change the bar; nothing in
  `cargo:rerun-if-changed` can express "the working tree changed" without re-running the
  build script on literally every build.
- **New commits on the same branch** move `.git/refs/heads/<branch>`, which may not exist as
  a loose file at all once refs are packed — so `rerun-if-changed` on it is unreliable.

Runtime costs a few subprocess spawns at launch, only ever in a debug build, and is always
correct. `#[cfg(debug_assertions)]` compiles the whole thing out of a release.

### The repo path must be baked in, not inferred

The app's working directory in a dev run is not the repo root. `env!("CARGO_MANIFEST_DIR")`
is a compile-time constant pointing at `app/src-tauri`, which is inside the repo — pass it to
`git -C`. Do **not** use `std::env::current_dir()`.

### `cargo check --release` catches what `cargo test` cannot

Splitting the logic into a pure `describe()` over a `Facts` struct made both of them
**dead code in a release build** — they are only ever reached from the `#[cfg(debug_assertions)]`
`git_id()`. Debug tests and clippy were perfectly happy; only a release check said so:

```
warning: struct `Facts` is never constructed
warning: function `describe` is never used
```

Fixed with `#[cfg(any(debug_assertions, test))]` on both — `test` is in there so
`cargo test --release` still compiles the test module. **Run `cargo check --release` after
touching anything cfg-gated in here**, because the shipping build is a genuinely different
compilation and debug-only checks will not see it.

### The clipboard, without a dependency

`navigator.clipboard.writeText` should simply work: Tauri serves the app from
`tauri.localhost`, which Chromium counts as a secure context, and a click supplies the user
activation the API wants. The deprecated `document.execCommand("copy")` sits behind it as a
fallback for anywhere that turns out not to hold.

Chosen over adding `tauri-plugin-clipboard-manager` (a Rust dep, a JS dep and a capabilities
entry) for a dev-only affordance, in a codebase that visibly resists dependencies — the BPM
work went as far as writing its own autocorrelation to avoid pulling in an FFT crate.

The fallback textarea is positioned off-screen rather than `display: none`, because a hidden
element cannot be selected and `execCommand` would copy nothing.

### The bar is the update badge too

`renderVersion()` rewrites the text for every update state. The git suffix therefore belongs
in `baseVersion` (the no-update-known branch), not bolted on afterwards, or the first status
event erases it.

## Progress log

- [x] Read the badge, its styles, `build.rs`, and the existing debug gating
- [x] `build_info.rs` — `build_id` command, pure `describe()`, 7 tests
- [x] Wire-up (`lib.rs` module + handler) and the dev-inert `update_action`
- [x] Frontend — suffix folded into `baseVersion`, `.readonly` bar, tip, click bail
- [x] Checks — `cargo test` 75 passed / 0 failed; clippy no new warnings; **release**
      `cargo check --release` clean (the cfg path that ships); `bun run build` clean;
      `rustfmt --check` clean on both files I touched
- [ ] **Real dev-build eyeball — the one thing left.** See below.

## What could not be verified from here

The logic is tested, including a live read that runs the real git commands and checks they
agree with git itself. What no test here can see is the **window**: that the text renders in
the corner as `v1.5.1 · master@f15dc14*` without wrapping or colliding with the `#tech`
readout opposite it.

Measured rather than guessed: at `v1.5.1 · master@f15dc14*` the string is ~24 characters,
*shorter* than the longest thing the bar already shows in a release
(`updating to v1.5.1 after this song`, ~34), so the common case adds no new risk. The case
that did was a long branch — `t3code/layout-spacing-alignment-fixes` would have made it ~54.
Hence the 24-character clip on the branch specifically. `#tech` solves the same problem with
`max-width: 34vw` + `text-overflow: ellipsis`, which is wrong here because it trims the end
— the hash.

Run `bun run tauri dev` in `app/` and look at the bottom-right corner.

**The clipboard write is the other unverifiable half.** `navigator.clipboard` cannot be
exercised by a Rust test or by loading the page in a normal browser (which has no `build_id`
command and so falls back to showing the plain version). Click the bar in a dev build and
paste somewhere: it should give a 40-character sha, and the bar should say `commit copied`
for a moment. If it says `copy failed`, both the async API and the `execCommand` fallback
were refused, and the plugin route in Decisions #7 is the answer.

## Things not to do

- Don't let this text appear in a release build, and don't let a missing/broken git make the
  bar show anything but the plain version — fail soft.
- Don't put the git read in `build.rs` (see Findings).
- Don't use `title=` for the tip; the badge already uses `attachTip`.
- Don't touch the four version files — no bump is part of this task.
- Don't run bare `cargo fmt`; check only the files in this diff.
