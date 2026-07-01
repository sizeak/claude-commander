//! Commander API — unified service layer for CLI and TUI consumers.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::comment::{
    ApplyOutcome, Comment, CommentStatus, CommentStore, SendDecision, compose_markdown,
    decide_send, reanchor_comments,
};
use crate::config::{AppState, Config, ConfigStore, StateStore};
use crate::error::{Result, SessionError};
use crate::git::{
    FileDiff, GitBackend, compose_review_diff, diff_stat_summary, effective_pr_state,
    enrich_binary_sizes, parse_unified_diff, prefer_remote_branch, read_base_blob,
    read_worktree_file,
};
use crate::reviewed::ReviewedStore;
use crate::session::{
    AgentState, ProjectId, ScanResult, SessionId, SessionManager, WorktreeSession,
    program_is_claude, program_with_claude_flags,
};
use crate::telemetry::{ConfigSnapshot, EnvFingerprint, FrontendInfo, Telemetry};
use crate::tmux::{AgentStateDetector, StatusBarInfo, TmuxExecutor};
use crate::tui::theme::Theme;

/// High-level service that wraps `SessionManager`, state stores, and agent
/// detection into a single entry point. Both the CLI and TUI route through
/// this rather than wiring the pieces together independently.
///
/// Cheap to clone: every field is an `Arc` or itself `Arc`-backed, so a clone
/// is a shared handle to the same state — used to hand the service to
/// background tasks (e.g. the review-diff refresh).
#[derive(Clone)]
pub struct CommanderService {
    manager: SessionManager,
    store: Arc<StateStore>,
    config_store: Arc<ConfigStore>,
    comments: Arc<CommentStore>,
    reviewed: Arc<ReviewedStore>,
    telemetry: Telemetry,
}

impl CommanderService {
    /// Construct the service. `frontend` identifies the embedding application
    /// (binary/GUI name + version) for telemetry attribution and is required —
    /// [`FrontendInfo::new`] panics if it is not properly populated.
    pub fn new(
        config_store: Arc<ConfigStore>,
        store: Arc<StateStore>,
        frontend: FrontendInfo,
    ) -> Self {
        let manager = SessionManager::new(
            config_store.clone(),
            store.clone(),
            Theme::default().tmux_status_style(),
        );
        // Comments and reviewed marks live beside state.json under the same
        // data dir the `StateStore` resolved — *not* a freshly recomputed
        // `Config::data_dir()`. Routing through the store's path means a test
        // (or any caller) that injects a `TempDir`-backed `StateStore` keeps
        // these sibling stores off the real `~/.local/share`, preserving the
        // project's strict test-isolation rule.
        let data_dir = store.data_dir();
        let comments = Arc::new(CommentStore::new(data_dir.join("comments")));
        let reviewed = Arc::new(ReviewedStore::new(data_dir.join("reviewed")));
        let telemetry = init_telemetry(&config_store, &store, &frontend);
        Self {
            manager,
            store,
            config_store,
            comments,
            reviewed,
            telemetry,
        }
    }

    pub fn for_cli(
        config: crate::config::Config,
        frontend: FrontendInfo,
    ) -> std::result::Result<Self, crate::Error> {
        let config_store = Arc::new(ConfigStore::new(config)?);
        // A corrupt state file must propagate, not default to an empty
        // state: `list` would report no sessions and `create` would persist
        // a duplicate project alongside the unreadable original.
        let app_state = AppState::load()?;
        let store = Arc::new(StateStore::new(app_state)?);
        Ok(Self::new(config_store, store, frontend))
    }

    /// Shared telemetry handle. Frontends and library code call
    /// `service.telemetry().feature("…")` to record usage; a no-op when
    /// telemetry is disabled.
    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    pub fn session_manager(&self) -> &SessionManager {
        &self.manager
    }

    pub fn store(&self) -> &Arc<StateStore> {
        &self.store
    }

    // -- Config --

    /// Snapshot the current in-memory config.
    pub fn read_config(&self) -> Config {
        self.config_store.read().clone()
    }

    /// Overwrite the persisted config (updates mtime so the hot-reload watcher
    /// won't re-read our own write).
    pub fn update_config(&self, config: Config) -> Result<()> {
        self.config_store.mutate(|c| *c = config)
    }

    /// Reload config from disk if the file changed since the last read.
    pub fn reload_config(&self) -> Result<bool> {
        self.config_store.reload_if_changed()
    }

    /// Whether a pending config change requires an app restart to take effect.
    pub fn restart_required(&self) -> bool {
        self.config_store.restart_required()
    }

    // -- Projects --

    /// Register a git repository as a project.
    pub async fn add_project(&self, repo_path: PathBuf) -> Result<ProjectId> {
        self.telemetry.feature("project.add");
        self.manager.add_project(repo_path).await
    }

    /// Scan a directory for git repositories and register them as projects.
    pub async fn scan_directory(&self, dir: &Path) -> Result<ScanResult> {
        self.telemetry.feature("project.scan_directory");
        self.manager.scan_directory(dir).await
    }

    /// Clear the paused cascade state without merging.
    pub async fn cascade_abandon(&self) -> Result<()> {
        self.manager.cascade_abandon().await
    }

