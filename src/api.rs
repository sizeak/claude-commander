//! Commander API — unified service layer for CLI and TUI consumers.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;

use uuid::Uuid;

use crate::comment::{
    ApplyOutcome, Comment, CommentSide, CommentStatus, CommentStore, CommentTarget,
    PrReviewOutcome, PrVerdict, SendDecision, compose_markdown, compose_pr_review, decide_send,
    pending_for_target, reanchor_comments,
};
use crate::config::{AppState, Config, ConfigStore, StateStore};
use crate::error::{Result, SessionError};
use crate::git::{
    GitBackend, ParsedDiff, PrState, ReviewDecision, compose_review_diff, diff_stat_summary,
    effective_pr_state, parse_unified_diff, prefer_remote_branch,
};
use crate::session::{
    AgentState, ProjectId, ScanResult, SessionId, SessionManager, SessionStatus, WorktreeSession,
    program_is_claude, program_with_claude_flags,
};
use crate::tmux::{AgentStateDetector, StatusBarInfo, TmuxExecutor};
use crate::tui::theme::Theme;

/// High-level service that wraps `SessionManager`, state stores, and agent
/// detection into a single entry point. Both the CLI and TUI route through
/// this rather than wiring the pieces together independently.
pub struct CommanderService {
    manager: SessionManager,
    store: Arc<StateStore>,
    config_store: Arc<ConfigStore>,
    comments: Arc<CommentStore>,
}

impl CommanderService {
    pub fn new(config_store: Arc<ConfigStore>, store: Arc<StateStore>) -> Self {
        let manager = SessionManager::new(
            config_store.clone(),
            store.clone(),
            Theme::default().tmux_status_style(),
        );
        // Comments live beside state.json under the data dir. `data_dir()`
        // only fails when no home directory can be resolved (effectively never
        // on supported platforms); fall back to a relative dir to keep `new`
        // infallible, mirroring `for_cli`'s tolerant state load.
        let comments = Arc::new(CommentStore::new(
            Config::data_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("comments"),
        ));
        Self {
            manager,
            store,
            config_store,
            comments,
        }
    }

    pub fn for_cli(config: crate::config::Config) -> std::result::Result<Self, crate::Error> {
        let config_store = Arc::new(ConfigStore::new(config)?);
        let app_state = AppState::load().unwrap_or_else(|_| AppState::new());
        let store = Arc::new(StateStore::new(app_state)?);
        Ok(Self::new(config_store, store))
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
        self.manager.add_project(repo_path).await
    }

    /// Scan a directory for git repositories and register them as projects.
    pub async fn scan_directory(&self, dir: &Path) -> Result<ScanResult> {
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
            let diff_base = found.base_commit.as_deref().unwrap_or("HEAD");
            diff_stat_summary(&found.worktree_path, diff_base).await
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
            info: SessionInfo::from_session(&found, &project_name),
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

    pub async fn check_tmux(&self) -> Result<()> {
        self.manager.check_tmux().await
    }

    // -- Mutations --

    pub async fn create_session(&self, opts: CreateSessionOpts) -> Result<SessionId> {
        self.manager.check_tmux().await?;

        let base_program = opts
            .program
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| self.config_store.read().default_program.clone());

        opts.validate_program_flags(&base_program)?;

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
        self.manager.restart_session(id).await
    }

    pub async fn delete_session(&self, id: &SessionId) -> Result<()> {
        self.manager.delete_session(id).await
    }

    // -- Review / comments --

