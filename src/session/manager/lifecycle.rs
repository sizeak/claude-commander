//! Session lifecycle: create, restart, kill, and delete sessions.

use super::*;
use crate::agent::AgentKind;

impl SessionManager {
    /// Prepare a placeholder session in `Creating` state.
    ///
    /// This inserts the session into state immediately so the UI can show a
    /// spinner. Call `finalize_session` in a background task to do the heavy
    /// git/tmux work.
    ///
    /// When `base_branch` is `Some`, the worktree will be created against
    /// that branch (existing local branch, or created from `origin/<branch>`
    /// if only the remote tracking branch exists). When `None`, a new branch
    /// is generated from `title` using the configured branch prefix.
    #[instrument(skip(self))]
    pub async fn prepare_session(
        &self,
        project_id: &ProjectId,
        title: String,
        program: Option<String>,
        base_branch: Option<String>,
    ) -> Result<SessionId> {
        let program = program.unwrap_or_else(|| self.config_store.read().default_program.clone());

        // Validate project exists
        {
            let state = self.store.read().await;
            state
                .get_project(project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?;
        }

        let branch_name = match base_branch {
            Some(b) => b,
            None => self.generate_branch_name(&title),
        };

        let session = WorktreeSession::new_creating(*project_id, title, branch_name, program);
        let session_id = session.id;

        self.store
            .mutate(move |state| {
                state.add_session(session);
            })
            .await?;

        info!("Prepared creating session {}", session_id);
        Ok(session_id)
    }

    /// If `base_branch` matches an existing session's branch in the same
    /// project, link the new session as stacked by setting
    /// `stack_parent_session_id`. When multiple sessions share a branch, the
    /// most recently created one is chosen. No-op when `base_branch` is `None`
    /// or doesn't match any session.
    ///
    /// Call this between `prepare_session` and `finalize_session` so that
    /// `finalize_session` can inject the PR-base context into the Claude
    /// prompt and cascade/push-stack operations recognise the relationship.
    pub async fn link_stack_parent_by_branch(
        &self,
        session_id: &SessionId,
        base_branch: Option<&str>,
    ) -> Result<()> {
        let Some(base) = base_branch else {
            return Ok(());
        };
        let sid = *session_id;
        let base = base.to_string();
        self.store
            .mutate(move |state| {
                let session_project = state.get_session(&sid).map(|s| s.project_id);
                if let Some(pid) = session_project {
                    let parent_id = state
                        .sessions
                        .values()
                        .filter(|s| s.project_id == pid && s.branch == base && s.id != sid)
                        .max_by_key(|s| s.created_at)
                        .map(|s| s.id);
                    if let Some(parent_id) = parent_id
                        && let Some(session) = state.get_session_mut(&sid)
                    {
                        session.stack_parent_session_id = Some(parent_id);
                    }
                }
            })
            .await
    }

    /// Finalize a session that was created with `prepare_session`.
    ///
    /// Performs the heavy work: git fetch, worktree creation, tmux session
    /// setup. On success, transitions the session from `Creating` to `Running`.
    ///
    /// When `base_branch` is `Some`, the session's (freshly generated) branch
    /// is forked off that base branch rather than `origin/<main>`. This backs
    /// the CLI `--base-branch` flag. For stacked sessions the fork point is
    /// derived from the stack parent instead and takes precedence.
    #[instrument(skip(self))]
    pub async fn finalize_session(
        &self,
        session_id: &SessionId,
        initial_prompt: Option<String>,
        base_branch: Option<String>,
    ) -> Result<SessionId> {
        // Read session and project info, plus stack parent's branch if any so
        // we know to fork from it below.
        let (project_id, title, branch_name, program, stack_parent_branch) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            let parent_branch = session
                .stack_parent_session_id
                .and_then(|pid| state.get_session(&pid))
                .map(|p| p.branch.clone());
            (
                session.project_id,
                session.title.clone(),
                session.branch.clone(),
                session.program.clone(),
                parent_branch,
            )
        };

        let (repo_path, main_branch) = {
            let state = self.store.read().await;
            let project = state
                .get_project(&project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?;
            (project.repo_path.clone(), project.main_branch.clone())
        };

        info!(
            "Finalizing session '{}' with branch '{}' in project {}",
            title, branch_name, project_id
        );
        let finalize_start = std::time::Instant::now();

        // Fetch latest changes from origin
        if self.config_store.read().fetch_before_create {
            info!(
                "Fetching latest changes from origin in {}",
                repo_path.display()
            );
            let fetch_start = std::time::Instant::now();
            let output = tokio::process::Command::new("git")
                .current_dir(&repo_path)
                .args(["fetch", "origin"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await?;
            info!(
                "[timing] git fetch origin took {}ms",
                fetch_start.elapsed().as_millis()
            );
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("git fetch failed (continuing anyway): {}", stderr);
            }
        }

        // Generate unique worktree name
        let worktree_name = format!(
            "{}-{}",
            self.sanitize_name(&title),
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("")
        );

        // Create worktree — sync gix work (branch check + start point) is done
        // in a block so non-Sync types are dropped before the first .await,
        // keeping the overall future Send.
        let repo_name = sanitize_name(
            repo_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown"),
        );
        let worktrees_dir = self.config_store.read().resolve_worktrees_dir(&repo_name)?;
        let (branch_exists, branch_preexisted, start_point) = {
            let backend = GitBackend::open(&repo_path)?;
            let exists = backend.branch_exists(&branch_name)?;
            // Whether this session adopts a branch that may already carry
            // commits (Checkout Branch, locally or from `origin/<branch>`),
            // rather than a fresh branch we create empty. Drives whether
            // `base_commit` records the fork point or HEAD (see below).
            let preexisted =
                exists || backend.ref_exists(&format!("refs/remotes/origin/{}", branch_name))?;
            // Fork point for the new branch. A stacked parent's branch takes
            // precedence (it exists locally thanks to its own worktree);
            // otherwise honour the CLI `--base-branch` value. Both mean "create
            // the new branch off this base", overriding the origin/<main>
            // fallback below.
            let fork_base = stack_parent_branch.as_deref().or(base_branch.as_deref());
            let sp = if exists {
                // The branch already exists locally — `git worktree add` will
                // just check it out, so no explicit start point is needed.
                None
            } else if let Some(base) = fork_base {
                // Fork off the base branch, preferring the local branch and
                // falling back to its remote tracking ref, then origin/<main>.
                let base_remote_ref = format!("refs/remotes/origin/{}", base);
                let main_remote_ref = format!("refs/remotes/origin/{}", main_branch);
                if backend.branch_exists(base)? {
                    Some(base.to_string())
                } else if backend.ref_exists(&base_remote_ref)? {
                    Some(format!("origin/{}", base))
                } else if backend.ref_exists(&main_remote_ref)? {
                    Some(format!("origin/{}", main_branch))
                } else {
                    None
                }
            } else {
                // Prefer origin/<branch_name> as the start point when the local
                // branch doesn't exist — this supports checking out an existing
                // remote branch (e.g. via the Checkout modal) as well as falling
                // back to origin/<main_branch> when creating a fresh branch.
                let branch_remote_ref = format!("refs/remotes/origin/{}", branch_name);
                let main_remote_ref = format!("refs/remotes/origin/{}", main_branch);
                if backend.ref_exists(&branch_remote_ref)? {
                    Some(format!("origin/{}", branch_name))
                } else if backend.ref_exists(&main_remote_ref)? {
                    Some(format!("origin/{}", main_branch))
                } else {
                    None
                }
            };
            (exists, preexisted, sp)
        };
        let worktree_path = worktrees_dir.join(&worktree_name);
        let worktree_create_start = std::time::Instant::now();
        let worktree_info = WorktreeManager::run_create_worktree(
            worktrees_dir,
            repo_path.clone(),
            worktree_path,
            branch_name.clone(),
            branch_exists,
            start_point,
        )
        .await?;
        info!(
            "[timing] run_create_worktree (git worktree add + worktree includes) took {}ms",
            worktree_create_start.elapsed().as_millis()
        );

        // Read tmux_session_name from the placeholder session
        let tmux_session_name = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            session.tmux_session_name.clone()
        };

        // Build a single positional prompt arg combining stack context (for
        // stacked sessions) and any user-provided initial prompt. Harnesses that
        // accept a positional prompt (Claude, Codex) take exactly one, so both
        // parts are merged; harnesses that don't (a bare shell) get neither.
        let accepts_prompt = AgentKind::from_program(&program).accepts_positional_prompt();
        let launch_cmd = {
            let mut prompt_parts: Vec<String> = Vec::new();
            if let Some(pb) = stack_parent_branch.as_deref()
                && accepts_prompt
            {
                prompt_parts.push(format!(
                    "This branch is stacked on `{pb}` (not main). \
                     When creating a PR for this session, use: \
                     gh pr create --base {pb}"
                ));
            }
            if let Some(ref user_prompt) = initial_prompt
                && accepts_prompt
            {
                prompt_parts.push(user_prompt.clone());
            }
            if prompt_parts.is_empty() {
                program.clone()
            } else {
                let combined = prompt_parts.join("\n\n");
                let escaped = shell_escape_single_quote(&combined);
                format!("{program} '{escaped}'")
            }
        };
        let launch_cmd = program_with_session_name(&launch_cmd, &title);
        let launch_cmd = self.maybe_wrap_nix_develop(&launch_cmd, &worktree_info.path);

        // Create tmux session in the worktree directory
        let tmux_start = std::time::Instant::now();
        self.tmux
            .create_session(&tmux_session_name, &worktree_info.path, Some(&launch_cmd))
            .await?;
        info!(
            "[timing] tmux create_session took {}ms",
            tmux_start.elapsed().as_millis()
        );

        // Update session to Running with the real worktree info
        let sid = *session_id;
        let wt_path = worktree_info.path.clone();
        let head = worktree_info.head.clone();
        // A fresh branch is created empty off its base, so HEAD *is* the fork
        // point. A checked-out branch sits on its tip, so record its genuine
        // fork point instead — else `merge-base(base, HEAD)` is HEAD and the
        // review diff comes up empty for a branch that is ahead of its target.
        let base_commit =
            crate::git::managed_base_commit(&wt_path, &head, Some(&main_branch), branch_preexisted)
                .await;
        // The branch this session forked from, recorded so the review diff can
        // resolve its base against that branch's *live* tip rather than the
        // frozen `base_commit`: a stack parent's branch, an explicit
        // `--base-branch`, or the project's main branch.
        let base_branch = stack_parent_branch
            .as_deref()
            .or(base_branch.as_deref())
            .unwrap_or(&main_branch)
            .to_string();
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.worktree_path = wt_path;
                    session.base_commit = Some(base_commit);
                    session.base_branch = Some(base_branch);
                    session.set_status(SessionStatus::Running);
                }
            })
            .await?;

        // Configure CC status bar (branch only, no PR yet)
        let status_bar = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            self.status_bar_info(session, &state)
        };
        self.tmux
            .configure_status_bar(&tmux_session_name, &status_bar)
            .await;

        info!(
            "Finalized session {} with tmux session {}",
            session_id, tmux_session_name
        );
        info!(
            "[timing] finalize_session total took {}ms",
            finalize_start.elapsed().as_millis()
        );
        Ok(*session_id)
    }

    /// Remove a session that is still in `Creating` state (e.g., on failure or startup cleanup).
    #[instrument(skip(self))]
    pub async fn remove_creating_session(&self, session_id: &SessionId) -> Result<()> {
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                state.remove_session(&sid);
            })
            .await?;
        info!("Removed creating session {}", session_id);
        Ok(())
    }

    /// Kill tmux sessions (main + shell) for a worktree session.
    pub(super) async fn kill_tmux_sessions(&self, tmux_name: &str, shell_tmux_name: Option<&str>) {
        if let Err(e) = self.tmux.kill_session(tmux_name).await {
            warn!("Failed to kill tmux session: {}", e);
        }
        if let Some(shell_name) = shell_tmux_name {
            let _ = self.tmux.kill_session(shell_name).await;
        }
    }

    /// Restart a session (kill tmux and recreate, optionally with --resume)
    #[instrument(skip(self))]
    pub async fn restart_session(&self, session_id: &SessionId) -> Result<()> {
        let (
            tmux_session_name,
            shell_tmux_name,
            worktree_path,
            title,
            program,
            hibernated,
            status_bar,
        ) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (
                session.tmux_session_name.clone(),
                session.shell_tmux_session_name.clone(),
                session.worktree_path.clone(),
                session.title.clone(),
                session.program.clone(),
                session.hibernated,
                self.status_bar_info(session, &state),
            )
        };

        self.kill_tmux_sessions(&tmux_session_name, shell_tmux_name.as_deref())
            .await;

        // Create a fresh tmux session, resuming the prior agent session if
        // configured, or unconditionally when this session was auto-hibernated
        // (resume is what makes hibernation non-destructive).
        let force_resume = self.config_store.read().resume_session || hibernated;
        let resume_program = resume_program_for(&program, force_resume);
        let resume_program = program_with_session_name(&resume_program, &title);
        let resume_program = self.maybe_wrap_nix_develop(&resume_program, &worktree_path);
        let create_result = self
            .tmux
            .create_session(&tmux_session_name, &worktree_path, Some(&resume_program))
            .await;

        if let Err(e) = create_result {
            // Tmux is dead but recreation failed — mark as Stopped so state is consistent
            let sid = *session_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(session) = state.get_session_mut(&sid) {
                        session.set_status(SessionStatus::Stopped);
                    }
                })
                .await;
            return Err(e);
        }

        // Configure status bar on the new session
        self.tmux
            .configure_status_bar(&tmux_session_name, &status_bar)
            .await;

        // Set status to Running and clear the hibernation marker — the pane has
        // been recreated (resumed above when it was hibernated).
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.set_status(SessionStatus::Running);
                    session.hibernated = false;
                }
            })
            .await?;

        info!("Restarted session {}", session_id);
        Ok(())
    }

    /// Restart a session's tmux pane without `--resume`, identified by tmux
    /// session name. Used when the process inside the pane exits (e.g. Claude
    /// had nothing to resume) so the user seamlessly gets a fresh conversation
    /// instead of being dropped back to the TUI.
    pub async fn restart_session_fresh_by_tmux_name(&self, tmux_name: &str) -> Result<()> {
        let (session_id, worktree_path, title, program, status_bar) = {
            let state = self.store.read().await;
            let session = state
                .sessions
                .values()
                .find(|s| s.tmux_session_name == tmux_name)
                .ok_or_else(|| SessionError::TmuxSessionNotFound(tmux_name.to_string()))?;
            (
                session.id,
                session.worktree_path.clone(),
                session.title.clone(),
                session.program.clone(),
                self.status_bar_info(session, &state),
            )
        };

        let _ = self.tmux.kill_session(tmux_name).await;

        let launch_cmd = program_with_session_name(&program, &title);
        let launch_cmd = self.maybe_wrap_nix_develop(&launch_cmd, &worktree_path);
        let create_result = self
            .tmux
            .create_session(tmux_name, &worktree_path, Some(&launch_cmd))
            .await;

        if let Err(e) = create_result {
            let sid = session_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(session) = state.get_session_mut(&sid) {
                        session.set_status(SessionStatus::Stopped);
                    }
                })
                .await;
            return Err(e);
        }

        self.tmux.configure_status_bar(tmux_name, &status_bar).await;

        let sid = session_id;
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.set_status(SessionStatus::Running);
                    // The pane is live again, so clear the hibernation marker to
                    // uphold the "live pane ⇒ not hibernated" invariant (matches
                    // restart_session and the attach/recreate wake path).
                    session.hibernated = false;
                }
            })
            .await?;

        info!(
            "Restarted session {} fresh (no --resume) via tmux name: {}",
            session_id, tmux_name
        );
        Ok(())
    }

    /// Kill a session (stop tmux, optionally remove worktree)
    #[instrument(skip(self))]
    pub async fn kill_session(&self, session_id: &SessionId, remove_worktree: bool) -> Result<()> {
        let session = {
            let state = self.store.read().await;
            state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?
                .clone()
        };

        self.kill_tmux_sessions(
            &session.tmux_session_name,
            session.shell_tmux_session_name.as_deref(),
        )
        .await;

        // Optionally remove worktree
        if remove_worktree {
            let repo_path = {
                let state = self.store.read().await;
                state
                    .get_project(&session.project_id)
                    .map(|p| p.repo_path.clone())
            };

            if let Some(repo_path) = repo_path
                && let Ok(backend) = GitBackend::open(&repo_path)
            {
                let worktree_manager =
                    WorktreeManager::new(backend, self.config_store.read().worktrees_dir()?);
                if let Err(e) = worktree_manager
                    .remove_worktree(&session.worktree_path, true)
                    .await
                {
                    warn!("Failed to remove worktree: {}", e);
                }
            }
        }

        // Update state
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.set_status(SessionStatus::Stopped);
                }
            })
            .await?;

        info!("Killed session {}", session_id);
        Ok(())
    }

    /// Hibernate a session: stop its tmux process to free memory while keeping
    /// the worktree, branch, and all metadata intact. Unlike [`kill_session`]
    /// this is a *policy* action driven by the idle-hibernation loop, so it
    /// marks the session `hibernated` — the wake path then resumes the agent
    /// conversation even when the global `resume_session` config is off.
    ///
    /// Guards against racing a concurrent manual restart: if the tmux session
    /// reappears after the kill (someone recreated it), the status update is
    /// skipped; and the final mutate only transitions a still-`Running` session
    /// so a restart that flipped it back to Running is not clobbered.
    #[instrument(skip(self))]
    pub async fn hibernate_session(&self, session_id: &SessionId) -> Result<()> {
        let (tmux_session_name, shell_tmux_name) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (
                session.tmux_session_name.clone(),
                session.shell_tmux_session_name.clone(),
            )
        };

        self.kill_tmux_sessions(&tmux_session_name, shell_tmux_name.as_deref())
            .await;

        // If a concurrent restart recreated the tmux session between our read
        // and the kill, don't mark it Stopped — that would leave a live pane
        // flagged hibernated.
        if self
            .tmux
            .session_exists(&tmux_session_name)
            .await
            .unwrap_or(false)
        {
            warn!(
                "Session {} tmux reappeared after hibernate kill; skipping status update",
                session_id
            );
            return Ok(());
        }

        let sid = *session_id;
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid)
                    && session.status == SessionStatus::Running
                {
                    session.set_status(SessionStatus::Stopped);
                    session.hibernated = true;
                }
            })
            .await?;

        info!("Hibernated session {}", session_id);
        Ok(())
    }

    /// Set a session's keep-alive flag (opt-out of auto-hibernation). Returns
    /// the value that was set, or [`SessionError::NotFound`] if the session no
    /// longer exists — so callers don't report success for a no-op (matches
    /// [`toggle_keep_alive`](Self::toggle_keep_alive)).
    pub async fn set_keep_alive(&self, session_id: &SessionId, keep_alive: bool) -> Result<bool> {
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                state.get_session_mut(&sid).map(|session| {
                    session.keep_alive = keep_alive;
                    session.keep_alive
                })
            })
            .await?
            .ok_or_else(|| SessionError::NotFound(sid).into())
    }

    /// Toggle a session's keep-alive flag, returning the new value. The flip is
    /// done inside a single mutate so concurrent toggles can't race.
    pub async fn toggle_keep_alive(&self, session_id: &SessionId) -> Result<bool> {
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                state.get_session_mut(&sid).map(|session| {
                    session.keep_alive = !session.keep_alive;
                    session.keep_alive
                })
            })
            .await?
            .ok_or_else(|| SessionError::NotFound(sid).into())
    }

    /// Delete a session (remove from state)
    #[instrument(skip(self))]
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        // First kill if active
        {
            let state = self.store.read().await;
            if let Some(session) = state.get_session(session_id)
                && session.status.is_active()
            {
                drop(state);
                self.kill_session(session_id, true).await?;
            }
        }

        // Remove from state, re-pointing stacked children onto the parent and
        // returning the durable PR-base edits — planned atomically with the
        // removal inside the same mutate, so no concurrent task can invalidate
        // the plan between read and remove.
        let sid = *session_id;
        let pr_retargets = self
            .store
            .mutate(move |state| state.remove_session_retargeting_children(&sid).1)
            .await?;

        // Durably retarget child PRs on GitHub (best-effort, non-fatal).
        Self::retarget_child_prs(pr_retargets).await;

        info!("Deleted session {}", session_id);
        Ok(())
    }

    /// Run the planned GitHub PR-base edits for a stack deletion. Best-effort:
    /// each `gh pr edit` failure is logged and skipped — the local metadata
    /// retarget already keeps the UI correct. Shared by the CLI and TUI delete
    /// paths.
    pub async fn retarget_child_prs(retargets: Vec<crate::config::PrBaseRetarget>) {
        for r in retargets {
            crate::git::retarget_pr_base(&r.repo_path, r.pr_number, &r.new_base_branch).await;
        }
    }
}