    /// Status-bar summary for a running session.
    pub fn status_bar_info(&self, session: &WorktreeSession, state: &AppState) -> StatusBarInfo {
        self.manager.status_bar_info(session, state)
    }

    // -- Queries --

    pub async fn list_sessions(&self, include_stopped: bool) -> Result<Vec<SessionInfo>> {
        let state = self.store.read().await;
        Ok(build_session_info_list(&state, include_stopped))
    }

    pub async fn find_session(&self, query: &str) -> Result<Option<SessionInfo>> {
        let state = self.store.read().await;
        Ok(find_session_info(&state, query))
    }

    /// Resolve a session by an *exact* title or full ID, surfacing ambiguity
    /// rather than picking arbitrarily. Used by destructive commands where a
    /// loose prefix match could act on the wrong session.
    pub async fn find_session_exact(
        &self,
        query: &str,
    ) -> Result<crate::cli::SessionLookup<SessionInfo>> {
        let state = self.store.read().await;
        Ok(match crate::cli::find_session_exact(&state, query) {
            crate::cli::SessionLookup::Found(session) => {
                let project_name = state
                    .projects
                    .get(&session.project_id)
                    .map(|p| p.name.as_str())
                    .unwrap_or("unknown");
                crate::cli::SessionLookup::Found(session_info_from_session(session, project_name))
            }
            crate::cli::SessionLookup::NotFound => crate::cli::SessionLookup::NotFound,
            crate::cli::SessionLookup::Ambiguous(n) => crate::cli::SessionLookup::Ambiguous(n),
        })
    }

    pub async fn get_session_detail(
        &self,
        query: &str,
        lines: Option<usize>,
    ) -> Result<Option<SessionDetail>> {
        let (found, project_name) = {
            let state = self.store.read().await;
            let Some(session) = crate::cli::find_session(&state, query) else {
                return Ok(None);
            };
            let pname = state
                .projects
                .get(&session.project_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            (session.clone(), pname)
        };

        let agent_state = if found.status.is_active() {
            let mut detector = AgentStateDetector::new(self.manager.tmux.clone(), Duration::ZERO);
            detector.detect(&found.tmux_session_name).await
        } else {
            AgentState::Unknown
        };

        let diff_stat = if found.worktree_path.exists() {
            // Resolve the base exactly as the review view does — prefer the PR
            // target branch, then the captured fork point, then HEAD — and pass
            // it through the merge-base so the stat counts only this branch's
            // changes (not changes the base accrued since the fork). Keeps the
            // CLI diffstat consistent with the review diff.
            let base = ReviewBase::of(&found).git_ref(&found.worktree_path).await;
            let target = crate::git::diff_target(&found.worktree_path, &base).await;
            diff_stat_summary(&found.worktree_path, &target).await
        } else {
            None
        };

        let pane_content = if found.status.is_active() && lines.is_some() {
            let n = lines.map(crate::cli::clamp_log_lines);
            capture_pane(&self.manager.tmux, &found.tmux_session_name, n).await?
        } else {
            None
        };

        Ok(Some(SessionDetail {
            info: session_info_from_session(&found, &project_name),
            agent_state,
            diff_stat,
            pane_content,
        }))
    }

    pub async fn get_pane_content(
        &self,
        query: &str,
        lines: Option<usize>,
    ) -> Result<Option<String>> {
        let state = self.store.read().await;
        let Some(session) = crate::cli::find_session(&state, query) else {
            return Ok(None);
        };
        let tmux_name = session.tmux_session_name.clone();
        drop(state);

        let n = lines.map(crate::cli::clamp_log_lines);
        capture_pane(&self.manager.tmux, &tmux_name, n).await
    }

    /// Resolve a session query (full UUID, ID prefix, or exact title) to its
    /// tmux session name, or `None` if nothing matches. Used by the server's
    /// WebSocket attach handler to spawn the tmux attach bridge for the right
    /// session, reusing the same `find_session` matching the CLI/HTTP API use.
    pub async fn resolve_tmux_session(&self, query: &str) -> Result<Option<String>> {
        let state = self.store.read().await;
        Ok(crate::cli::find_session(&state, query).map(|s| s.tmux_session_name.clone()))
    }

    pub async fn check_tmux(&self) -> Result<()> {
        self.manager.check_tmux().await
    }

    // -- Mutations --

    pub async fn create_session(&self, opts: CreateSessionOpts) -> Result<SessionId> {
        self.telemetry.feature("session.create");
        self.manager.check_tmux().await?;

        let base_program = opts
            .program
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| self.config_store.read().default_program.clone());

        validate_program_flags(&opts, &base_program)?;

        let program =
            program_with_claude_flags(&base_program, opts.mode.as_deref(), opts.effort.as_deref());

        let path = {
            let backend = GitBackend::discover(&opts.project_path)?;
            backend.path().to_path_buf()
        };

        let project_id = self.ensure_project(path).await?;

        let session_id = self
            .manager
            .prepare_session(&project_id, opts.title, Some(program), None)
            .await?;

        if let Some(section) = &opts.section {
            let section = section.clone();
            let sections = self.config_store.read().sections.clone();
            let now = chrono::Utc::now();
            self.store
                .mutate(move |state| {
                    if let Some(session) = state.sessions.get_mut(&session_id) {
                        crate::session::place_created_session(session, &section, &sections, now);
                    }
                })
                .await?;
        }

        let result = async {
            self.manager
                .link_stack_parent_by_branch(&session_id, opts.base_branch.as_deref())
                .await?;
            self.manager
                .finalize_session(&session_id, opts.initial_prompt, opts.base_branch)
                .await?;
            Ok::<(), crate::Error>(())
        }
        .await;

        if let Err(e) = result {
            let _ = self.manager.remove_creating_session(&session_id).await;
            return Err(e);
        }

        Ok(session_id)
    }

