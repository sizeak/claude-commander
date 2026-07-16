//! Branch reconciliation: keep `WorktreeSession.branch` in sync with a
//! worktree's live git HEAD after a branch **rename**.
//!
//! `session.branch` is captured at creation and drives every PR lookup
//! (`gh pr list --head <branch>`) plus stack-parent matching. If the user
//! renames the underlying git branch (`git branch -m old new`), the stored name
//! goes stale, the PR poll queries a branch that no longer has a PR, and the
//! session silently loses its PR + stack topology. This module holds the pure
//! decision of *whether* to adopt the live branch as the session's new name;
//! the IO (listing worktrees, checking ref existence) is done by the caller in
//! [`crate::api::CommanderService::reconcile_session_branches`].
//!
//! The rule deliberately fires **only** on a rename — never on a plain
//! `git switch` to another existing branch — so a user who drives one session
//! across several existing branches is never thrashed. See
//! [`decide_branch_reconcile`] for the exact predicate.

/// Decide whether a session's stored branch should be reconciled to `live`,
/// the worktree's current HEAD branch. Returns `Some(new_branch)` to adopt it,
/// or `None` to leave `session.branch` unchanged.
///
/// Pure: every git fact is passed in so this is exhaustively unit-testable.
///
/// - `stored` — the session's currently recorded branch.
/// - `live` — the worktree's live HEAD branch, or `None` when HEAD is detached,
///   the worktree is missing, or the branch could not be read.
/// - `stored_local_ref_exists` — whether `refs/heads/<stored>` still exists.
/// - `stored_remote_ref_exists` — whether `refs/remotes/origin/<stored>` exists.
/// - `main_branch` — the project's default branch.
/// - `sibling_branches` — stored branches of the *other* non-`Creating`
///   sessions in the same project.
///
/// The guards, in order:
/// 1. No live branch, or it already equals `stored`, or it's the synthetic
///    `"HEAD"` (detached) → nothing to do.
/// 2. The stored branch still exists locally **or** as `origin/<stored>` → this
///    is a plain switch (or an as-yet-unpushed rename whose PR still heads the
///    old name); leave it. Reconciling here would wipe a PR that is currently
///    displaying correctly.
/// 3. The live branch is the project's main branch → the session's branch was
///    deleted and HEAD fell back to main (abandonment, not a rename); leave it.
/// 4. The live branch is already owned by another session → adopting it would
///    make two sessions claim one branch and corrupt stack matching; leave it.
/// 5. Otherwise the old name is gone and the new one is unclaimed → adopt it.
pub fn decide_branch_reconcile(
    stored: &str,
    live: Option<&str>,
    stored_local_ref_exists: bool,
    stored_remote_ref_exists: bool,
    main_branch: &str,
    sibling_branches: &[&str],
) -> Option<String> {
    let live = live?;
    if live == stored || live == "HEAD" {
        return None;
    }
    if stored_local_ref_exists || stored_remote_ref_exists {
        return None;
    }
    if live == main_branch {
        return None;
    }
    if sibling_branches.contains(&live) {
        return None;
    }
    Some(live.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Convenience with the common "no refs, main is `main`, no siblings" case.
    fn decide(stored: &str, live: Option<&str>, local: bool, remote: bool) -> Option<String> {
        decide_branch_reconcile(stored, live, local, remote, "main", &[])
    }

    #[test]
    fn renamed_branch_is_adopted() {
        // Old name gone (no local, no remote), live differs → adopt.
        assert_eq!(decide("old", Some("new"), false, false), Some("new".into()));
    }

    #[test]
    fn unchanged_branch_is_left() {
        assert_eq!(decide("same", Some("same"), false, false), None);
    }

    #[test]
    fn detached_head_is_left() {
        assert_eq!(decide("old", Some("HEAD"), false, false), None);
    }

    #[test]
    fn missing_live_branch_is_left() {
        // Worktree not found / unreadable → no candidate.
        assert_eq!(decide("old", None, false, false), None);
    }

    #[test]
    fn plain_switch_is_left_when_stored_local_ref_exists() {
        // `git switch other` — the stored branch still exists locally, so this
        // is a transient switch, not a rename.
        assert_eq!(decide("old", Some("other"), true, false), None);
    }

    #[test]
    fn unpushed_rename_is_left_when_remote_ref_exists() {
        // Renamed locally but not pushed: GitHub's PR still heads `old`, and
        // `origin/old` still exists. Reconciling would wipe a working PR;
        // defer until the rename is pushed and `origin/old` disappears.
        assert_eq!(decide("old", Some("new"), false, true), None);
    }

    #[test]
    fn live_equal_to_main_is_left() {
        // Stored branch deleted, HEAD fell back to main → abandonment.
        assert_eq!(
            decide_branch_reconcile("old", Some("main"), false, false, "main", &[]),
            None
        );
    }

    #[test]
    fn collision_with_sibling_is_left() {
        // Adopting `new` would make this session and the sibling both claim it.
        assert_eq!(
            decide_branch_reconcile("old", Some("new"), false, false, "main", &["new"]),
            None
        );
    }

    #[test]
    fn adopt_when_sibling_set_does_not_collide() {
        assert_eq!(
            decide_branch_reconcile("old", Some("new"), false, false, "main", &["unrelated"]),
            Some("new".into())
        );
    }
}