    /// Open the review diff for a session: compose the base→working-tree diff,
    /// parse it, and re-anchor the session's stored comments against it
    /// (persisting any status changes). Returns the parsed diff plus the
    /// re-anchored comments.
    pub async fn open_review(&self, session_id: &SessionId) -> Result<ReviewSnapshot> {
        let (worktree_path, review_base) = {
            let state = self.store.read().await;
            let session = state
                .sessions
                .get(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (session.worktree_path.clone(), ReviewBase::of(session))
        };

        let base = review_base.git_ref(&worktree_path).await;
        let raw = compose_review_diff(&worktree_path, &base).await?;
        let diff = parse_unified_diff(&raw);

        let mut comments = self.comments.load(*session_id)?;
        reanchor_comments(&mut comments, &diff);
        self.comments.save(*session_id, &comments)?;

        Ok(ReviewSnapshot {
            base,
            diff,
            comments,
        })
    }

    /// List a session's stored comments (without re-anchoring).
    pub async fn list_comments(&self, session_id: &SessionId) -> Result<Vec<Comment>> {
        self.comments.load(*session_id)
    }

    /// Session ids that have at least one not-yet-applied comment, for the
    /// session-list pending-comment indicator.
    pub async fn sessions_with_pending_comments(
        &self,
    ) -> Result<std::collections::HashSet<SessionId>> {
        self.comments.sessions_with_pending()
    }

    /// Stage a new comment; returns its id.
    pub async fn create_comment(&self, session_id: &SessionId, draft: NewComment) -> Result<Uuid> {
        let ann = Comment::new(
            draft.file,
            draft.side,
            draft.line_range,
            draft.snippet,
            draft.comment,
        )
        .with_target(draft.target);
        let id = ann.id;
        self.comments.add(*session_id, ann)?;
        Ok(id)
    }

    /// Delete a staged comment by id (no-op if absent).
    pub async fn delete_comment(&self, session_id: &SessionId, id: Uuid) -> Result<()> {
        self.comments.delete(*session_id, id)
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
        let mut comments = self.comments.load(*session_id)?;
        reanchor_comments(&mut comments, &parsed);
        self.comments.save(*session_id, &comments)?;

        // Only not-yet-applied, agent-targeted comments participate; PR comments
        // are delivered separately via `submit_pr_review`.
        let staged = pending_for_target(&comments, CommentTarget::Agent);
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
        let path = write_apply_brief(*session_id, &compose_markdown(&title, &staged))?;
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

        // Mark the delivered (agent-targeted) comments applied.
        for ann in comments
            .iter_mut()
            .filter(|a| a.status != CommentStatus::Applied && a.target == CommentTarget::Agent)
        {
            ann.status = CommentStatus::Applied;
        }
        self.comments.save(*session_id, &comments)?;

        Ok(ApplyOutcome::Applied { path, count })
    }

    /// Submit a session's PR-targeted comments as a single GitHub review.
    ///
    /// Re-anchors first so drift status is current; if any PR comment is
    /// drifted the submission is blocked. The comments are composed into a
    /// reviews-API payload and posted via `gh api .../pulls/{n}/reviews` from
    /// the session's worktree. On success the submitted comments are marked
    /// [`CommentStatus::Applied`]. Returns [`PrReviewOutcome::NoPr`] when the
    /// session has no associated pull request.
    pub async fn submit_pr_review(
        &self,
        session_id: &SessionId,
        verdict: PrVerdict,
        summary: &str,
    ) -> Result<PrReviewOutcome> {
        let (worktree_path, review_base, pr_number) = {
            let state = self.store.read().await;
            let s = state
                .sessions
                .get(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (s.worktree_path.clone(), ReviewBase::of(s), s.pr_number)
        };

        let Some(pr_number) = pr_number else {
            return Ok(PrReviewOutcome::NoPr);
        };

        // Re-anchor against a fresh diff so drift status is current.
        let base = review_base.git_ref(&worktree_path).await;
        let raw = compose_review_diff(&worktree_path, &base).await?;
        let parsed = parse_unified_diff(&raw);
        let mut comments = self.comments.load(*session_id)?;
        reanchor_comments(&mut comments, &parsed);
        self.comments.save(*session_id, &comments)?;

        // Only not-yet-applied, PR-targeted comments participate.
        let staged = pending_for_target(&comments, CommentTarget::Pr);
        if staged.is_empty() {
            return Ok(PrReviewOutcome::Nothing);
        }
        let drifted: Vec<Uuid> = staged
            .iter()
            .filter(|a| a.status == CommentStatus::Drifted)
            .map(|a| a.id)
            .collect();
        if !drifted.is_empty() {
            return Ok(PrReviewOutcome::Blocked { drifted });
        }

        // Post the review via `gh api`, letting gh resolve {owner}/{repo} from
        // the worktree's remote.
        let payload = compose_pr_review(verdict, summary, &staged);
        submit_review_via_gh(&worktree_path, pr_number, &payload).await?;

        // Mark the submitted comments applied.
        for ann in comments
            .iter_mut()
            .filter(|a| a.status != CommentStatus::Applied && a.target == CommentTarget::Pr)
        {
            ann.status = CommentStatus::Applied;
        }
        self.comments.save(*session_id, &comments)?;

        Ok(PrReviewOutcome::Submitted {
            count: staged.len(),
        })
    }
}

/// Post a composed review payload to a PR via `gh api`. `gh` resolves the
/// `{owner}`/`{repo}` placeholders from the worktree's git remote, so no repo
/// slug is needed. The JSON body is fed on stdin.
async fn submit_review_via_gh(
    worktree_path: &std::path::Path,
    pr_number: u32,
    payload: &serde_json::Value,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let body = serde_json::to_string(payload)
        .map_err(|e| crate::error::GitError::OperationFailed(e.to_string()))?;
    let mut child = tokio::process::Command::new("gh")
        .args([
            "api",
            "--method",
            "POST",
            &format!("repos/{{owner}}/{{repo}}/pulls/{pr_number}/reviews"),
            "--input",
            "-",
        ])
        .current_dir(worktree_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            crate::error::GitError::OperationFailed(format!("gh api spawn failed: {e}"))
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| crate::error::GitError::OperationFailed(e.to_string()))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| crate::error::GitError::OperationFailed(e.to_string()))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| crate::error::GitError::OperationFailed(e.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::error::GitError::OperationFailed(format!(
            "gh api PR review failed: {}",
            stderr.trim()
        ))
        .into());
    }
    Ok(())
}