    pub async fn ensure_project(&self, path: PathBuf) -> Result<ProjectId> {
        let existing = {
            let state = self.store.read().await;
            state
                .projects
                .values()
                .find(|p| p.repo_path == path)
                .map(|p| p.id)
        };
        match existing {
            Some(id) => Ok(id),
            None => self.manager.add_project(path).await,
        }
    }

    pub async fn kill_session(&self, id: &SessionId) -> Result<()> {
        self.manager.kill_session(id, false).await
    }

    pub async fn restart_session(&self, id: &SessionId) -> Result<()> {
        self.telemetry.feature("session.restart");
        self.manager.restart_session(id).await
    }

    pub async fn delete_session(&self, id: &SessionId) -> Result<()> {
        self.telemetry.feature("session.delete");
        self.manager.delete_session(id).await
    }

    // -- Review / comments --

    /// Open the review diff for a session: compose the base→working-tree diff,
    /// parse it, and re-anchor the session's stored comments against it
    /// (persisting any status changes). Returns the parsed diff plus the
    /// re-anchored comments.
    pub async fn open_review(&self, session_id: &SessionId) -> Result<ReviewSnapshot> {
        self.telemetry.feature("review.open");
        let (worktree_path, review_base) = self.review_target(session_id).await?;
        let base = review_base.git_ref(&worktree_path).await;
        let raw = compose_review_diff(&worktree_path, &base).await?;
        self.snapshot_from_raw(session_id, &worktree_path, base, raw)
            .await
    }

    /// Re-compose the review diff and, when its content differs from
    /// `prev_hash`, return a fresh snapshot (re-anchoring comments and pruning
    /// reviewed marks against the new diff). Returns `None` when the working
    /// tree is unchanged, so the caller can skip the expensive parse/precompute
    /// and leave the open view untouched. Drives the review view's in-place
    /// refresh after the agent finishes a turn (or on manual request).
    pub async fn refresh_review_if_changed(
        &self,
        session_id: &SessionId,
        prev_hash: u64,
    ) -> Result<Option<ReviewSnapshot>> {
        let (worktree_path, review_base) = self.review_target(session_id).await?;
        let base = review_base.git_ref(&worktree_path).await;
        let raw = compose_review_diff(&worktree_path, &base).await?;
        if xxhash_rust::xxh3::xxh3_64(raw.as_bytes()) == prev_hash {
            return Ok(None);
        }
        Ok(Some(
            self.snapshot_from_raw(session_id, &worktree_path, base, raw)
                .await?,
        ))
    }

    /// Resolve the worktree path and review base for a session under a brief
    /// read lock. Shared by `open_review`, `refresh_review_if_changed`,
    /// `review_blob_source` and `fetch_diff_blob`.
    async fn review_target(&self, session_id: &SessionId) -> Result<(PathBuf, ReviewBase)> {
        let state = self.store.read().await;
        let session = state
            .sessions
            .get(session_id)
            .ok_or(SessionError::NotFound(*session_id))?;
        Ok((session.worktree_path.clone(), ReviewBase::of(session)))
    }

    /// Build a [`ReviewSnapshot`] from an already-composed raw unified diff:
    /// hash it for staleness detection, parse it, then re-anchor the session's
    /// comments and prune stale reviewed marks against the parsed diff
    /// (persisting any changes).
    async fn snapshot_from_raw(
        &self,
        session_id: &SessionId,
        worktree_path: &Path,
        base: String,
        raw: String,
    ) -> Result<ReviewSnapshot> {
        let content_hash = xxhash_rust::xxh3::xxh3_64(raw.as_bytes());
        let mut diff = parse_unified_diff(&raw);
        // Binary files carry metadata only; fill in the blob sizes the parser
        // can't know. Bytes are lazy-loaded via `fetch_diff_blob`.
        enrich_binary_sizes(&mut diff, worktree_path).await;

        let mut comments = self.comments.load(*session_id).await?;
        reanchor_comments(&mut comments, &diff);
        self.comments.save(*session_id, &comments).await?;

        // Reviewed marks pinned to a file's diff content: drop any whose file
        // changed or left the diff since they were set.
        let mut marks = self.reviewed.load(*session_id).await?;
        if crate::reviewed::prune_invalidated(&mut marks, &diff) {
            self.reviewed.save(*session_id, &marks).await?;
        }
        let reviewed = marks.into_iter().map(|m| m.file).collect();

        Ok(ReviewSnapshot {
            base,
            diff,
            comments,
            reviewed,
            content_hash,
        })
    }

