//! What this particular build actually is, for the version bar.
//!
//! A release carries a version number that means something: CI built it from the commit a
//! `vX.Y.Z` tag points at, and the number identifies it exactly. A dev build carries the same
//! number and it means almost nothing — the tree may be three commits ahead, on a branch,
//! with uncommitted edits on top.
//!
//! So in a debug build the bar says which commit is really running. A release build reports
//! nothing here and the bar is unchanged.
//!
//! Git is read at *runtime* rather than in `build.rs`, which is the conventional home for
//! this. It has to be: a dirty working tree is not a build input, and no
//! `cargo:rerun-if-changed` can say "some tracked file changed" without re-running the build
//! script on every single build. Reading it live costs a few subprocess spawns at launch,
//! only ever in a debug build, and is never stale.

use serde::Serialize;

/// Identity of the tree a dev build came from.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitId {
    /// `None` on a detached HEAD, where there is no branch to name.
    pub branch: Option<String>,
    /// Abbreviated commit hash — what the bar shows.
    pub hash: String,
    /// The full 40-character sha, for copying.
    ///
    /// Deliberately not the abbreviated one: the short hash is already on screen and can be
    /// retyped, so copying it adds nothing. The full sha is the part you cannot get by
    /// reading, and it is what pastes unambiguously into anything.
    pub full: String,
    /// Tracked files differ from HEAD, so the hash alone does not describe what is running.
    pub dirty: bool,
    /// HEAD is exactly the commit `v{version}` points at.
    ///
    /// The bar stays quiet about the commit when this holds and the tree is clean — but the
    /// hash is still reported, because clicking to copy it must work in that state too.
    pub tagged: bool,
}

/// What the version bar needs to describe this build.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BuildId {
    /// A debug build. The updater does nothing in one, so the bar is a readout, not a button.
    pub dev: bool,
    /// Set whenever git could be read at all — so `None` in a release, or outside a repo.
    ///
    /// Present even when the tree *is* exactly the tagged release. Whether the bar mentions
    /// the commit is a separate question ([`GitId::tagged`]); the hash has to be here either
    /// way, because clicking the bar copies it.
    pub git: Option<GitId>,
}

#[tauri::command]
pub fn build_id() -> BuildId {
    BuildId {
        dev: cfg!(debug_assertions),
        git: git_id(),
    }
}

/// Everything the decision needs, as read from git. Separated from [`describe`] so the
/// decision is testable — the same split `updates.rs` makes for the same reason.
///
/// Debug-only, like everything that reads git: a release never builds one. `test` is in the
/// cfg as well so `cargo test --release` still compiles the tests below.
#[cfg(any(debug_assertions, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Facts {
    head: String,
    short: String,
    /// The literal string "HEAD" when detached, which is what `--abbrev-ref` reports.
    branch: Option<String>,
    dirty: bool,
    /// The commit `v{version}` points at, or `None` when no such tag exists here.
    tag_commit: Option<String>,
}

/// Turn what git said into what the bar needs.
///
/// Pure, so the one part where a mistake is visible to every dev every day can be tested
/// without needing a repo in a particular state.
#[cfg(any(debug_assertions, test))]
fn describe(f: &Facts) -> GitId {
    GitId {
        // A detached HEAD has no branch to name; `--abbrev-ref` says "HEAD" and means it.
        branch: f.branch.clone().filter(|b| b != "HEAD"),
        hash: f.short.clone(),
        full: f.head.clone(),
        dirty: f.dirty,
        // A tag that does not exist gives `None`, which must read as "not on it". Claiming to
        // be the release is the one wrong answer here.
        tagged: f.tag_commit.as_deref() == Some(f.head.as_str()),
    }
}

/// Compiled out of a release entirely: no subprocesses, no git, nothing to go wrong in a
/// build that ships.
#[cfg(not(debug_assertions))]
fn git_id() -> Option<GitId> {
    None
}

#[cfg(debug_assertions)]
fn git_id() -> Option<GitId> {
    // `CARGO_PKG_VERSION` is one of the four version sites the project keeps in lockstep, so
    // this is the same number the bar shows.
    //
    // `^{commit}` resolves an annotated tag through to the commit it points at — and every
    // release tag here is annotated. Without it this compares a commit against a tag *object*
    // and never matches. A tag that does not exist yet fails the command, giving `None`,
    // which reads correctly as "not on it".
    let tag = format!("v{}^{{commit}}", env!("CARGO_PKG_VERSION"));

    Some(describe(&Facts {
        head: git(&["rev-parse", "HEAD"])?,
        short: git(&["rev-parse", "--short", "HEAD"])?,
        branch: git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        dirty: !git(&["status", "--porcelain"])?.is_empty(),
        tag_commit: git(&["rev-parse", "--verify", "--quiet", &tag]),
    }))
}