/// Write the apply brief to a stable absolute path in the system temp dir
/// (outside the worktree, so it's never committed). One file per session,
/// overwritten on re-apply.
fn write_apply_brief(session_id: SessionId, markdown: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("cc-comments-{}.md", session_id.as_uuid()));
    std::fs::write(&path, markdown)
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
    /// The PR's target branch. Resolved to its `origin/<branch>` remote-tracking
    /// ref when present, so the diff reflects the pushed upstream rather than a
    /// possibly-stale local branch; falls back to the local branch otherwise.
    Branch(String),
    /// The fork-point commit captured at session creation (a fixed SHA).
    Commit(String),
    /// No base known; diff the working tree against `HEAD`.
    Head,
}

impl ReviewBase {
    /// Classify a session's base: the PR target branch if known, else the
    /// fork-point commit captured at creation, else `HEAD`.
    fn of(session: &WorktreeSession) -> Self {
        if let Some(branch) = session.pr_base_branch.clone() {
            ReviewBase::Branch(branch)
        } else if let Some(commit) = session.base_commit.clone() {
            ReviewBase::Commit(commit)
        } else {
            ReviewBase::Head
        }
    }

    /// The git commit-ish to diff against. Only a branch base prefers its
    /// remote-tracking ref; a commit SHA and `HEAD` are used verbatim.
    async fn git_ref(self, worktree: &Path) -> String {
        match self {
            ReviewBase::Branch(branch) => prefer_remote_branch(worktree, &branch).await,
            ReviewBase::Commit(commit) => commit,
            ReviewBase::Head => "HEAD".to_string(),
        }
    }
}

pub struct CreateSessionOpts {
    pub project_path: PathBuf,
    pub title: String,
    pub program: Option<String>,
    pub initial_prompt: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub base_branch: Option<String>,
    pub section: Option<String>,
}

impl CreateSessionOpts {
    pub fn validate_program_flags(&self, resolved_program: &str) -> Result<()> {
        if !program_is_claude(resolved_program)
            && (self.effort.is_some() || self.mode.is_some() || self.initial_prompt.is_some())
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
}

// -- Response types --

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub session_id: SessionId,
    pub title: String,
    pub branch: String,
    pub status: SessionStatus,
    pub program: String,
    pub project_id: ProjectId,
    pub project_name: String,
    pub pr_number: Option<u32>,
    pub pr_url: Option<String>,
    pub pr_state: PrState,
    pub pr_draft: bool,
    pub pr_labels: Vec<String>,
    pub review_decision: Option<ReviewDecision>,
    pub pr_reviewers: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl SessionInfo {
    pub fn from_session(session: &WorktreeSession, project_name: &str) -> Self {
        Self {
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
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub info: SessionInfo,
    pub agent_state: AgentState,
    pub diff_stat: Option<String>,
    pub pane_content: Option<String>,
}

/// Request to stage a new comment on a session's review diff.
#[derive(Debug, Clone)]
pub struct NewComment {
    pub file: String,
    pub side: CommentSide,
    pub line_range: (usize, usize),
    pub snippet: String,
    pub comment: String,
    /// Where the comment is delivered when applied (agent vs PR).
    pub target: CommentTarget,
}

/// Result of opening the review view: the parsed diff plus the session's
/// (re-anchored) comments and the base they were computed against.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewSnapshot {
    pub base: String,
    pub diff: ParsedDiff,
    pub comments: Vec<Comment>,
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
            entries.push(SessionInfo::from_session(session, &project.name));
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
    Some(SessionInfo::from_session(session, project_name))
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
    use crate::session::{Project, ProjectId, WorktreeSession};
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
        let info = SessionInfo::from_session(&session, "my-project");

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

        let info = SessionInfo::from_session(&session, "proj");
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
            info: SessionInfo::from_session(&session, "proj"),
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
        let err = opts.validate_program_flags("bash").unwrap_err();
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
        let err = opts.validate_program_flags("vim").unwrap_err();
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
        opts.validate_program_flags("claude").unwrap();
    }

    #[test]
    fn review_base_classifies_pr_base_then_fork_then_head() {
        let mut s = make_session_for_project("t", ProjectId::new());
        // No PR base or fork point recorded yet → HEAD.
        assert_eq!(ReviewBase::of(&s), ReviewBase::Head);
        // Fork-point commit captured at creation.
        s.base_commit = Some("abc123".to_string());
        assert_eq!(ReviewBase::of(&s), ReviewBase::Commit("abc123".to_string()));
        // PR's target branch takes precedence once known.
        s.pr_base_branch = Some("main".to_string());
        assert_eq!(ReviewBase::of(&s), ReviewBase::Branch("main".to_string()));
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
        opts.validate_program_flags("bash").unwrap();
    }
}