    /// A session's worktree path and the resolved base git ref the review diff
    /// is computed against. These are the inputs a background image fetch needs
    /// to read blob bytes (via `crate::git::read_base_blob`/`read_worktree_file`)
    /// without holding the non-`Send` service handle across the task boundary.
    pub async fn review_blob_source(&self, session_id: &SessionId) -> Result<(PathBuf, String)> {
        let (worktree_path, review_base) = self.review_target(session_id).await?;
        let base = review_base.git_ref(&worktree_path).await;
        Ok((worktree_path, base))
    }

    /// Fetch the raw bytes of one side of a binary file in a session's review
    /// diff. This is the lazy-load half of the binary-diff seam: bytes are kept
    /// OUT of `ReviewSnapshot` and fetched on demand only when a consumer needs
    /// to render (or compare) the image.
    ///
    /// - `DiffSide::New` reads the working-tree file.
    /// - `DiffSide::Old` reads the blob at the review base (errors for an added
    ///   file, which has no base side).
    pub async fn fetch_diff_blob(
        &self,
        session_id: &SessionId,
        side: DiffSide,
        path: &str,
    ) -> Result<Vec<u8>> {
        let (worktree_path, review_base) = self.review_target(session_id).await?;
        match side {
            DiffSide::New => read_worktree_file(&worktree_path, path).await,
            DiffSide::Old => {
                let base = review_base.git_ref(&worktree_path).await;
                read_base_blob(&worktree_path, &base, path).await
            }
        }
    }

    /// Toggle the reviewed mark for one file of a session's review diff.
    /// The hash is computed from the `FileDiff` the caller is displaying, so
    /// the mark records exactly what the user saw (not a possibly-newer
    /// working tree). Returns the new reviewed state.
    pub async fn toggle_file_reviewed(
        &self,
        session_id: &SessionId,
        file: &FileDiff,
    ) -> Result<bool> {
        self.telemetry.feature("review.toggle_reviewed");
        let mut marks = self.reviewed.load(*session_id).await?;
        let now_reviewed = crate::reviewed::toggle(&mut marks, file);
        self.reviewed.save(*session_id, &marks).await?;
        Ok(now_reviewed)
    }

    /// Toggle a file's reviewed mark by display path: resolve the file in the
    /// **current** review diff and toggle against that. Keeps the wire API down
    /// to a path (no `FileDiff` echo for remote clients to cache) and makes it
    /// impossible to record a mark against a stale copy of the file — the hash
    /// always reflects the diff as it exists now. `FileNotInDiff` when the path
    /// isn't in the current diff.
    pub async fn toggle_file_reviewed_by_path(
        &self,
        session_id: &SessionId,
        display_path: &str,
    ) -> Result<bool> {
        let (worktree_path, review_base) = self.review_target(session_id).await?;
        let base = review_base.git_ref(&worktree_path).await;
        let raw = compose_review_diff(&worktree_path, &base).await?;
        let diff = parse_unified_diff(&raw);
        let file = diff
            .files
            .iter()
            .find(|f| f.display_path() == display_path)
            .ok_or_else(|| SessionError::FileNotInDiff(display_path.to_string()))?;
        self.toggle_file_reviewed(session_id, file).await
    }

    /// List a session's stored comments (without re-anchoring).
    pub async fn list_comments(&self, session_id: &SessionId) -> Result<Vec<Comment>> {
        self.comments.load(*session_id).await
    }

    /// Session ids that have at least one not-yet-applied comment, for the
    /// session-list pending-comment indicator.
    pub async fn sessions_with_pending_comments(
        &self,
    ) -> Result<std::collections::HashSet<SessionId>> {
        self.comments.sessions_with_pending().await
    }

    /// Stage a new comment; returns its id.
    pub async fn create_comment(&self, session_id: &SessionId, draft: NewComment) -> Result<Uuid> {
        self.telemetry.feature("review.comment.create");
        let ann = Comment::new(
            draft.file,
            draft.side,
            draft.line_range,
            draft.snippet,
            draft.comment,
        );
        let id = ann.id;
        self.comments.add(*session_id, ann).await?;
        Ok(id)
    }

    /// Delete a staged comment by id (no-op if absent).
    pub async fn delete_comment(&self, session_id: &SessionId, id: Uuid) -> Result<()> {
        self.telemetry.feature("review.comment.delete");
        self.comments.delete(*session_id, id).await
    }

