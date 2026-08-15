# Branch cleanup and the v1.5.0 release

## Goal

Collapse four divergent lines of work into a linear `master`, then cut **v1.5.0** carrying
the BPM technical-footer feature. The branch state had drifted far enough that two
sessions were about to make contradictory assumptions about what was already released.

## Environment / context

- Repo: `Pandora` (the app is **Jarlid**). Primary worktree
  `C:\Users\camer\git\Personal Projects\Pandora`, branch `master`.
- Release is **tag-triggered**: `.github/workflows/release.yml` fires on `push: tags: v*`
  and runs `tauri-apps/tauri-action`, which builds on `windows-latest` and publishes a
  non-draft GitHub release. The app self-updates from that release. Pushing the tag *is*
  the release.
- Version lives at four sites that must move together (see `CLAUDE.md`):
  `app/package.json`, `app/src-tauri/tauri.conf.json`, `app/src-tauri/Cargo.toml`,
  `app/src-tauri/Cargo.lock`.
- There is **no `Cargo.toml` at the repo root**. `crates/` is its own cargo workspace;
  `app/src-tauri/` is a separate manifest.

## Starting state (2026-08-14, before any changes)

```
origin/master              018929b  v1.4.3: the sign-in card, legible at last
master                     97f821f  ahead 3, behind 5
fix-login-unavailable      c63b708  (worktree t3code-fce79af4, clean)
t3code/add-bpm-display     574913b  (worktree t3code-838673ea, clean)
t3code/layout-spacing-...  b3af271  (worktree t3code-c75218f2, DIRTY — active session)
```

## Decisions already made (don't re-ask)

1. **The v1.4.3 hotfix is already on `origin/master`** — it was rebased and pushed there.
   `git cherry -v origin/master fix-login-unavailable` returns `-` for all five commits.
   "Merge the hotfix into master" is therefore satisfied by the rebase alone; there is
   nothing to merge.
2. **BPM lands by cherry-pick, not by rebasing its branch.** Chosen over rebasing
   `t3code/add-bpm-display` onto the new master. Reason: the BPM branch lives in a separate
   t3 worktree, and cherry-picking rewrites nothing outside the primary worktree. The cost
   is a stale duplicate branch left behind, accepted deliberately.
3. **The DJ-blend-mode probe (`574913b`) stays off master.** It is exploration for a future
   feature — an example binary and a plan doc, no product code. It remains on
   `t3code/add-bpm-display`, which is being preserved anyway per decision 2.
4. **`fix-login-unavailable` gets deleted** — branch, remote branch, and worktree. Fully
   absorbed; the `v1.4.3` tag keeps its commits reachable.
5. **The `v1.4.2` / `v1.4.3` tags are left exactly as they are.** See Findings.
6. **Target version is 1.5.0**, a minor bump — the release adds a user-visible feature.
   Version bump is its own commit, containing only version numbers.

## Plan / steps

1. [x] Map the real topology; verify hotfix absorption by patch-equivalence, not by subject line.
2. [x] Confirm all three local `master` commits are genuinely new.
3. [x] Confirm the BPM feature is finished and the DJ probe is separable.
4. [x] Rebase `master` onto `origin/master` (replays 3 commits).
5. [x] Cherry-pick the 4 BPM commits `b058b09..b19564c` onto `master`.
6. [x] Run the repo's checks: `cargo test`, `cargo clippy`, `bun run build`, `cargo check` on the Tauri crate.
7. [ ] Version-bump commit to 1.5.0 across the four sites. **Version numbers only.**
8. [ ] Push `master`.
9. [ ] Tag `v1.5.0` and push the tag — this publishes the release.
10. [ ] Delete `fix-login-unavailable`: worktree, local branch, remote branch.

## Findings / gotchas

### The hotfix was already upstream, under different hashes

`origin/master` carries `f52bba2 / f7dec55 / b269016 / 27f989d / 018929b`, which are
rebased copies of `68b7d22 / a32fad2 / 2fe09d0 / a8a2d95 / c63b708` on
`fix-login-unavailable`. Identical subject lines, different hashes. Comparing by subject
suggests two independent lines of work; `git cherry` proves they are the same patches.
**Always check patch-equivalence before concluding a branch is unmerged.**