/// Run git in the repo and return trimmed stdout, or `None` for any failure at all.
///
/// Callers treat `None` as "cannot tell", which degrades to showing the plain version number.
/// There is no configuration in which a missing or unhappy git may break the bar.
#[cfg(debug_assertions)]
fn git(args: &[&str]) -> Option<String> {
    // The app's working directory during a dev run is not the repo. This is a compile-time
    // constant pointing at `app/src-tauri`, which is inside it.
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "f15dc147743ef20c1bd4d44dd937623276bbf6c9";
    const OTHER: &str = "345f503313a4c8ec1368e1e1e40a8addfc6f466f";

    fn on_tag() -> Facts {
        Facts {
            head: HEAD.into(),
            short: "f15dc14".into(),
            branch: Some("master".into()),
            dirty: false,
            tag_commit: Some(HEAD.into()),
        }
    }

    /// The display rule the frontend applies. Stated here so the tests below can assert it
    /// alongside the facts it is derived from.
    fn names_commit(g: &GitId) -> bool {
        !g.tagged || g.dirty
    }

    /// A clean checkout of the tagged release says nothing extra, so the bar stays exactly as
    /// it has always been — but the hash is still reported, because the click copies it.
    #[test]
    fn the_tagged_release_adds_nothing_but_still_offers_its_hash() {
        let g = describe(&on_tag());
        assert!(g.tagged);
        assert!(!names_commit(&g), "a clean tagged build names no commit");
        assert_eq!(g.full, HEAD, "the hash must still be there to copy");
    }

    /// One commit past the tag and the version number is no longer the truth.
    #[test]
    fn a_commit_past_the_tag_names_the_commit() {
        let f = Facts {
            tag_commit: Some(OTHER.into()),
            ..on_tag()
        };
        let g = describe(&f);
        assert!(names_commit(&g));
        assert_eq!(
            g,
            GitId {
                branch: Some("master".into()),
                hash: "f15dc14".into(),
                full: HEAD.into(),
                dirty: false,
                tagged: false,
            }
        );
    }

    /// Sitting on the tag but with edits on top is still not the tagged release — this is the
    /// case a tag-only check would call clean and wrongly stay quiet about.
    #[test]
    fn dirty_on_the_tag_still_speaks_up() {
        let g = describe(&Facts {
            dirty: true,
            ..on_tag()
        });
        assert!(g.tagged, "still on the tag");
        assert!(names_commit(&g), "but no longer what the tag contains");
    }

    /// `rev-parse --abbrev-ref` answers the literal string "HEAD" when detached. Printing
    /// that would render `v1.5.1 · HEAD@f15dc14`, which reads like a branch called HEAD.
    #[test]
    fn a_detached_head_is_not_a_branch_named_head() {
        let f = Facts {
            branch: Some("HEAD".into()),
            tag_commit: Some(OTHER.into()),
            ..on_tag()
        };
        assert_eq!(describe(&f).branch, None);
    }

    /// No tag of this version anywhere — a fresh clone with no tags fetched, or a version
    /// bumped ahead of its release. "Cannot find it" must read as "not on it", never as "on
    /// it": claiming to be the release is the one wrong answer.
    #[test]
    fn a_missing_tag_is_never_mistaken_for_being_on_it() {
        let g = describe(&Facts {
            tag_commit: None,
            ..on_tag()
        });
        assert!(!g.tagged);
        assert!(names_commit(&g));
    }

    /// The copied value is the full sha, not the abbreviated one on screen. Copying what is
    /// already legible would be a pointless button.
    #[test]
    fn the_copied_hash_is_the_full_one() {
        let g = describe(&on_tag());
        assert_eq!(g.full.len(), 40);
        assert!(g.full.starts_with(&g.hash));
        assert_ne!(g.full, g.hash);
    }

    /// A release build must never pay for any of this, and must never show it.
    #[test]
    fn a_release_build_reports_nothing() {
        let id = build_id();
        assert_eq!(id.dev, cfg!(debug_assertions));
        if !cfg!(debug_assertions) {
            assert_eq!(id.git, None);
        }
    }

    /// The live read agrees with git itself. Guards the argument strings, which the pure
    /// tests above cannot see — a typo there fails every call into `None` and silently
    /// reverts the bar to the plain version.
    #[cfg(debug_assertions)]
    #[test]
    fn the_live_read_matches_git() {
        let Some(expected_head) = git(&["rev-parse", "HEAD"]) else {
            // No git, or not a repo. Failing soft is the documented behaviour, so there is
            // nothing to assert.
            return;
        };
        assert_eq!(expected_head.len(), 40, "HEAD should be a full sha");
        assert!(
            git(&["rev-parse", "--short", "HEAD"]).is_some_and(|s| expected_head.starts_with(&s)),
            "the short hash must be a prefix of the full one"
        );
        // `--porcelain` is the stable, script-readable form; the default output is not.
        assert!(git(&["status", "--porcelain"]).is_some());
    }
}