    /// Apply a session's staged comments: re-anchor them, and if none are
    /// drifted, compose a markdown brief to a temp file and inject a pointer
    /// prompt into the session's tmux pane for the agent to act on.
    ///
    /// Delivery is gated on agent state: sent immediately when idle/working
    /// (Claude queues natively), held until a permission prompt clears, and
    /// deferred if the agent is stopped or never becomes ready. Applied
    /// comments are marked [`CommentStatus::Applied`].
    pub async fn apply_comments(&self, session_id: &SessionId) -> Result<ApplyOutcome> {
        self.telemetry.feature("review.apply_comments");
        let (worktree_path, review_base, title, tmux_name, is_active) = {
            let state = self.store.read().await;
            let s = state
                .sessions
                .get(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (
                s.worktree_path.clone(),
                ReviewBase::of(s),
                s.title.clone(),
                s.tmux_session_name.clone(),
                s.status.is_active(),
            )
        };

        // Re-anchor against a fresh diff so drift status is current.
        let base = review_base.git_ref(&worktree_path).await;
        let raw = compose_review_diff(&worktree_path, &base).await?;
        let parsed = parse_unified_diff(&raw);
        let mut comments = self.comments.load(*session_id).await?;
        reanchor_comments(&mut comments, &parsed);
        self.comments.save(*session_id, &comments).await?;

        // Only not-yet-applied comments participate.
        let staged: Vec<Comment> = comments
            .iter()
            .filter(|a| a.status != CommentStatus::Applied)
            .cloned()
            .collect();
        if staged.is_empty() {
            return Ok(ApplyOutcome::Nothing);
        }
        let drifted: Vec<Uuid> = staged
            .iter()
            .filter(|a| a.status == CommentStatus::Drifted)
            .map(|a| a.id)
            .collect();
        if !drifted.is_empty() {
            return Ok(ApplyOutcome::Blocked { drifted });
        }

        // Compose the brief to an absolute temp path outside the worktree.
        let path = write_apply_brief(*session_id, &compose_markdown(&title, &staged)).await?;
        let count = staged.len();

        if !is_active {
            return Ok(ApplyOutcome::Deferred { path, count });
        }

        // Gate delivery on agent state.
        let mut detector = AgentStateDetector::new(self.manager.tmux.clone(), Duration::ZERO);
        let ready = match decide_send(detector.detect(&tmux_name).await) {
            SendDecision::Now => true,
            SendDecision::HoldUntilClear => wait_until_ready(&mut detector, &tmux_name).await,
        };
        if !ready {
            return Ok(ApplyOutcome::Deferred { path, count });
        }

        // Inject the pointer prompt (literal text, then Enter to submit).
        let prompt = format!(
            "Review the comments in {} and address them.",
            path.display()
        );
        self.manager.tmux.send_keys(&tmux_name, &prompt).await?;
        self.manager.tmux.send_keys(&tmux_name, "Enter").await?;

        // Mark the delivered comments applied.
        for ann in comments
            .iter_mut()
            .filter(|a| a.status != CommentStatus::Applied)
        {
            ann.status = CommentStatus::Applied;
        }
        self.comments.save(*session_id, &comments).await?;

        Ok(ApplyOutcome::Applied { path, count })
    }
}

/// Write the apply brief to a stable absolute path in the system temp dir
/// (outside the worktree, so it's never committed). One file per session,
/// overwritten on re-apply.
async fn write_apply_brief(session_id: SessionId, markdown: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("cc-comments-{}.md", session_id.as_uuid()));
    tokio::fs::write(&path, markdown)
        .await
        .map_err(|e| crate::error::ConfigError::SaveFailed(e.to_string()))?;
    Ok(path)
}

/// Poll the agent state, returning `true` once it leaves `WaitingForInput`, or
/// `false` if it stays at a prompt past the bounded timeout.
async fn wait_until_ready(detector: &mut AgentStateDetector, tmux_name: &str) -> bool {
    const ATTEMPTS: u32 = 20;
    const INTERVAL: Duration = Duration::from_millis(250);
    for _ in 0..ATTEMPTS {
        if detector.detect(tmux_name).await != AgentState::WaitingForInput {
            return true;
        }
        tokio::time::sleep(INTERVAL).await;
    }
    false
}

/// The logical base a session's review diff is computed against.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReviewBase {
    /// A target *branch*, with the frozen fork-point SHA as a fallback. The
    /// branch resolves to its `origin/<branch>` remote-tracking ref when present
    /// (so the diff reflects the pushed upstream rather than a possibly-stale
    /// local branch), giving `merge-base(branch, HEAD)..working-tree` recomputed
    /// against the branch's *live* tip — the GitHub PR model. If the branch can
    /// no longer be resolved at all, the `fallback` SHA is used instead.
    Branch {
        name: String,
        fallback: Option<String>,
    },
    /// The fork-point commit captured at session creation (a fixed SHA). Only
    /// reached when no target branch was ever recorded.
    Commit(String),
    /// No base known; diff the working tree against `HEAD`.
    Head,
}

impl ReviewBase {
    /// Classify a session's base, preferring a *live branch* over a frozen SHA:
    ///
    /// 1. the PR's target branch (GitHub is authoritative once a PR exists);
    /// 2. else the branch captured at creation ([`WorktreeSession::base_branch`]
    ///    — a stack parent's branch, an explicit `--base-branch`, or main),
    ///    which keeps the diff correct as that branch advances;
    /// 3. else the frozen fork-point commit ([`WorktreeSession::base_commit`]),
    ///    used only when no branch was recorded;
    /// 4. else `HEAD`.
    ///
    /// In the branch cases the frozen `base_commit` rides along as a fallback,
    /// used by [`Self::git_ref`] only if the branch itself fails to resolve.
    fn of(session: &WorktreeSession) -> Self {
        let fallback = session.base_commit.clone();
        if let Some(branch) = session.pr_base_branch.clone() {
            ReviewBase::Branch {
                name: branch,
                fallback,
            }
        } else if let Some(branch) = session.base_branch.clone() {
            ReviewBase::Branch {
                name: branch,
                fallback,
            }
        } else if let Some(commit) = session.base_commit.clone() {
            ReviewBase::Commit(commit)
        } else {
            ReviewBase::Head
        }
    }

