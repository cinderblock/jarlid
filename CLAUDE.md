# Jarlid — project instructions

## Version bumps are their own commit, and never your idea

**A version bump commit contains only version numbers.** Never mix a bump into a commit that
also carries a feature, a fix, or an icon change. All four sites move together, in one
commit of their own:

- `app/package.json`
- `app/src-tauri/tauri.conf.json`
- `app/src-tauri/Cargo.toml`
- `app/src-tauri/Cargo.lock`

Copy the existing pattern — `7a7b40d` (`v1.3.3: …`), `266ed15` (`v1.3.4: …`): a four-file,
version-only diff whose subject starts with the new `vX.Y.Z`. The release workflow derives
the version from the git tag, so that commit is what the tag points at.

**Never change a version number unless changing it is the task you were given.** Not to
"finish" a bump someone else started, not to fix an inconsistency you noticed, not as part of
shipping a feature. If the four files disagree, say so and leave them alone — a disagreement
means someone is mid-decision about the next release, and that decision isn't yours.

This is not hypothetical. `8200fab` mixed a bump into an icon commit, leaving two files at
1.4.0 and two at 1.3.3. Another session read that split, concluded the repo was "mid-bump to
1.4.0", and wrote 1.4.0 into the Cargo files — but the icon change was a patch, so the
version had to be walked back to 1.3.4 while an uncommitted 1.4.0 sat in the shared working
tree. The inference felt obviously right and was wrong.

## This tree is shared between concurrent sessions

More than one agent may be working in this working tree at once, with uncommitted changes
that are not yours.

- Stage only your own hunks. Never `git checkout -- <file>`, `git restore`, or
  `git reset --hard` to get a clean base.
- Re-read `git log` before assuming a baseline; HEAD moves under you mid-task.
- If your edit conflicts with uncommitted work in the same file, apply yours on top rather
  than replacing the file.

## Layout notes that are easy to get wrong

- **There is no `Cargo.toml` at the repo root.** `crates/` is its own cargo workspace
  (`audio`, `engine`, `pandora`); the Tauri app is a separate manifest at `app/src-tauri/`.
  Running `cargo` from the repo root fails.
- `bun run build` in `app/` is `tsc && vite build`. The dev server is port 1420, `strictPort`.
- `cargo fmt` with no arguments reformats pre-existing code you did not touch. Check only the
  files in your diff (`rustfmt --check <file>`) and fix only your own hunks.

## UI

Every control on the Settings page is drawn by the app, not the platform. No native
`<select>`, no UA-drawn radios or checkboxes, and **no `title=` tooltip attributes anywhere**
— they are invisible on touch and hide information behind a hover. Put the information inline
or use `attachTip` in `main.ts`.