### The v1.4.2 and v1.4.3 tags point off master's history

`v1.4.3 -> c63b708` and `v1.4.2 -> a32fad2` — the *pre-rebase* originals, which are not
ancestors of `origin/master`. Both tags are already pushed, and the release binaries users
are self-updating from were built from them. Retagging would mean deleting a published tag
and invalidating the artifacts built from it. **Left alone on purpose.** The content is
identical to what is on master; only the hashes differ.

### Rebasing master orphans the BPM branch's base

`t3code/add-bpm-display` forks from `b7a9aae`, one of the three local `master` commits the
rebase rewrites. A naive merge after the rebase would drag in duplicate copies of
`8d07d6f` and `b7a9aae`. This is why BPM lands by cherry-pick.

### Three local commits, not one

An initial read suggested only `97f821f` was unpushed. It is three: `8d07d6f`, `b7a9aae`,
`97f821f`. `git cherry -v origin/master master` returns `+` for all three, and
`git branch -a --contains` finds `8d07d6f`/`b7a9aae` only on `master` and the BPM branch.

### The cherry-pick auto-merged native.rs, correctly

`app/src-tauri/src/native.rs` differs between `b19564c` and the new `master` by exactly 55
insertions / 9 deletions — which is precisely `f52bba2 login: ask whether we need
credentials`, a commit the BPM branch never had. Verified that master's `native.rs` carries
*both* the login latch (`needs_login` / `require_login`) and the BPM ticker
(`engine://technical`). Not a bad merge.

### Two pre-existing warnings, neither introduced here

- `clippy::items_after_test_module` at `crates/audio/src/player.rs` — `impl Drop for Player`
  sits after `mod tests`. Present on `origin/master` at lines 591/612; the BPM additions
  only shifted it to 675/696. **Not ours; don't "fix" it** (see the `cargo fmt` rule).
- `dead_code` on `ImportPlan::stations_to_create` / `total_seeds_to_add` in
  `app/src-tauri/src/import.rs` — the half-built import feature. Untouched by this work.

### The layout-spacing worktree has uncommitted work

`t3code-c75218f2` has a modified `app/src/styles.css` and an untracked
`plans/layout-alignment-pass.md` — an active session. Its branch pointer `b3af271` is
already an ancestor of `origin/master`, so nothing in this task touches it. **Do not go
near that worktree.**

## Progress log

- [x] Fetched, mapped topology, confirmed working tree clean
- [x] Proved hotfix absorption via `git cherry`
- [x] Confirmed versions consistent at 1.4.3 across all four sites on `origin/master`
- [x] Confirmed BPM plan doc fully checked off (tests, clippy, typecheck, live app run)
- [x] Rebase — clean, 3 commits replayed, `master` ahead 3 / behind 0
- [x] Cherry-pick — 4 commits, no conflicts; DJ probe confirmed absent
- [x] Checks — `cargo test` 34 passed / 0 failed; clippy clean bar the pre-existing warning;
      `bun run build` (tsc + vite) clean; `cargo check` on the Tauri crate clean.
      All run through the `compute-budget` broker, sequentially, on an idle machine.
- [ ] Bump
- [ ] Push + tag
- [ ] Delete hotfix branch

## Things not to do

- Don't retag `v1.4.2` / `v1.4.3` to "fix" them pointing off-history. Published tags.
- Don't touch the `t3code-c75218f2` worktree — another session has uncommitted work there.
- Don't rebase `t3code/add-bpm-display`; the whole point of cherry-picking was to leave it alone.
- Don't merge `574913b` (DJ probe) into master.
- Don't run bare `cargo fmt` — it reformats files outside the diff. Use `rustfmt --check <file>`.
- Don't run `cargo` from the repo root; there is no manifest there.
- Don't fold the version bump into the BPM commits. Bump is its own commit.