    /// The git commit-ish to diff against. A branch base prefers its
    /// remote-tracking ref; if neither the remote nor local branch resolves it
    /// drops to the frozen `fallback` SHA, then to the branch name verbatim. A
    /// commit SHA and `HEAD` are used verbatim.
    async fn git_ref(self, worktree: &Path) -> String {
        match self {
            ReviewBase::Branch { name, fallback } => {
                let candidate = prefer_remote_branch(worktree, &name).await;
                if crate::git::ref_resolves(worktree, &candidate).await {
                    candidate
                } else if let Some(sha) = fallback {
                    sha
                } else {
                    candidate
                }
            }
            ReviewBase::Commit(commit) => commit,
            ReviewBase::Head => "HEAD".to_string(),
        }
    }
}

/// Validate that the claude-only create flags (`--effort`, `--mode`,
/// `--initial-prompt`) aren't set for a non-claude program. [`CreateSessionOpts`]
/// itself is a plain wire type in `claude-commander-protocol`; this check lives
/// here because it needs core's `program_is_claude`.
pub fn validate_program_flags(opts: &CreateSessionOpts, resolved_program: &str) -> Result<()> {
    if !program_is_claude(resolved_program)
        && (opts.effort.is_some() || opts.mode.is_some() || opts.initial_prompt.is_some())
    {
        return Err(SessionError::InvalidProgram(format!(
            "--effort, --mode, and --initial-prompt are only supported \
             when the program is claude (got {:?})",
            resolved_program
        ))
        .into());
    }
    Ok(())
}

/// Build the telemetry handle for a freshly-constructed service: resolve the
/// install id, construct the handle from config, and emit the once-per-launch
/// `session_start` event. A no-op handle is returned when telemetry is disabled.
fn init_telemetry(
    config_store: &Arc<ConfigStore>,
    store: &Arc<StateStore>,
    frontend: &FrontendInfo,
) -> Telemetry {
    let config = config_store.read().clone();
    // Skip install-id generation (and its background persist spawn) entirely
    // when telemetry is off — keeps disabled/sync contexts (e.g. unit tests)
    // from needing a Tokio runtime.
    if !crate::telemetry::would_be_enabled(&config.telemetry) {
        return Telemetry::disabled();
    }
    let install_id = ensure_install_id(store);
    let telemetry = Telemetry::init(&config.telemetry, frontend, &install_id);
    if telemetry.is_active() {
        let env = EnvFingerprint::collect(Some(crate::tui::theme::ColorMode::detect().name()));
        let snapshot = ConfigSnapshot::from_config(&config, store.try_view_mode());
        telemetry.session_start(&env, &snapshot);
    }
    telemetry
}

/// Return the anonymous install id, generating one if none is stored yet.
///
/// The in-memory read is uncontended at startup, so it normally reflects disk:
/// when genuinely absent, the fresh id is persisted (via the flocked
/// `set_install_id_if_absent`) and reused on every future launch. In the rare
/// case the read missed because the lock was momentarily held — i.e. an id
/// already exists — the persist leaves that existing id untouched, so this one
/// session uses a throwaway id that won't match the persisted one. That's an
/// acceptable edge case; we never clobber an existing id.
fn ensure_install_id(store: &Arc<StateStore>) -> String {
    if let Some(id) = store.try_install_id() {
        return id;
    }
    let id = Uuid::new_v4().to_string();
    // Persist this session's id in the background (so construction stays sync),
    // but only when a runtime is present to host the task. The presence guard
    // lives in `AppState::set_install_id_if_absent`, so it isn't duplicated here.
    if tokio::runtime::Handle::try_current().is_ok() {
        let id_for_persist = id.clone();
        let store = store.clone();
        tokio::spawn(async move {
            let _ = store
                .mutate(move |s| {
                    s.set_install_id_if_absent(&id_for_persist);
                })
                .await;
        });
    }
    id
}

// -- Response types --
//
// The HTTP request/response DTOs live in `claude-commander-protocol`
// (`Serialize + Deserialize`, mobile-safe) and are re-exported here so
// `crate::api::{SessionInfo, ReviewSnapshot, ...}` paths keep working.
// Construction that needs core's domain model is done by
// `session_info_from_session` below.
pub use claude_commander_protocol::api::{
    CreateSessionOpts, DiffSide, NewComment, ReviewSnapshot, SessionDetail, SessionInfo,
    ToggleReviewed,
};

/// Build a [`SessionInfo`] wire DTO from core's `WorktreeSession` domain model.
/// (Was `SessionInfo::from_session`; relocated here because `SessionInfo` is now
/// a foreign type and this conversion needs core-only types.)
fn session_info_from_session(session: &WorktreeSession, project_name: &str) -> SessionInfo {
    SessionInfo {
        id: session.id.as_uuid().to_string(),
        session_id: session.id,
        title: session.title.clone(),
        branch: session.branch.clone(),
        status: session.status,
        program: session.program.clone(),
        project_id: session.project_id,
        project_name: project_name.to_string(),
        pr_number: session.pr_number,
        pr_url: session.pr_url.clone(),
        pr_state: effective_pr_state(session.pr_state, session.pr_merged),
        pr_draft: session.pr_draft,
        pr_labels: session.pr_labels.clone(),
        review_decision: session.review_decision,
        pr_reviewers: session.pr_reviewers.clone(),
        created_at: session.created_at,
    }
}

