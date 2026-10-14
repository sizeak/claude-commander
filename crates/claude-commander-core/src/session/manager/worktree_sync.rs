//! Worktree synchronization: import unmanaged git worktrees as sessions.

use super::*;

impl SessionManager {
    /// Sync unmanaged git worktrees as stopped sessions
    ///
    /// Lists actual git worktrees for the project and imports any that aren't
    /// already tracked as sessions. Imported worktrees get `Stopped` status —
    /// they have no running tmux session but can be attached to (which will
    /// recreate the tmux session on demand).
    #[instrument(skip(self))]
    pub async fn sync_worktrees(&self, project_id: &ProjectId) -> Result<usize> {
        let (repo_path, worktree_paths, stored_origin_url) = {
            let state = self.store.read().await;
            let project = match state.get_project(project_id) {
                Some(p) => p,
                None => return Ok(0),
            };

            let repo_path = project.repo_path.clone();
            let stored_origin_url = project.origin_url.clone();

            // Snapshot existing session worktree paths to canonicalize off-lock.
            let paths: Vec<PathBuf> = project
                .worktrees
                .iter()
                .filter_map(|sid| state.get_session(sid))
                .map(|s| s.worktree_path.clone())
                .collect();

            (repo_path, paths, stored_origin_url)
        };

        // Canonicalize existing session paths without holding the state lock.
        let mut existing_paths: Vec<PathBuf> = Vec::with_capacity(worktree_paths.len());
        for path in worktree_paths {
            if let Ok(canonical) = tokio::fs::canonicalize(&path).await {
                existing_paths.push(canonical);
            }
        }

        // Open git backend and list worktrees
        let backend = match GitBackend::open(&repo_path) {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to open git backend for sync: {}", e);
                return Ok(0);
            }
        };

        let worktrees_dir = self.config_store.read().worktrees_dir()?;
        let canonical_worktrees_dir = tokio::fs::canonicalize(&worktrees_dir)
            .await
            .unwrap_or_else(|_| worktrees_dir.clone());
        // The project's default branch is the target an imported worktree's
        // review diff should be based against. Capture it before the backend is
        // moved into the worktree manager.
        let default_branch = backend.detect_main_branch().ok();
        // Reconcile `origin_url` against the repo, using the backend we already
        // hold. This is repair-on-every-read, never version-gated: `state.json`
        // is multi-writer and an older binary can drop the field again at any
        // time, so a one-shot marker would stop firing exactly when it is still
        // needed.
        //
        // It *corrects* as well as fills. A repo that was renamed, transferred
        // to another owner, or switched between ssh and https keeps an origin
        // that no longer identifies it, which would silently mis-badge it in
        // the repo picker this field feeds. Equally, a repo whose `origin` was
        // removed settles back to `None` rather than pinning a remote it no
        // longer has.
        //
        // Writing only on a difference is what keeps that idempotent: an
        // unchanged origin (including a repo that has none) costs one local
        // config read and no state write, so this can't churn `state.json` on
        // every sync.
        let resolved_origin_url = backend.origin_url();
        let origin_url_changed = resolved_origin_url != stored_origin_url;
        let worktree_manager = WorktreeManager::new(backend, worktrees_dir);

        if origin_url_changed {
            let pid = *project_id;
            self.store
                .mutate(move |state| {
                    if let Some(project) = state.projects.get_mut(&pid) {
                        project.origin_url = resolved_origin_url;
                    }
                })
                .await?;
        }

        let worktrees = match worktree_manager.list_worktrees().await {
            Ok(w) => w,
            Err(e) => {
                warn!("Failed to list worktrees for sync: {}", e);
                return Ok(0);
            }
        };

        // Also canonicalize the repo path for main worktree comparison
        let canonical_repo = tokio::fs::canonicalize(&repo_path)
            .await
            .unwrap_or(repo_path);

        let mut imported = 0;
        let mut new_sessions = Vec::new();

        for wt in &worktrees {
            if wt.is_main {
                continue;
            }

            let canonical_wt = match tokio::fs::canonicalize(&wt.path).await {
                Ok(p) => p,
                Err(_) => continue, // Worktree path doesn't exist, skip
            };

            // Skip if this path matches the main repo
            if canonical_wt == canonical_repo {
                continue;
            }

            // Only import worktrees inside the managed worktrees directory
            if !canonical_wt.starts_with(&canonical_worktrees_dir) {
                continue;
            }

            // Skip if already tracked by an existing session
            if existing_paths.contains(&canonical_wt) {
                continue;
            }

            let mut session = WorktreeSession::new(
                *project_id,
                wt.branch.clone(),
                wt.branch.clone(),
                wt.path.clone(),
                self.config_store.read().default_session_program(),
            );
            session.set_status(SessionStatus::Stopped);
            session.base_commit = Some(
                crate::git::import_base_commit(&wt.path, &wt.head, default_branch.as_deref()).await,
            );
            // An imported worktree's review base is the project's default branch
            // (its fork point), resolved live so the diff tracks that branch.
            session.base_branch = default_branch.clone();

            info!(
                "Importing unmanaged worktree as session: branch={}, path={:?}",
                wt.branch, wt.path
            );

            new_sessions.push(session);
            imported += 1;
        }

        if !new_sessions.is_empty() {
            self.store
                .mutate(move |state| {
                    for session in new_sessions {
                        state.add_session(session);
                    }
                })
                .await?;
        }

        if imported > 0 {
            info!(
                "Synced {} unmanaged worktree(s) for project {}",
                imported, project_id
            );
        }

        Ok(imported)
    }
}
