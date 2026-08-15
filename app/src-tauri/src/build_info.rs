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
    /// Abbreviated commit hash.
    pub hash: String,
    /// Tracked files differ from HEAD, so the hash alone does not describe what is running.
    pub dirty: bool,
}

/// What the version bar needs to describe this build.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BuildId {
    /// A debug build. The updater does nothing in one, so the bar is a readout, not a button.
    pub dev: bool,
    /// Set only when the tree is **not** exactly the tagged release — off the tag, or dirty.
    ///
    /// `None` is the ordinary case for a release, and also for a dev build sitting clean on
    /// its own tag. The extra text marks a *difference*; it would be furniture if it were
    /// always there.
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

/// Whether this tree differs from the tagged release, and how.
///
/// Pure, so the one part where a mistake is visible to every dev every day can be tested
/// without a repo in a particular state.
fn describe(f: &Facts) -> Option<GitId> {
    // Exactly the tagged release, byte for byte. Nothing worth adding to the bar.
    if f.tag_commit.as_deref() == Some(f.head.as_str()) && !f.dirty {
        return None;
    }
    Some(GitId {
        // A detached HEAD has no branch to name; `--abbrev-ref` says "HEAD" and means it.
        branch: f.branch.clone().filter(|b| b != "HEAD"),
        hash: f.short.clone(),
        dirty: f.dirty,
    })
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

    describe(&Facts {
        head: git(&["rev-parse", "HEAD"])?,
        short: git(&["rev-parse", "--short", "HEAD"])?,
        branch: git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        dirty: !git(&["status", "--porcelain"])?.is_empty(),
        tag_commit: git(&["rev-parse", "--verify", "--quiet", &tag]),
    })
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

    /// The whole point of the `Option`: a clean checkout of the tagged release says nothing
    /// extra, so the bar stays exactly as it has always been.
    #[test]
    fn the_tagged_release_adds_nothing() {
        assert_eq!(describe(&on_tag()), None);
    }

    /// One commit past the tag and the version number is no longer the truth.
    #[test]
    fn a_commit_past_the_tag_names_the_commit() {
        let f = Facts {
            tag_commit: Some(OTHER.into()),
            ..on_tag()
        };
        assert_eq!(
            describe(&f),
            Some(GitId {
                branch: Some("master".into()),
                hash: "f15dc14".into(),
                dirty: false,
            })
        );
    }

    /// Sitting on the tag but with edits on top is still not the tagged release — this is the
    /// case a hash-only check would call clean and wrongly stay quiet about.
    #[test]
    fn dirty_on_the_tag_still_speaks_up() {
        let f = Facts {
            dirty: true,
            ..on_tag()
        };
        assert_eq!(describe(&f).map(|g| g.dirty), Some(true));
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
        assert_eq!(describe(&f).and_then(|g| g.branch), None);
    }

    /// No tag of this version anywhere — a fresh clone with no tags fetched, or a version
    /// bumped ahead of its release. "Cannot find it" must read as "not on it", never as "on
    /// it": claiming to be the release is the one wrong answer.
    #[test]
    fn a_missing_tag_is_never_mistaken_for_being_on_it() {
        let f = Facts {
            tag_commit: None,
            ..on_tag()
        };
        assert_eq!(describe(&f).map(|g| g.hash), Some("f15dc14".into()));
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