// -- Internal helpers --

fn build_session_info_list(state: &AppState, include_stopped: bool) -> Vec<SessionInfo> {
    let mut entries = Vec::new();
    for project in state.projects.values() {
        for session in project
            .worktrees
            .iter()
            .filter_map(|id| state.sessions.get(id))
            .filter(|s| include_stopped || s.status.is_active())
        {
            entries.push(session_info_from_session(session, &project.name));
        }
    }
    entries
}

fn find_session_info(state: &AppState, query: &str) -> Option<SessionInfo> {
    let session = crate::cli::find_session(state, query)?;
    let project_name = state
        .projects
        .get(&session.project_id)
        .map(|p| p.name.as_str())
        .unwrap_or("unknown");
    Some(session_info_from_session(session, project_name))
}

async fn capture_pane(
    executor: &TmuxExecutor,
    tmux_name: &str,
    lines: Option<usize>,
) -> Result<Option<String>> {
    if !executor.session_exists(tmux_name).await? {
        return Ok(None);
    }
    let mut args = vec!["capture-pane", "-t", tmux_name, "-p"];
    let lines_arg;
    if let Some(n) = lines {
        lines_arg = format!("-{}", n);
        args.extend_from_slice(&["-S", &lines_arg]);
    }
    Ok(Some(executor.execute(&args).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comment::CommentSide;
    use crate::git::PrState;
    use crate::session::{Project, ProjectId, SessionStatus, WorktreeSession};
    use std::path::PathBuf;

    fn make_project(name: &str) -> Project {
        Project::new(name, PathBuf::from("/tmp/repo"), "main")
    }

    fn make_session_for_project(title: &str, project_id: ProjectId) -> WorktreeSession {
        WorktreeSession::new(
            project_id,
            title,
            format!("branch-{}", title),
            PathBuf::from("/tmp/wt"),
            "claude",
        )
    }

    fn make_state_with_project(project: &Project, sessions: Vec<WorktreeSession>) -> AppState {
        let mut state = AppState::new();
        let mut proj = project.clone();
        for s in &sessions {
            proj.add_worktree(s.id);
        }
        state.projects.insert(proj.id, proj);
        for s in sessions {
            state.sessions.insert(s.id, s);
        }
        state
    }

    #[test]
    fn session_info_from_session_populates_fields() {
        let session = make_session_for_project("fix-bug", ProjectId::new());
        let info = session_info_from_session(&session, "my-project");

        assert_eq!(info.title, "fix-bug");
        assert_eq!(info.branch, "branch-fix-bug");
        assert_eq!(info.program, "claude");
        assert_eq!(info.project_name, "my-project");
        assert_eq!(info.session_id, session.id);
        assert!(uuid::Uuid::parse_str(&info.id).is_ok());
    }

    #[test]
    fn session_info_resolves_legacy_pr_merged() {
        let mut session = make_session_for_project("legacy", ProjectId::new());
        session.pr_number = Some(10);
        session.pr_state = None;
        session.pr_merged = true;

        let info = session_info_from_session(&session, "proj");
        assert_eq!(info.pr_state, PrState::Merged);
    }

    #[test]
    fn build_list_excludes_stopped_by_default() {
        let project = make_project("repo");
        let mut s1 = make_session_for_project("running", project.id);
        s1.set_status(SessionStatus::Running);
        let mut s2 = make_session_for_project("stopped", project.id);
        s2.set_status(SessionStatus::Stopped);

        let state = make_state_with_project(&project, vec![s1, s2]);
        let list = build_session_info_list(&state, false);

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "running");
    }

    #[test]
    fn build_list_includes_stopped_when_requested() {
        let project = make_project("repo");
        let mut s1 = make_session_for_project("running", project.id);
        s1.set_status(SessionStatus::Running);
        let mut s2 = make_session_for_project("stopped", project.id);
        s2.set_status(SessionStatus::Stopped);

        let state = make_state_with_project(&project, vec![s1, s2]);
        let list = build_session_info_list(&state, true);

        assert_eq!(list.len(), 2);
    }

    #[test]
    fn build_list_populates_project_name() {
        let project = make_project("my-repo");
        let s = make_session_for_project("task", project.id);
        let state = make_state_with_project(&project, vec![s]);
        let list = build_session_info_list(&state, false);

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].project_name, "my-repo");
    }

    #[test]
    fn find_session_info_by_title() {
        let project = make_project("repo");
        let s = make_session_for_project("fix-auth", project.id);
        let expected_id = s.id;
        let state = make_state_with_project(&project, vec![s]);

        let info = find_session_info(&state, "fix-auth").unwrap();
        assert_eq!(info.session_id, expected_id);
        assert_eq!(info.project_name, "repo");
    }

    #[test]
    fn find_session_info_returns_none_for_missing() {
        let state = AppState::new();
        assert!(find_session_info(&state, "nope").is_none());
    }

    #[test]
    fn session_detail_flattens_info_in_json() {
        let session = make_session_for_project("test", ProjectId::new());
        let detail = SessionDetail {
            info: session_info_from_session(&session, "proj"),
            agent_state: AgentState::Working,
            diff_stat: Some("3 files changed".to_string()),
            pane_content: None,
        };
        let json: serde_json::Value = serde_json::to_value(&detail).unwrap();
        assert_eq!(json["title"], "test");
        assert_eq!(json["agent_state"], "working");
        assert_eq!(json["diff_stat"], "3 files changed");
        assert!(json["pane_content"].is_null());
    }

    #[test]
    fn validate_rejects_non_claude_program_with_effort() {
        let opts = CreateSessionOpts {
            project_path: PathBuf::from("/tmp/repo"),
            title: "test".to_string(),
            program: Some("bash".to_string()),
            initial_prompt: None,
            effort: Some("high".to_string()),
            mode: None,
            base_branch: None,
            section: None,
        };
        let err = validate_program_flags(&opts, "bash").unwrap_err();
        assert!(err.to_string().contains("--effort"));
    }

    #[test]
    fn validate_rejects_non_claude_program_with_mode() {
        let opts = CreateSessionOpts {
            project_path: PathBuf::from("/tmp/repo"),
            title: "test".to_string(),
            program: Some("vim".to_string()),
            initial_prompt: None,
            effort: None,
            mode: Some("auto".to_string()),
            base_branch: None,
            section: None,
        };
        let err = validate_program_flags(&opts, "vim").unwrap_err();
        assert!(err.to_string().contains("--mode"));
    }

    #[test]
    fn validate_allows_claude_with_flags() {
        let opts = CreateSessionOpts {
            project_path: PathBuf::from("/tmp/repo"),
            title: "test".to_string(),
            program: Some("claude".to_string()),
            initial_prompt: Some("hello".to_string()),
            effort: Some("high".to_string()),
            mode: Some("auto".to_string()),
            base_branch: None,
            section: None,
        };
        validate_program_flags(&opts, "claude").unwrap();
    }

    #[test]
    fn review_base_prefers_live_branch_then_falls_back_to_commit_then_head() {
        let mut s = make_session_for_project("t", ProjectId::new());
        // Nothing recorded yet → HEAD.
        assert_eq!(ReviewBase::of(&s), ReviewBase::Head);
        // Only a frozen fork-point commit → use it as a last resort.
        s.base_commit = Some("abc123".to_string());
        assert_eq!(ReviewBase::of(&s), ReviewBase::Commit("abc123".to_string()));
        // The branch captured at creation wins over the frozen SHA, which rides
        // along as a fallback for when the branch can't be resolved.
        s.base_branch = Some("parent-feature".to_string());
        assert_eq!(
            ReviewBase::of(&s),
            ReviewBase::Branch {
                name: "parent-feature".to_string(),
                fallback: Some("abc123".to_string()),
            }
        );
        // The PR's target branch is authoritative once known, over both.
        s.pr_base_branch = Some("main".to_string());
        assert_eq!(
            ReviewBase::of(&s),
            ReviewBase::Branch {
                name: "main".to_string(),
                fallback: Some("abc123".to_string()),
            }
        );
    }

    #[test]
    fn validate_allows_non_claude_without_flags() {
        let opts = CreateSessionOpts {
            project_path: PathBuf::from("/tmp/repo"),
            title: "test".to_string(),
            program: Some("bash".to_string()),
            initial_prompt: None,
            effort: None,
            mode: None,
            base_branch: None,
            section: None,
        };
        validate_program_flags(&opts, "bash").unwrap();
    }

    /// A `CommanderService` built over `TempDir`-backed stores must root its
    /// comment/reviewed stores under that temp data dir — never the real
    /// `Config::data_dir()`. Writing a comment proves the on-disk path is the
    /// injected one (test-isolation regression).
    #[tokio::test]
    async fn comment_writes_stay_under_injected_data_dir() {
        use crate::config::storage::AppState as CoreState;
        use crate::config::{ConfigStore, StateStore};

        let dir = tempfile::TempDir::new().unwrap();
        // Telemetry is opt-out by default with a baked ingest token; disable it
        // so this test never posts events to the production OpenObserve instance.
        let mut config = Config::default();
        config.telemetry.enabled = false;
        let config_store = Arc::new(ConfigStore::with_path(
            config,
            dir.path().join("config.toml"),
        ));
        let store = Arc::new(StateStore::with_path(
            CoreState::default(),
            dir.path().join("state.json"),
        ));
        let frontend = FrontendInfo::new("test", "0.0.0");
        let service = CommanderService::new(config_store, store, frontend);

        // Write a comment through the public API.
        let session_id = SessionId::new();
        service
            .create_comment(
                &session_id,
                NewComment {
                    file: "a.rs".to_string(),
                    side: CommentSide::New,
                    line_range: (1, 1),
                    snippet: "let x = 1;".to_string(),
                    comment: "nit".to_string(),
                },
            )
            .await
            .unwrap();

        // The comment must have landed under the injected temp dir, and the
        // real data dir must be untouched by this write.
        let comments_dir = dir.path().join("comments");
        assert!(
            comments_dir.exists(),
            "comments should be written under the injected data dir"
        );
        if let Ok(real) = Config::data_dir() {
            assert_ne!(
                real,
                dir.path(),
                "temp data dir must differ from the real data dir"
            );
        }
    }
}