/// Insert `--permission-mode <mode>` and/or `--effort <level>` into a Claude
/// command string. Always uses long-form flags (never short flags like `-p`)
/// because short flags on the Claude CLI can have different meanings.
///
/// `"default"` mode is treated as a no-op — the Claude CLI uses its own
/// default when the flag is absent. Effort has no equivalent no-op value
/// (its levels are `high`/`medium`/`low`), so all values are passed through.
///
/// No-op when the program isn't Claude.
pub fn program_with_claude_flags(
    program: &str,
    mode: Option<&str>,
    effort: Option<&str>,
) -> String {
    if !AgentKind::from_program(program).is_claude() || (mode.is_none() && effort.is_none()) {
        return program.to_string();
    }

    let mut parts = program.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap();
    let rest = parts.next();

    let mut flags = Vec::new();
    if let Some(m) = mode
        && m != "default"
    {
        flags.push(format!("--permission-mode {m}"));
    }
    if let Some(e) = effort {
        flags.push(format!("--effort {e}"));
    }

    match (flags.is_empty(), rest) {
        (true, Some(r)) => format!("{cmd} {r}"),
        (true, None) => cmd.to_string(),
        (false, Some(r)) => format!("{cmd} {} {r}", flags.join(" ")),
        (false, None) => format!("{cmd} {}", flags.join(" ")),
    }
}

/// Inject `-n <session_title>` into a Claude command so the Claude Code
/// session is named to match the Claude Commander session.
///
/// For non-claude programs the command is returned unchanged.
pub(super) fn program_with_session_name(program: &str, session_title: &str) -> String {
    if !AgentKind::from_program(program).is_claude() || session_title.is_empty() {
        return program.to_string();
    }
    let escaped = shell_escape_single_quote(session_title);
    let mut parts = program.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap();
    match parts.next() {
        Some(rest) => format!("{cmd} -n '{escaped}' {rest}"),
        None => format!("{cmd} -n '{escaped}'"),
    }
}

pub(super) fn shell_escape_single_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Choose the launch command when recreating a session's tmux pane: the
/// harness's resume command when `force_resume` is set, otherwise the program
/// launched fresh. Resume syntax is harness-specific; an unrecognised program
/// has no resume mechanism, so it launches fresh regardless.
///
/// Callers combine two inputs into `force_resume`: the global `resume_session`
/// config and the per-session `hibernated` marker (an auto-hibernated session
/// must resume to be non-destructive, even when the global flag is off).
pub(super) fn resume_program_for(program: &str, force_resume: bool) -> String {
    if force_resume {
        AgentKind::from_program(program)
            .resume_command(program)
            .unwrap_or_else(|| program.to_string())
    } else {
        program.to_string()
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    // --- program_with_claude_flags ---

    #[test]
    fn claude_flags_effort_only() {
        assert_eq!(
            program_with_claude_flags("claude", None, Some("high")),
            "claude --effort high"
        );
    }

    #[test]
    fn claude_flags_mode_only() {
        assert_eq!(
            program_with_claude_flags("claude", Some("auto"), None),
            "claude --permission-mode auto"
        );
    }

    #[test]
    fn claude_flags_both() {
        assert_eq!(
            program_with_claude_flags("claude", Some("plan"), Some("low")),
            "claude --permission-mode plan --effort low"
        );
    }

    #[test]
    fn claude_flags_default_mode_is_noop() {
        assert_eq!(
            program_with_claude_flags("claude", Some("default"), None),
            "claude"
        );
    }

    #[test]
    fn claude_flags_preserves_existing_args() {
        assert_eq!(
            program_with_claude_flags("claude --resume", Some("auto"), Some("high")),
            "claude --permission-mode auto --effort high --resume"
        );
    }

    #[test]
    fn claude_flags_noop_for_non_claude() {
        assert_eq!(
            program_with_claude_flags("bash", Some("auto"), Some("high")),
            "bash"
        );
        // Codex has its own flag conventions — never inject Claude's flags.
        assert_eq!(
            program_with_claude_flags("codex", Some("auto"), Some("high")),
            "codex"
        );
    }

    #[test]
    fn claude_flags_noop_when_no_flags() {
        assert_eq!(
            program_with_claude_flags("claude --resume", None, None),
            "claude --resume"
        );
    }

    // --- program_with_session_name ---

    #[test]
    fn session_name_injected_for_bare_claude() {
        let cmd = program_with_session_name("claude", "my session");
        assert_eq!(cmd, "claude -n 'my session'");
    }

    #[test]
    fn session_name_injected_with_existing_args() {
        let cmd = program_with_session_name("claude --resume", "fix auth");
        assert_eq!(cmd, "claude -n 'fix auth' --resume");
    }

    #[test]
    fn session_name_skipped_for_non_claude() {
        let cmd = program_with_session_name("bash", "my session");
        assert_eq!(cmd, "bash");
        // Codex has no `-n` session-name flag — leave its command untouched.
        let codex = program_with_session_name("codex", "my session");
        assert_eq!(codex, "codex");
    }

    #[test]
    fn session_name_skipped_for_empty_title() {
        let cmd = program_with_session_name("claude", "");
        assert_eq!(cmd, "claude");
    }

    #[test]
    fn session_name_escapes_single_quotes() {
        let cmd = program_with_session_name("claude", "it's a test");
        assert_eq!(cmd, "claude -n 'it'\\''s a test'");
    }

    // --- resume_program_for ---

    #[test]
    fn resume_program_for_forces_resume_per_harness() {
        assert_eq!(resume_program_for("claude", true), "claude --resume");
        assert_eq!(resume_program_for("codex", true), "codex resume --last");
        // Flags on the base command survive the resume rewrite.
        assert_eq!(resume_program_for("claude -c", true), "claude -c --resume");
    }

    #[test]
    fn resume_program_for_unknown_harness_launches_fresh_even_when_forced() {
        // A bare shell has no resume mechanism, so forcing resume can't change it.
        assert_eq!(resume_program_for("bash", true), "bash");
    }

    #[test]
    fn resume_program_for_without_force_launches_fresh() {
        assert_eq!(resume_program_for("claude", false), "claude");
        assert_eq!(resume_program_for("codex", false), "codex");
    }

    #[test]
    fn session_name_with_prompt_arg() {
        // Simulates the shape produced when an initial prompt is appended
        let with_prompt = "claude 'Fix the auth bug'";
        let cmd = program_with_session_name(with_prompt, "my session");
        assert!(cmd.starts_with("claude -n 'my session' '"));
        assert!(cmd.contains("Fix the auth bug"));
    }
}
