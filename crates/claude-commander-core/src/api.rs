//! Commander API — unified service layer for CLI and TUI consumers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::StreamExt;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::agent::AgentKind;
use crate::comment::{
    ApplyOutcome, Comment, CommentStatus, CommentStore, SendDecision, compose_markdown,
    decide_send, reanchor_comments,
};
use crate::config::{AppState, Config, ConfigStore, ProgramEntry, StateStore};
use crate::error::{Result, SessionError};
use crate::git::{
    FileDiff, GitBackend, PrCheckResult, compose_review_diff, compute_branch_diff,
    diff_stat_summary, effective_pr_state, enrich_binary_sizes, is_gh_available,
    parse_unified_diff, prefer_remote_branch, read_base_blob, read_worktree_file,
};
use crate::reviewed::ReviewedStore;
use crate::session::{
    AgentState, CascadeOutcome, ProjectId, ScanResult, SessionId, SessionManager, SessionStatus,
    WorktreeSession, apply_assignment, clear_override_and_reassign, program_with_agent_flags,
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
    /// Bounded in-memory ledger of recent cascade / push-stack operations,
    /// surfaced through [`Self::workspace_snapshot`]. Capped at
    /// [`OPERATION_LEDGER_CAP`]; oldest entries are evicted.
    operations: Arc<std::sync::Mutex<std::collections::VecDeque<OperationStatus>>>,
    /// Monotonic id source for ledger entries (stable for the process lifetime).
    next_op_id: Arc<std::sync::atomic::AtomicU64>,
    /// Cached `gh --version` availability. Computed once (fork/exec is not free)
    /// and reused for every `workspace_snapshot`.
    gh_available: Arc<tokio::sync::OnceCell<bool>>,
    /// Shared agent-state detector with a short TTL cache, reused across
    /// non-`fresh` [`Self::agent_states`] calls so repeated polls don't
    /// re-capture every pane. A `fresh` call bypasses it with a zero-TTL
    /// detector instead.
    agent_detector: Arc<tokio::sync::Mutex<AgentStateDetector>>,
    /// Latest bulk agent-state snapshot, maintained by the background poll loop
    /// ([`Self::spawn_background_tasks`]). Once [`Self::agent_states_primed`] is
    /// set, non-`fresh` [`Self::agent_states`] reads serve this cache rather than
    /// re-detecting on demand, so every frontend (and a remote client polling the
    /// route) sees the loop's liveness.
    agent_states_cache: Arc<tokio::sync::RwLock<AgentStatesSnapshot>>,
    /// Whether the poll loop has populated [`Self::agent_states_cache`] at least
    /// once. Until then (and when no loop runs — CLI, tests) `agent_states`
    /// falls back to on-demand detection.
    agent_states_primed: Arc<std::sync::atomic::AtomicBool>,
    /// Most recent per-project background-pull status, maintained by the pull
    /// loop and surfaced in [`WorkspaceSnapshot::project_pull`].
    pull_status: Arc<std::sync::Mutex<BTreeMap<ProjectId, PullStatus>>>,
    /// Last PR-status fan-out time, for debouncing manual refresh bursts.
    last_pr_check: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    /// Signals the PR-status loop to run an immediate check (manual refresh).
    pr_refresh: Arc<tokio::sync::Notify>,
    /// Idempotency guard for [`Self::spawn_background_tasks`]: the loops spawn
    /// once per service, even if both a local TUI and an embedded caller ask.
    background_started: Arc<std::sync::atomic::AtomicBool>,
    /// Short-TTL cache of the last `tmux -V` probe result, so per-client
    /// [`Self::workspace_snapshot`] polling (~2s cadence) doesn't fork a
    /// subprocess on every poll. See [`Self::cached_tmux_ok`].
    tmux_ok_cache: Arc<std::sync::Mutex<Option<(std::time::Instant, bool)>>>,
}

/// Max entries kept in the operation ledger before the oldest are evicted.
const OPERATION_LEDGER_CAP: usize = 32;

/// TTL for the shared agent-state detector cache used by non-`fresh`
/// `agent_states` polls.
const AGENT_STATE_CACHE_TTL: Duration = Duration::from_millis(1000);

/// TTL for the [`CommanderService::workspace_snapshot`] tmux-availability cache.
/// Long enough that a 2s client poll reuses the last probe rather than forking
/// `tmux -V` every time, short enough that tmux coming up/going down surfaces
/// within a few seconds.
const TMUX_OK_CACHE_TTL: Duration = Duration::from_secs(5);

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
        // NB: the idle-hibernation loop is NOT started here. `new` is shared by
        // one-shot CLI commands (via `for_cli`), and a tokio runtime is always
        // present under `#[tokio::main]`, so starting it here would let any CLI
        // command trigger a hibernation pass. Long-lived frontends call
        // `start_hibernation_loop` explicitly after construction instead.
        let agent_detector = Arc::new(tokio::sync::Mutex::new(AgentStateDetector::new(
            manager.tmux.clone(),
            AGENT_STATE_CACHE_TTL,
        )));
        Self {
            manager,
            store,
            config_store,
            comments,
            reviewed,
            telemetry,
            operations: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            next_op_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            gh_available: Arc::new(tokio::sync::OnceCell::new()),
            agent_detector,
            agent_states_cache: Arc::new(tokio::sync::RwLock::new(AgentStatesSnapshot {
                states: BTreeMap::new(),
                commander_running: false,
            })),
            agent_states_primed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pull_status: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            last_pr_check: Arc::new(std::sync::Mutex::new(None)),
            pr_refresh: Arc::new(tokio::sync::Notify::new()),
            background_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tmux_ok_cache: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Start the background idle-hibernation policy loop. Long-lived frontends
    /// (the TUI) call this once after construction; one-shot CLI paths do not,
    /// so a CLI command can never trigger a hibernation pass as a side effect.
    /// No-op unless `hibernate_enabled` is set, the check interval is non-zero,
    /// and a tokio runtime is present.
    pub fn start_hibernation_loop(&self) {
        self.manager.spawn_hibernation_loop(self.telemetry.clone());
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

    /// Replace the configured program list (the new-session picker options) and
    /// persist it. Exposed as its own method — and, for remotes, its own HTTP
    /// endpoint — so the picker list can be edited without opening up the general
    /// config-patch surface. An empty list is allowed (the picker then falls back
    /// to a synthesized `claude` entry).
    pub fn set_programs(&self, programs: Vec<ProgramEntry>) -> Result<()> {
        self.telemetry.feature("programs.set");
        self.config_store.mutate(|c| c.programs = programs)
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
            detector
                .detect(
                    AgentKind::from_program(&found.program),
                    &found.tmux_session_name,
                )
                .await
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

    /// Resolve a session query to its **shell** pane's tmux session name,
    /// creating the paired shell session on demand (the `Ctrl+\` partner). The
    /// shell counterpart of [`Self::resolve_tmux_session`], mirroring
    /// [`crate::backend::LocalBackend::attach`]'s `AttachKind::Shell` arm so the
    /// server's WebSocket attach and the local backend resolve the shell pane
    /// through the same [`ensure_shell_session`](crate::session::SessionManager::ensure_shell_session).
    /// `None` when the query matches no session.
    pub async fn resolve_shell_tmux_session(&self, query: &str) -> Result<Option<String>> {
        let session_id = {
            let state = self.store.read().await;
            crate::cli::find_session(&state, query).map(|s| s.id)
        };
        match session_id {
            Some(id) => Ok(Some(self.manager.ensure_shell_session(&id).await?)),
            None => Ok(None),
        }
    }

    /// Resolve a session query to the tmux session name for the requested pane,
    /// **reviving** a dead tmux session and **stamping** `last_attached_at` — the
    /// exact preparation [`crate::backend::LocalBackend::attach`] performs before
    /// spawning its bridge. The server's WebSocket attach handler routes through
    /// this so a remote attach reaches parity with a local one: a session whose
    /// tmux died is recreated (agent resumed, status bar reconfigured) rather than
    /// failing with a raw error, and every attach updates MRU ordering.
    /// `None` when the query matches no session.
    pub async fn resolve_attach_session(
        &self,
        query: &str,
        kind: crate::backend::AttachKind,
    ) -> Result<Option<String>> {
        let session_id = {
            let state = self.store.read().await;
            crate::cli::find_session(&state, query).map(|s| s.id)
        };
        let Some(id) = session_id else {
            return Ok(None);
        };
        let tmux_name = match kind {
            crate::backend::AttachKind::Agent => self.manager.ensure_attachable(&id).await?,
            crate::backend::AttachKind::Shell => self.manager.ensure_shell_session(&id).await?,
        };
        self.mark_attached(&id).await?;
        Ok(Some(tmux_name))
    }

    /// Resolve a session by **tmux session name** (primary or shell pair) and
    /// prepare its agent pane for attach, exactly as
    /// [`Self::resolve_attach_session`] does for a title/id query: a dead tmux
    /// session is revived (agent resumed, status bar reconfigured) and
    /// `last_attached_at` is stamped for MRU ordering. Returns the primary
    /// tmux name to attach or switch to. Used by the in-session Ctrl+Space
    /// switcher, whose picker knows sessions only by tmux name.
    pub async fn ensure_attachable_by_tmux_name(&self, tmux_name: &str) -> Result<String> {
        let session_id = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .find(|s| s.matches_tmux_name(tmux_name))
                .map(|s| s.id)
        };
        let id =
            session_id.ok_or_else(|| SessionError::TmuxSessionNotFound(tmux_name.to_string()))?;
        let name = self.manager.ensure_attachable(&id).await?;
        self.mark_attached(&id).await?;
        Ok(name)
    }

    /// Wrap this service as the [`crate::tmux::SwitcherRevive`] hook the
    /// attach loop invokes before `tmux switch-client`, so the in-session
    /// switcher revives a dead pick the same way the tree-view attach path
    /// does (via [`Self::ensure_attachable_by_tmux_name`]).
    pub fn switcher_revive_hook(&self) -> crate::tmux::SwitcherRevive {
        let service = self.clone();
        Arc::new(move |name: String| {
            let service = service.clone();
            Box::pin(async move { service.ensure_attachable_by_tmux_name(&name).await })
        })
    }

    /// Store a pasted image for a remote session and inject its file path into
    /// the session's Claude pane.
    ///
    /// The desktop TUI captures the *local* clipboard image on Ctrl+V during a
    /// remote attach and uploads the bytes here (via the server's
    /// `POST /api/sessions/{id}/paste-image` route). We validate + write them to
    /// a pruned temp file under the OS temp dir, then `send-keys -l` the absolute
    /// path into the agent pane so the Claude CLI — which accepts a plain-text
    /// image path in the prompt — picks it up. No Enter is sent: the user adds
    /// prompt text and submits. Returns the written path.
    ///
    /// Errors: [`SessionError::InvalidImage`] (bad/oversized content → 400),
    /// [`SessionError::TmuxSessionNotFound`] (no such session → 404).
    pub async fn paste_image(&self, query: &str, bytes: &[u8]) -> Result<PathBuf> {
        // Validate the bytes up front so junk/oversized input is a clean 400
        // regardless of whether the session exists (and before any disk write).
        crate::paste_image::validate(bytes)?;

        let tmux_name = self
            .resolve_tmux_session(query)
            .await?
            .ok_or_else(|| SessionError::TmuxSessionNotFound(query.to_string()))?;

        // Store under the OS temp dir, not the data dir: the temp dir is
        // space-free on every platform (macOS's data dir under `~/Library/
        // Application Support/…` contains spaces, which the CLI would mis-parse
        // in an unquoted injected path), and it's the same location
        // `write_apply_brief` uses for comment-apply briefs — proven readable by
        // the agent without a permission prompt. Tests override the base via
        // `paste_images_dir` to keep writes (and the store's prune) off the real
        // `/tmp`, per the repo's test-isolation rule.
        let base = self
            .read_config()
            .paste_images_dir
            .unwrap_or_else(std::env::temp_dir);
        let store = crate::paste_image::PasteImageStore::new(&base);
        let path = store.store(bytes)?;

        // Inject the path with `send-keys -l` (no Enter). Unlike automated
        // comment-apply, this is user-initiated (the operator pressed Ctrl+V) and
        // user-visible (they're attached and watching the pane), so it is *not*
        // gated on agent state via `decide_send`: if the agent happens to be at a
        // prompt, the user sees the path land and can correct it. The composed
        // text wraps the path in spaces and escapes any interior space; the path
        // is server-generated (UUID name, space-free temp dir) so it carries no
        // client-controlled content.
        let injected = crate::paste_image::compose_injection(&path);
        self.manager
            .tmux
            .send_keys_literal(&tmux_name, &injected)
            .await?;

        self.telemetry.feature("paste_image");
        debug!("pasted image for session {} -> {}", query, path.display());
        Ok(path)
    }

    pub async fn check_tmux(&self) -> Result<()> {
        self.manager.check_tmux().await
    }

    /// tmux availability with a short-TTL cache ([`TMUX_OK_CACHE_TTL`]) so a
    /// per-client `workspace_snapshot` poll doesn't fork `tmux -V` every call.
    /// The lock is never held across the probe `.await`; two concurrent stale
    /// callers may both re-probe once (a benign, self-healing race).
    async fn cached_tmux_ok(&self) -> bool {
        if let Some((at, ok)) = *self.tmux_ok_cache.lock().expect("tmux_ok_cache poisoned")
            && at.elapsed() < TMUX_OK_CACHE_TTL
        {
            return ok;
        }
        let ok = self.check_tmux().await.is_ok();
        *self.tmux_ok_cache.lock().expect("tmux_ok_cache poisoned") =
            Some((std::time::Instant::now(), ok));
        ok
    }

    // -- Mutations --

    pub async fn create_session(&self, opts: CreateSessionOpts) -> Result<SessionId> {
        self.telemetry.feature("session.create");
        self.manager.check_tmux().await?;

        let base_program = opts
            .program
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| self.config_store.read().default_session_program());

        validate_program_flags(&opts, &base_program)?;

        let program = program_with_agent_flags(
            &base_program,
            opts.mode.as_deref(),
            opts.effort.as_deref(),
            opts.model.as_deref(),
        );

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

        // Local stack-parent hint (set by the "new stacked session" flow):
        // `finalize_session` reads it to fork the branch from the parent's
        // branch and inject the PR-base launch context.
        if let Some(parent) = opts.stack_parent {
            self.store
                .mutate(move |state| {
                    if let Some(session) = state.sessions.get_mut(&session_id) {
                        session.stack_parent_session_id = Some(parent);
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

        // When smudging was skipped, the worktree holds LFS pointer files. The
        // TUI pulls the real content in the background, but a CLI invocation
        // exits right after this call, so pull synchronously here (best-effort)
        // to leave a usable worktree behind.
        if self.config_store.read().skip_lfs_smudge {
            let worktree_path = {
                let state = self.store.read().await;
                state
                    .get_session(&session_id)
                    .map(|s| s.worktree_path.clone())
            };
            if let Some(worktree_path) = worktree_path
                && let Err(e) = crate::git::lfs::pull(&worktree_path).await
            {
                tracing::warn!(error = %e, "git lfs pull after session create failed");
            }
        }

        Ok(session_id)
    }

    /// One-time startup reconciliation, run once when a frontend attaches (and
    /// on the server at boot). Bundles the checks the TUI used to run inline so
    /// every frontend — and the server — reconciles identically:
    ///
    /// 1. Drop sessions stuck in `Creating` from a previous run.
    /// 2. Reset transient `Merging`/`Pushing` states to `Running` (a died
    ///    mid-op process leaves stale spinners; `CascadePaused` is left alone).
    /// 3. Mark sessions whose tmux session/pane is gone as `Stopped`, and sync
    ///    each project's unmanaged worktrees.
    /// 4. Re-run section assignment over every session against current config.
    pub async fn startup_reconcile(&self) -> Result<()> {
        // 1. Stale Creating sessions.
        let creating_ids: Vec<SessionId> = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .filter(|s| s.status == SessionStatus::Creating)
                .map(|s| s.id)
                .collect()
        };
        if !creating_ids.is_empty() {
            warn!(
                "Cleaning up {} stale Creating session(s) from previous run",
                creating_ids.len()
            );
            self.store
                .mutate(move |state| {
                    for sid in &creating_ids {
                        state.remove_session(sid);
                    }
                })
                .await?;
        }

        // 2. Stale Merging/Pushing sessions.
        let stale_ids: Vec<SessionId> = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .filter(|s| matches!(s.status, SessionStatus::Merging | SessionStatus::Pushing))
                .map(|s| s.id)
                .collect()
        };
        if !stale_ids.is_empty() {
            warn!(
                "Resetting {} stale Merging/Pushing session(s) to Running",
                stale_ids.len()
            );
            self.store
                .mutate(move |state| {
                    for sid in &stale_ids {
                        if let Some(session) = state.get_session_mut(sid) {
                            session.set_status(SessionStatus::Running);
                        }
                    }
                })
                .await?;
        }

        // 3. Sync session status against live tmux, then sync worktrees.
        let session_ids: Vec<(SessionId, String)> = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .filter(|s| s.status.is_active() && s.status != SessionStatus::Creating)
                .map(|s| (s.id, s.tmux_session_name.clone()))
                .collect()
        };
        for (session_id, tmux_name) in session_ids {
            let should_mark_stopped =
                if let Ok(exists) = self.manager.tmux.session_exists(&tmux_name).await {
                    if !exists {
                        true
                    } else {
                        self.manager
                            .tmux
                            .is_pane_dead(&tmux_name)
                            .await
                            .unwrap_or(false)
                    }
                } else {
                    false
                };
            if should_mark_stopped {
                let _ = self.manager.tmux.kill_session(&tmux_name).await;
                self.store
                    .mutate(move |state| {
                        if let Some(session) = state.get_session_mut(&session_id) {
                            session.set_status(SessionStatus::Stopped);
                        }
                    })
                    .await?;
            }
        }
        let project_ids: Vec<ProjectId> = {
            let state = self.store.read().await;
            state.projects.keys().copied().collect()
        };
        for project_id in project_ids {
            if let Err(e) = self.manager.sync_worktrees(&project_id).await {
                debug!("Failed to sync worktrees for project {}: {}", project_id, e);
            }
        }

        // 4. Reconcile section assignments against current config.
        self.reconcile_all_section_assignments().await?;

        Ok(())
    }

    /// Re-run section assignment over every session against the current
    /// `[[sections]]` config. Used at startup and after a live config change.
    /// A no-op when no sections are configured and none are currently pinned.
    pub async fn reconcile_all_section_assignments(&self) -> Result<()> {
        let sections = self.config_store.read().sections.clone();
        let now = chrono::Utc::now();
        self.store
            .mutate(move |state| {
                if sections.is_empty()
                    && state.sessions.values().all(|s| s.current_section.is_none())
                {
                    return;
                }
                for session in state.sessions.values_mut() {
                    crate::session::apply_assignment(session, &sections, now);
                }
            })
            .await?;
        Ok(())
    }

    /// Re-run section assignment for a single session against current config.
    /// Used after creating a session, where the rest of the set is already
    /// reconciled. No-op when no sections are configured.
    pub async fn reconcile_one_section_assignment(&self, session_id: SessionId) -> Result<()> {
        let sections = self.config_store.read().sections.clone();
        if sections.is_empty() {
            return Ok(());
        }
        let now = chrono::Utc::now();
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&session_id) {
                    crate::session::apply_assignment(session, &sections, now);
                }
            })
            .await?;
        Ok(())
    }

    /// Persist a batch of PR-check results (from the background PR poll),
    /// re-run section assignment, then push refreshed status bars to running
    /// sessions' tmux panes. `Found` sets the cached PR fields, `NotFound`
    /// authoritatively clears them, `FetchFailed` preserves cached state so a
    /// transient error doesn't flatten a PR stack in the UI.
    pub async fn apply_pr_results(&self, results: Vec<(SessionId, PrCheckResult)>) -> Result<()> {
        let sections = self.config_store.read().sections.clone();
        let now = chrono::Utc::now();
        self.store
            .mutate(move |state| {
                for (session_id, result) in &results {
                    let Some(session) = state.get_session_mut(session_id) else {
                        continue;
                    };
                    match result {
                        PrCheckResult::Found(info) => {
                            session.pr_number = Some(info.number);
                            session.pr_url = Some(info.url.clone());
                            session.pr_state = Some(info.state);
                            session.pr_draft = info.is_draft;
                            session.pr_labels = info.labels.clone();
                            session.pr_merged = info.merged();
                            session.review_decision = info.review_decision;
                            session.pr_reviewers = info.reviewers.clone();
                            session.pr_base_branch = info.base_ref_name.clone();
                        }
                        PrCheckResult::NotFound => {
                            session.pr_number = None;
                            session.pr_url = None;
                            session.pr_state = None;
                            session.pr_draft = false;
                            session.pr_labels.clear();
                            session.pr_merged = false;
                            session.review_decision = None;
                            session.pr_reviewers.clear();
                            session.pr_base_branch = None;
                        }
                        PrCheckResult::FetchFailed => {}
                    }
                }
                for session in state.sessions.values_mut() {
                    crate::session::apply_assignment(session, &sections, now);
                }
            })
            .await?;

        // Push refreshed status bars to running sessions' tmux panes. Snapshot
        // under the lock, then release before the async tmux I/O.
        let status_bar_updates: Vec<_> = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .filter(|s| s.status == SessionStatus::Running)
                .map(|s| (s.tmux_session_name.clone(), self.status_bar_info(s, &state)))
                .collect()
        };
        for (tmux_name, info) in &status_bar_updates {
            self.manager
                .tmux
                .configure_status_bar(tmux_name, info)
                .await;
        }
        Ok(())
    }

    /// Mark a batch of sessions unread (agent-finished transitions detected by
    /// the poll loop). Paired with [`Self::mark_read`].
    pub async fn mark_unread(&self, ids: Vec<SessionId>) -> Result<()> {
        self.store
            .mutate(move |state| {
                for id in &ids {
                    if let Some(session) = state.get_session_mut(id) {
                        session.unread = true;
                    }
                }
            })
            .await
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

    /// Restart a session's tmux pane without `--resume` (a fresh agent
    /// conversation). Used by the attach loop when the agent process exits, so
    /// the user seamlessly gets a new session rather than being dropped to the
    /// tree.
    pub async fn restart_session_fresh(&self, id: &SessionId) -> Result<()> {
        self.telemetry.feature("session.restart_fresh");
        self.manager.restart_session_fresh(id).await
    }

    pub async fn delete_session(&self, id: &SessionId) -> Result<()> {
        self.telemetry.feature("session.delete");
        self.manager.delete_session(id).await
    }

    /// Set a session's keep-alive flag (opt-out of auto-hibernation).
    pub async fn set_keep_alive(&self, id: &SessionId, keep_alive: bool) -> Result<bool> {
        self.telemetry.feature("session.set_keep_alive");
        self.manager.set_keep_alive(id, keep_alive).await
    }

    /// Toggle a session's keep-alive flag, returning the new value.
    pub async fn toggle_keep_alive(&self, id: &SessionId) -> Result<bool> {
        self.telemetry.feature("session.toggle_keep_alive");
        self.manager.toggle_keep_alive(id).await
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
    /// read lock. Shared by `open_review`, `refresh_review_if_changed` and
    /// `fetch_diff_blob`.
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
        let (worktree_path, review_base, title, tmux_name, is_active, kind) = {
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
                AgentKind::from_program(&s.program),
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
        let ready = match decide_send(detector.detect(kind, &tmux_name).await) {
            SendDecision::Now => true,
            SendDecision::HoldUntilClear => wait_until_ready(&mut detector, kind, &tmux_name).await,
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

        // Delivering a prompt flips an idle agent back to working without
        // attaching or changing status, so bump last_active_at: a concurrent
        // hibernation pass then sees a fresh stamp and won't kill the session we
        // just handed work to (its still_hibernatable re-check compares stamps).
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.touch();
                }
            })
            .await?;

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

    // -- Workspace / tree (additive: everything the session tree needs) --

    /// One snapshot of the whole workspace: projects, sessions (including
    /// stopped, so the full tree renders), cascade state, pending-comment
    /// indicators, the recent-operations ledger, and server health. This is the
    /// single query a remote client polls to build the session tree.
    ///
    /// `project_pull` reflects the background pull loop's latest per-project
    /// status ([`Self::spawn_background_tasks`]); it is empty until the loop has
    /// run (or when project auto-pull is disabled).
    pub async fn workspace_snapshot(&self) -> Result<WorkspaceSnapshot> {
        let gh_available = self.gh_available().await;
        let tmux_ok = self.cached_tmux_ok().await;
        let pending = self.sessions_with_pending_comments().await?;

        let (projects, sessions, cascade_paused) = {
            let state = self.store.read().await;
            (
                build_project_info_list(&state),
                build_session_info_list(&state, true),
                state.cascade_paused_at,
            )
        };

        let mut pending_comment_sessions: Vec<SessionId> = pending.into_iter().collect();
        pending_comment_sessions.sort();

        Ok(WorkspaceSnapshot {
            projects,
            sessions,
            cascade_paused,
            pending_comment_sessions,
            project_pull: self
                .pull_status
                .lock()
                .expect("pull status poisoned")
                .clone(),
            operations: self.operations_snapshot(),
            server: ServerStatus {
                gh_available,
                tmux_ok,
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        })
    }

    /// List projects (sorted by name), for clients that only need the project
    /// set rather than a full [`Self::workspace_snapshot`].
    pub async fn list_projects(&self) -> Vec<ProjectInfo> {
        let state = self.store.read().await;
        build_project_info_list(&state)
    }

    /// Remove a project and all its sessions (kills their tmux sessions first).
    /// Wraps [`SessionManager::remove_project`].
    pub async fn remove_project(&self, id: &ProjectId) -> Result<()> {
        self.telemetry.feature("project.remove");
        self.manager.remove_project(id).await
    }

    /// List a project's git branches. When `fetch` is set, a best-effort
    /// `git fetch origin` runs first so newly-pushed remote branches appear.
    /// Mirrors the TUI checkout picker's source of truth
    /// ([`GitBackend::list_branches`]).
    pub async fn list_branches(
        &self,
        project_id: &ProjectId,
        fetch: bool,
    ) -> Result<Vec<BranchInfo>> {
        let repo_path = {
            let state = self.store.read().await;
            state
                .get_project(project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?
                .repo_path
                .clone()
        };
        if fetch {
            // Best-effort — a failed fetch (offline, no remote) just means we
            // list whatever refs already exist.
            let _ = tokio::process::Command::new("git")
                .current_dir(&repo_path)
                .args(["fetch", "origin"])
                .output()
                .await;
        }
        let backend = GitBackend::open(&repo_path)?;
        Ok(backend
            .list_branches()?
            .into_iter()
            .map(|(name, is_remote)| BranchInfo { name, is_remote })
            .collect())
    }

    /// Compute preview data (agent pane, diff + stat, shell pane) for a session
    /// or a project. Lifts the TUI's `fetch_preview_data`; the TUI keeps its own
    /// copy until Phase C deletes it.
    pub async fn preview(&self, target: PreviewTarget) -> Result<PreviewData> {
        match target {
            PreviewTarget::Session { id, lines } => self.session_preview(&id, lines).await,
            PreviewTarget::Project(id) => self.project_preview(&id).await,
        }
    }

    async fn session_preview(&self, sid: &SessionId, lines: Option<usize>) -> Result<PreviewData> {
        let (is_creating, tmux_name) = {
            let state = self.store.read().await;
            match state.get_session(sid) {
                Some(s) => (
                    s.status == crate::session::SessionStatus::Creating,
                    s.tmux_session_name.clone(),
                ),
                None => return Err(SessionError::NotFound(*sid).into()),
            }
        };
        // No tmux session exists yet while creating; short-circuit like the TUI.
        if is_creating {
            return Ok(PreviewData {
                pane: Some("Creating session...".to_string()),
                diff_text: String::new(),
                diff_stat: None,
                stats: None,
                shell: None,
            });
        }

        // An explicit line count captures the pane directly; otherwise use the
        // manager's cached content (matches the TUI preview).
        let pane = match lines {
            Some(n) => capture_pane(&self.manager.tmux, &tmux_name, Some(n)).await?,
            None => self.manager.get_content(sid).await.ok().map(|c| c.content),
        };
        let diff = self.manager.get_diff(sid).await.ok();
        let shell = self
            .manager
            .get_shell_content(sid)
            .await
            .ok()
            .flatten()
            .map(|c| c.content);
        Ok(PreviewData {
            pane,
            diff_text: diff.as_ref().map(|d| d.diff.clone()).unwrap_or_default(),
            diff_stat: diff.as_ref().map(|d| d.summary()),
            stats: diff.as_ref().map(|d| diff_stat_from_info(d)),
            shell,
        })
    }

    async fn project_preview(&self, pid: &ProjectId) -> Result<PreviewData> {
        let diff = self.manager.get_project_diff(pid).await.ok();
        let shell = self
            .manager
            .get_project_shell_content(pid)
            .await
            .ok()
            .flatten()
            .map(|c| c.content);
        Ok(PreviewData {
            pane: None,
            diff_text: diff.as_ref().map(|d| d.diff.clone()).unwrap_or_default(),
            diff_stat: diff.as_ref().map(|d| d.summary()),
            stats: diff.as_ref().map(|d| diff_stat_from_info(d)),
            shell,
        })
    }

    /// The full branch diff (committed vs `origin/<main>` plus uncommitted
    /// working changes) used for the AI summary. Wraps
    /// [`crate::git::compute_branch_diff`].
    pub async fn branch_diff(&self, session_id: &SessionId) -> Result<String> {
        let (worktree_path, main_branch) = {
            let state = self.store.read().await;
            let s = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            let project = state
                .get_project(&s.project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(s.project_id.to_string()))?;
            (s.worktree_path.clone(), project.main_branch.clone())
        };
        Ok(compute_branch_diff(&worktree_path, &main_branch).await)
    }

    /// Rename a session's title. Rejects an empty (whitespace-only) title.
    pub async fn rename_session(&self, id: &SessionId, title: impl Into<String>) -> Result<()> {
        let title = title.into().trim().to_string();
        if title.is_empty() {
            return Err(SessionError::InvalidName {
                name: title,
                reason: "session name cannot be empty".to_string(),
            }
            .into());
        }
        self.ensure_session_exists(id).await?;
        self.telemetry.feature("session.rename");
        let id = *id;
        self.store
            .mutate(move |state| {
                if let Some(s) = state.get_session_mut(&id) {
                    s.title = title;
                }
            })
            .await
    }

    /// Move a session to a section (`Some(name)`) or clear its manual override
    /// and re-run predicate assignment (`None`). Mirrors the TUI's
    /// `apply_section_move`.
    pub async fn set_section(&self, id: &SessionId, section: Option<String>) -> Result<()> {
        self.ensure_session_exists(id).await?;
        self.telemetry.feature("session.set_section");
        let sections = self.config_store.read().sections.clone();
        let now = chrono::Utc::now();
        let id = *id;
        self.store
            .mutate(move |state| {
                if let Some(s) = state.get_session_mut(&id) {
                    match section {
                        Some(name) => {
                            s.section_override = Some(name);
                            apply_assignment(s, &sections, now);
                        }
                        None => {
                            clear_override_and_reassign(s, &sections, now);
                        }
                    }
                }
            })
            .await
    }

    /// Stamp a session's `last_attached_at` to now, for MRU ordering in the
    /// in-session switcher. Called by the backend's `attach` so every frontend
    /// records the attach the same way (the TUI used to do this inline). No-op
    /// if the session doesn't exist — an attach to a vanished session is a
    /// harmless race, not an error worth surfacing here.
    pub async fn mark_attached(&self, id: &SessionId) -> Result<()> {
        let id = *id;
        self.store
            .mutate(move |state| {
                if let Some(s) = state.get_session_mut(&id) {
                    s.mark_attached();
                }
            })
            .await
    }

    /// Clear a session's unread flag (as the TUI does when attaching).
    pub async fn mark_read(&self, id: &SessionId) -> Result<()> {
        self.ensure_session_exists(id).await?;
        let id = *id;
        self.store
            .mutate(move |state| {
                if let Some(s) = state.get_session_mut(&id) {
                    s.unread = false;
                }
            })
            .await
    }

    /// New-session dialog options: the default program, the configured program
    /// list, and the configured section names.
    pub fn create_options(&self) -> CreateOptions {
        let config = self.config_store.read();
        CreateOptions {
            default_program: config.default_session_program(),
            programs: config.programs.iter().map(ProgramInfo::from).collect(),
            sections: config.sections.iter().map(|s| s.name.clone()).collect(),
        }
    }

    // -- Cascade / push-stack (record outcomes in the operation ledger) --

    /// Cascade-merge the stack containing `start_from`. Detects agent states
    /// itself (the server has no cached map like the TUI), runs the merge, and
    /// records the outcome in the ledger. Returns the recorded status.
    pub async fn cascade_merge(&self, start_from: &SessionId) -> Result<OperationStatus> {
        self.telemetry.feature("cascade.merge");
        let states = self.detect_active_states().await;
        let outcome = self.manager.cascade_merge_stack(start_from, &states).await;
        Ok(self.record_cascade_outcome(outcome))
    }

    /// Resume a paused cascade. Records the outcome in the ledger.
    pub async fn cascade_resume(&self) -> Result<OperationStatus> {
        self.telemetry.feature("cascade.resume");
        let states = self.detect_active_states().await;
        let outcome = self.manager.cascade_resume(&states).await;
        Ok(self.record_cascade_outcome(outcome))
    }

    /// Push every branch in the stack containing `start_from`. Records the
    /// outcome in the ledger.
    pub async fn push_stack(&self, start_from: &SessionId) -> Result<OperationStatus> {
        self.telemetry.feature("stack.push");
        let states = self.detect_active_states().await;
        let outcome = match self.manager.push_stack(start_from, &states).await {
            Ok(o) => OperationOutcome::Succeeded {
                detail: format!("{} pushed", o.sessions_pushed),
            },
            Err(e) => OperationOutcome::Failed {
                error: e.to_string(),
            },
        };
        Ok(self.record_operation(OperationKind::PushStack, outcome))
    }

    /// Bulk agent-state snapshot over active sessions.
    ///
    /// When the background poll loop ([`Self::spawn_background_tasks`]) is
    /// running, a non-`fresh` call serves its maintained cache (so the reported
    /// `commander_running` and states reflect the loop's liveness, and a remote
    /// client polling the route sees the same data the TUI does). Before the
    /// loop's first tick — or when no loop runs (CLI, tests) — it falls back to
    /// on-demand detection via the shared TTL-cached detector.
    ///
    /// `fresh` always bypasses caches with a zero-TTL detector, forcing a
    /// re-capture, and folds the result back into the shared cache so the loop's
    /// unread baseline advances too — this is what stops a session the operator
    /// just watched go idle during an attach from being re-flagged unread on the
    /// next poll tick.
    pub async fn agent_states(&self, fresh: bool) -> AgentStatesSnapshot {
        if fresh {
            // Recompute commander liveness the same way the poll loop does, so a
            // fresh read (and any non-fresh read served from the primed cache
            // afterwards, including a remote client's) reflects the commander's
            // real state rather than a stale flag left in the cache.
            let (commander_enabled, commander_program) = {
                let c = self.config_store.read();
                (c.commander_enabled, c.commander_program())
            };
            let commander_running =
                commander_enabled && crate::commander::is_running(&self.manager.tmux).await;
            // Include the commander's sentinel target when it's live so the
            // rebuilt cache carries its state too — otherwise the commander chip
            // blinks empty for one tick after an attach primes the cache.
            let active = with_commander_target(
                self.active_session_targets().await,
                commander_running,
                &commander_program,
            );
            let mut detector = AgentStateDetector::new(self.manager.tmux.clone(), Duration::ZERO);
            let states = detector.detect_all(&active).await;
            let mut cache = self.agent_states_cache.write().await;
            cache.states = states;
            cache.commander_running = commander_running;
            self.agent_states_primed
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return cache.clone();
        }
        if self
            .agent_states_primed
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return self.agent_states_cache.read().await.clone();
        }
        let active = self.active_session_targets().await;
        let states = {
            let mut detector = self.agent_detector.lock().await;
            detector.detect_all(&active).await
        };
        // Mirror the fresh/loop arms: never fabricate `commander_running`.
        // `is_running` short-circuits when the commander is disabled, so no
        // tmux probe happens in the common (disabled) case.
        let commander_enabled = self.config_store.read().commander_enabled;
        let commander_running =
            commander_enabled && crate::commander::is_running(&self.manager.tmux).await;
        AgentStatesSnapshot {
            states,
            commander_running,
        }
    }

    /// Drop any agent states left in the cache once the running-session set has
    /// gone empty (the last running session stopped), waking the change-feed so
    /// clients re-read an empty snapshot rather than being served ghost states.
    /// Idempotent: a call on an already-empty cache is a no-op (returns whether
    /// it cleared anything). Called from the agent-state loop's quiet path.
    async fn clear_stale_agent_states(&self) -> bool {
        let had_states = !self.agent_states_cache.read().await.states.is_empty();
        if had_states {
            self.agent_states_cache.write().await.states.clear();
            self.store.notify_change();
        }
        had_states
    }

    /// Request an immediate PR-metadata refresh. Wakes the background PR-status
    /// loop ([`Self::spawn_background_tasks`]); if no loop is running (PR checks
    /// disabled, or before startup) the notification is simply dropped.
    pub fn request_pr_refresh(&self) -> Result<()> {
        self.pr_refresh.notify_one();
        Ok(())
    }

    /// Spawn the service-owned background loops: agent-state polling, PR-status
    /// checks, project auto-pull, and cross-instance state-sync. Idempotent — a
    /// second call (e.g. a local TUI and an embedded caller sharing one service)
    /// is a no-op and returns empty handles.
    ///
    /// Every loop drives the same observable surface the frontends read: the
    /// agent loop maintains [`Self::agent_states`]' cache and persists unread
    /// transitions; the PR loop persists results via [`Self::apply_pr_results`];
    /// the pull loop feeds [`WorkspaceSnapshot::project_pull`]; the sync loop
    /// reloads the state file. Each wakes the [`StateStore`] change-feed on a
    /// real change (either via a persisted mutation or [`StateStore::notify_change`]),
    /// so a subscriber (the TUI's per-backend change-feed task) re-reads the
    /// relevant snapshot — no frontend-specific event plumbing crosses the
    /// backend seam.
    ///
    /// Intervals are read from config once here (matching the old TUI loops).
    /// The returned [`BackgroundHandles`] let tests abort the loops; production
    /// callers ignore them and let the tasks run for the process lifetime.
    pub fn spawn_background_tasks(&self, opts: BackgroundOpts) -> BackgroundHandles {
        // Idempotency: only the first caller spawns.
        if self
            .background_started
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_err()
        {
            return BackgroundHandles::default();
        }

        let config = self.config_store.read().clone();
        let handles = vec![
            self.spawn_agent_state_loop(
                config.agent_state_poll_interval_ms,
                opts.commander_enabled,
                config.commander_program(),
            ),
            self.spawn_pr_status_loop(config.pr_check_interval_secs),
            self.spawn_project_pull_loop(
                config.project_pull_enabled,
                config.project_pull_interval_secs,
            ),
            self.spawn_state_sync_loop(config.state_sync_interval_ms),
        ];
        BackgroundHandles { handles }
    }

    /// Poll every running session's (and the commander's) agent state on a fixed
    /// cadence, maintain [`Self::agent_states_cache`], persist Working→Idle
    /// transitions as unread, and wake the change-feed on any change. No-op loop
    /// when `interval_ms` is 0.
    fn spawn_agent_state_loop(
        &self,
        interval_ms: u64,
        commander_enabled: bool,
        commander_program: String,
    ) -> tokio::task::JoinHandle<()> {
        let service = self.clone();
        let tmux = self.manager.tmux.clone();
        let cache = self.agent_states_cache.clone();
        let primed = self.agent_states_primed.clone();
        let store = self.store.clone();
        tokio::spawn(async move {
            if interval_ms == 0 {
                return;
            }
            let cache_ttl = Duration::from_millis(interval_ms.saturating_sub(500).max(500));
            let mut detector = AgentStateDetector::new(tmux.clone(), cache_ttl);
            let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
            let mut last_commander_running = false;
            let sentinel = crate::commander::commander_sentinel_id();
            loop {
                interval.tick().await;
                let sessions: Vec<(SessionId, String, String)> = {
                    let state = store.read().await;
                    state
                        .sessions
                        .values()
                        .filter(|s| s.status == SessionStatus::Running)
                        .map(|s| (s.id, s.tmux_session_name.clone(), s.program.clone()))
                        .collect()
                };
                let commander_running =
                    commander_enabled && crate::commander::is_running(&tmux).await;
                let sessions =
                    with_commander_target(sessions, commander_running, &commander_program);
                // Quiet path: nothing to detect and the commander's state is
                // unchanged — skip the tick (no cache write, no wake). But if the
                // cache still holds states from a previous tick (the last running
                // session just stopped), clear it once and wake the feed so
                // clients — including remote pollers — aren't served ghost states.
                if poll_tick_can_skip(
                    sessions.is_empty(),
                    commander_running,
                    last_commander_running,
                ) {
                    service.clear_stale_agent_states().await;
                    continue;
                }
                let states: BTreeMap<SessionId, AgentState> = if sessions.is_empty() {
                    BTreeMap::new()
                } else {
                    detector.detect_all(&sessions).await
                };
                if !poll_tick_should_send(
                    states.is_empty(),
                    commander_running,
                    last_commander_running,
                ) {
                    continue;
                }
                let commander_flipped = commander_running != last_commander_running;
                last_commander_running = commander_running;

                // Diff against the previous cache to flag agents that just
                // finished a turn (Working→Idle), skipping the commander
                // sentinel (it has no `WorktreeSession` to mark).
                let prev = { cache.read().await.states.clone() };
                let unread_ids: Vec<SessionId> = detect_unread_transitions(&prev, &states)
                    .into_iter()
                    .filter(|id| *id != sentinel)
                    .collect();
                let states_changed = states != prev;

                // Only write the cache when something changed: a rebuilt-but-
                // identical map would serialize identically anyway (BTreeMap,
                // deterministic key order), but skipping the write keeps the
                // remote pollers' content-hash diffing honest by construction
                // and avoids needless lock traffic on all-idle ticks.
                {
                    let mut c = cache.write().await;
                    if states_changed {
                        c.states = states;
                    }
                    c.commander_running = commander_running;
                }
                primed.store(true, std::sync::atomic::Ordering::Relaxed);

                // Wake the change-feed only when observable state actually
                // changed (fresh states, or the commander flipped), so a steady
                // row of idle agents doesn't trigger a snapshot re-fetch every
                // tick. Persisting unread already bumps the feed; otherwise wake
                // it explicitly. The cache is updated first so the snapshot the
                // wake triggers reads the fresh states.
                if !unread_ids.is_empty() {
                    let _ = service.mark_unread(unread_ids).await;
                } else if states_changed || commander_flipped {
                    store.notify_change();
                }
            }
        })
    }

    /// Fan out `gh pr list` across all sessions on a fixed cadence (and on
    /// [`Self::request_pr_refresh`]), then persist results via
    /// [`Self::apply_pr_results`]. When `interval_secs` is 0 the periodic tick is
    /// disabled but a manual refresh still runs.
    fn spawn_pr_status_loop(&self, interval_secs: u64) -> tokio::task::JoinHandle<()> {
        let service = self.clone();
        let store = self.store.clone();
        let notify = self.pr_refresh.clone();
        let last_check = self.last_pr_check.clone();
        tokio::spawn(async move {
            let mut ticker = (interval_secs > 0)
                .then(|| tokio::time::interval(Duration::from_secs(interval_secs)));
            loop {
                match ticker.as_mut() {
                    Some(t) => {
                        tokio::select! {
                            _ = t.tick() => {}
                            _ = notify.notified() => {}
                        }
                    }
                    None => notify.notified().await,
                }
                // Debounce rapid re-triggers (e.g. a double manual refresh) so we
                // don't launch several concurrent sweeps.
                {
                    let mut lc = last_check.lock().expect("pr check time poisoned");
                    let now = std::time::Instant::now();
                    if !pr_check_debounce_passed(*lc, now, PR_CHECK_DEBOUNCE) {
                        continue;
                    }
                    *lc = Some(now);
                }
                if !service.gh_available().await {
                    continue;
                }
                let sessions_to_check: Vec<(SessionId, String, PathBuf)> = {
                    let state = store.read().await;
                    state
                        .sessions
                        .values()
                        .filter(|s| s.status != SessionStatus::Creating)
                        .filter_map(|s| {
                            let project = state.projects.get(&s.project_id)?;
                            Some((s.id, s.branch.clone(), project.repo_path.clone()))
                        })
                        .collect()
                };
                if sessions_to_check.is_empty() {
                    continue;
                }
                let results: Vec<(SessionId, PrCheckResult)> =
                    futures::stream::iter(sessions_to_check.into_iter().map(
                        |(id, branch, repo_path)| async move {
                            (
                                id,
                                crate::git::check_pr_for_branch(&repo_path, &branch).await,
                            )
                        },
                    ))
                    .buffer_unordered(PR_FANOUT_CONCURRENCY)
                    .collect()
                    .await;
                if let Err(e) = service.apply_pr_results(results).await {
                    debug!("apply_pr_results failed: {e}");
                }
            }
        })
    }

    /// Fast-forward each project's main branch on a fixed cadence, recording the
    /// per-project outcome in [`Self::pull_status`] (surfaced through
    /// [`WorkspaceSnapshot::project_pull`]) and waking the change-feed when any
    /// project's status changes. No-op loop when disabled or `interval_secs` 0.
    fn spawn_project_pull_loop(
        &self,
        enabled: bool,
        interval_secs: u64,
    ) -> tokio::task::JoinHandle<()> {
        let store = self.store.clone();
        let pull_status = self.pull_status.clone();
        tokio::spawn(async move {
            if !enabled || interval_secs == 0 {
                return;
            }
            // Startup grace so launch doesn't immediately hammer every project.
            tokio::time::sleep(PROJECT_PULL_STARTUP_GRACE).await;
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                let projects: Vec<(ProjectId, PathBuf, String)> = {
                    let state = store.read().await;
                    state
                        .projects
                        .values()
                        .map(|p| (p.id, p.repo_path.clone(), p.main_branch.clone()))
                        .collect()
                };
                if projects.is_empty() {
                    continue;
                }
                let outcomes: Vec<(ProjectId, PullStatus)> =
                    futures::stream::iter(projects.into_iter().map(
                        |(id, repo_path, main_branch)| async move {
                            let status = crate::git::run_project_pull(&repo_path, &main_branch)
                                .await
                                .to_status();
                            (id, status)
                        },
                    ))
                    .buffer_unordered(PROJECT_PULL_FANOUT_CONCURRENCY)
                    .collect()
                    .await;

                let mut changed = false;
                {
                    let mut status = pull_status.lock().expect("pull status poisoned");
                    for (id, outcome) in outcomes {
                        if status.get(&id) != Some(&outcome) {
                            status.insert(id, outcome);
                            changed = true;
                        }
                    }
                }
                if changed {
                    store.notify_change();
                }
            }
        })
    }

    /// Reload the state file on a fixed cadence, waking the change-feed when
    /// another instance mutated it. No-op loop when `interval_ms` is 0.
    fn spawn_state_sync_loop(&self, interval_ms: u64) -> tokio::task::JoinHandle<()> {
        let store = self.store.clone();
        tokio::spawn(async move {
            if interval_ms == 0 {
                return;
            }
            let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
            loop {
                interval.tick().await;
                match store.reload_if_changed().await {
                    Ok(_) => {}
                    Err(e) => debug!("State sync check failed: {e}"),
                }
            }
        })
    }

    // -- Internal helpers for the workspace surface --

    /// Cached `gh --version` availability (computed once per process).
    async fn gh_available(&self) -> bool {
        *self
            .gh_available
            .get_or_init(|| async { is_gh_available().await })
            .await
    }

    /// Error with `NotFound` when a session id doesn't exist, so mutations can
    /// surface a 404 rather than silently no-op'ing.
    async fn ensure_session_exists(&self, id: &SessionId) -> Result<()> {
        let state = self.store.read().await;
        if state.get_session(id).is_none() {
            return Err(SessionError::NotFound(*id).into());
        }
        Ok(())
    }

    /// `(id, tmux_session_name, program)` tuples for active sessions, the input
    /// shape [`AgentStateDetector::detect_all`] expects.
    async fn active_session_targets(&self) -> Vec<(SessionId, String, String)> {
        let state = self.store.read().await;
        state
            .get_active_sessions()
            .into_iter()
            .map(|s| (s.id, s.tmux_session_name.clone(), s.program.clone()))
            .collect()
    }

    /// Detect agent states for active sessions via the shared TTL-cached
    /// detector — used to gate cascade/push delivery.
    async fn detect_active_states(&self) -> BTreeMap<SessionId, AgentState> {
        let active = self.active_session_targets().await;
        let mut detector = self.agent_detector.lock().await;
        detector.detect_all(&active).await
    }

    /// Push an operation onto the bounded ledger and return the recorded status.
    fn record_operation(&self, kind: OperationKind, outcome: OperationOutcome) -> OperationStatus {
        let status = OperationStatus {
            id: self.next_op_id.fetch_add(1, Ordering::Relaxed),
            kind,
            outcome,
            finished_at: Some(chrono::Utc::now()),
        };
        let mut ledger = self.operations.lock().expect("operation ledger poisoned");
        if ledger.len() >= OPERATION_LEDGER_CAP {
            ledger.pop_front();
        }
        ledger.push_back(status.clone());
        status
    }

    /// Map a cascade result onto a ledger entry (`Complete`/`PausedOnConflict`/
    /// error) and record it.
    fn record_cascade_outcome(&self, outcome: Result<CascadeOutcome>) -> OperationStatus {
        let outcome = match outcome {
            Ok(CascadeOutcome::Complete { sessions_merged }) => OperationOutcome::Succeeded {
                detail: format!("{sessions_merged} merged"),
            },
            Ok(CascadeOutcome::PausedOnConflict {
                at,
                sessions_merged,
            }) => OperationOutcome::Paused {
                detail: format!("paused at {at} after {sessions_merged} merged"),
            },
            Err(e) => OperationOutcome::Failed {
                error: e.to_string(),
            },
        };
        self.record_operation(OperationKind::Cascade, outcome)
    }

    /// Snapshot the operation ledger (oldest first).
    fn operations_snapshot(&self) -> Vec<OperationStatus> {
        self.operations
            .lock()
            .expect("operation ledger poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

/// What a [`CommanderService::preview`] call targets: a session (with an
/// optional explicit pane line count) or a project.
#[derive(Debug, Clone, Copy)]
pub enum PreviewTarget {
    Session { id: SessionId, lines: Option<usize> },
    Project(ProjectId),
}

/// Project a computed [`DiffInfo`](crate::git::DiffInfo)'s counts onto the
/// protocol [`DiffStat`] DTO carried in [`PreviewData`].
fn diff_stat_from_info(d: &crate::git::DiffInfo) -> crate::api::DiffStat {
    crate::api::DiffStat {
        files_changed: d.files_changed,
        lines_added: d.lines_added,
        lines_removed: d.lines_removed,
    }
}

/// Options for [`CommanderService::spawn_background_tasks`].
#[derive(Debug, Clone, Default)]
pub struct BackgroundOpts {
    /// Whether the persistent commander session should be polled for its agent
    /// state. Restart-required in the TUI (mirrors `commander_enabled_at_init`),
    /// so it is passed in rather than re-read live.
    pub commander_enabled: bool,
}

/// Abortable handles to the service's background loops (see
/// [`CommanderService::spawn_background_tasks`]). Production callers ignore the
/// value and let the loops run for the process lifetime; tests hold it and call
/// [`Self::abort`] to stop them deterministically.
#[derive(Default)]
pub struct BackgroundHandles {
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl BackgroundHandles {
    /// Abort every background loop. Idempotent.
    pub fn abort(&self) {
        for h in &self.handles {
            h.abort();
        }
    }

    /// Whether this handle owns no loops — true for the no-op returned by a
    /// second [`CommanderService::spawn_background_tasks`] call.
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

/// Cap concurrent subprocess fan-outs (e.g. `gh pr list` across all sessions).
/// Each call holds 3+ pipe FDs, so unbounded fan-out can EMFILE under the macOS
/// launchd 256-FD default.
const PR_FANOUT_CONCURRENCY: usize = 8;

/// Cap concurrent project-branch pulls so a user with many projects doesn't
/// spawn one `git fetch` per project at the same instant.
const PROJECT_PULL_FANOUT_CONCURRENCY: usize = 4;

/// Minimum gap between PR-status fan-outs, debouncing rapid manual triggers
/// (e.g. a double manual refresh). It sits far below `pr_check_interval_secs`,
/// so a manual refresh is still effectively immediate.
const PR_CHECK_DEBOUNCE: Duration = Duration::from_secs(2);

/// Grace period after startup before the first project-pull sweep, so launch
/// doesn't immediately hammer every project.
const PROJECT_PULL_STARTUP_GRACE: Duration = Duration::from_secs(5);

/// Whether enough time has elapsed since the last PR-status check to spawn
/// another (see [`PR_CHECK_DEBOUNCE`]). `None` (never checked) always passes.
fn pr_check_debounce_passed(
    last_check: Option<std::time::Instant>,
    now: std::time::Instant,
    debounce: Duration,
) -> bool {
    last_check.is_none_or(|t| now.saturating_duration_since(t) >= debounce)
}

/// Session ids whose agent just finished a turn: present as [`AgentState::Idle`]
/// now but [`AgentState::Working`] in the previous poll. Drives the unread
/// marker. An empty `prev` (never polled, or cleared after an attach) yields no
/// transitions, so a freshly-populated baseline can't produce false unread.
pub(crate) fn detect_unread_transitions(
    prev: &BTreeMap<SessionId, AgentState>,
    new: &BTreeMap<SessionId, AgentState>,
) -> Vec<SessionId> {
    new.iter()
        .filter(|(id, state)| {
            **state == AgentState::Idle && prev.get(id) == Some(&AgentState::Working)
        })
        .map(|(id, _)| *id)
        .collect()
}

/// Append the commander's sentinel detection target to `active` when the
/// commander is running, so its agent state is detected alongside real
/// sessions. The sentinel is a reserved id with no `WorktreeSession`; callers
/// that flag unread transitions must filter it out. Shared by the poll loop and
/// the `fresh` [`CommanderService::agent_states`] rebuild so both carry the
/// commander's state (keeping the footer chip from blinking empty for a tick).
fn with_commander_target(
    mut active: Vec<(SessionId, String, String)>,
    commander_running: bool,
    commander_program: &str,
) -> Vec<(SessionId, String, String)> {
    if commander_running {
        active.push((
            crate::commander::commander_sentinel_id(),
            crate::commander::COMMANDER_TMUX_NAME.to_string(),
            commander_program.to_string(),
        ));
    }
    active
}

/// Whether the agent-state poll tick can skip entirely: nothing to detect (no
/// running sessions and the commander isn't running) AND the commander's
/// running state has not changed since the last emitted update. Skipping keeps
/// the no-sessions path quiet — no cache write, no change-feed wake.
fn poll_tick_can_skip(
    sessions_empty: bool,
    commander_running: bool,
    last_commander_running: bool,
) -> bool {
    sessions_empty && !commander_running && !last_commander_running
}

/// Whether a poll tick (that wasn't skipped) should emit an update: when there
/// are fresh agent states, or the commander's running state flipped — the
/// latter is what lets the footer chip turn *off* on the trailing edge.
fn poll_tick_should_send(
    states_empty: bool,
    commander_running: bool,
    last_commander_running: bool,
) -> bool {
    !states_empty || commander_running != last_commander_running
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
async fn wait_until_ready(
    detector: &mut AgentStateDetector,
    kind: AgentKind,
    tmux_name: &str,
) -> bool {
    const ATTEMPTS: u32 = 20;
    const INTERVAL: Duration = Duration::from_millis(250);
    for _ in 0..ATTEMPTS {
        if detector.detect(kind, tmux_name).await != AgentState::WaitingForInput {
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

/// Validate that Claude-only create flags aren't set for a program that can't
/// use them. [`CreateSessionOpts`] itself is a plain wire type in
/// `claude-commander-protocol`; this check lives here because it needs core's
/// [`AgentKind`] harness abstraction.
pub fn validate_program_flags(opts: &CreateSessionOpts, resolved_program: &str) -> Result<()> {
    let kind = AgentKind::from_program(resolved_program);
    // `--effort` / `--mode` map to Claude-specific flags.
    if !kind.is_claude() && (opts.effort.is_some() || opts.mode.is_some()) {
        return Err(SessionError::InvalidProgram(format!(
            "--effort and --mode are only supported when the program is \
             claude (got {:?})",
            resolved_program
        ))
        .into());
    }
    // An initial prompt is passed as a positional argument, which only
    // harnesses that accept one (claude, codex) understand.
    if opts.initial_prompt.is_some() && !kind.accepts_positional_prompt() {
        return Err(SessionError::InvalidProgram(format!(
            "--initial-prompt is only supported for programs that accept a \
             positional prompt, e.g. claude or codex (got {:?})",
            resolved_program
        ))
        .into());
    }
    // `--model` is understood by Claude, Codex, and OpenCode.
    if opts.model.is_some() && !kind.supports_model_flag() {
        return Err(SessionError::InvalidProgram(format!(
            "--model is only supported for programs that accept it, e.g. \
             claude, codex, or opencode (got {:?})",
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
    AgentStatesSnapshot, BranchInfo, CreateOptions, CreateSessionOpts, DiffSide, DiffStat,
    NewComment, OperationKind, OperationOutcome, OperationStatus, PreviewData, ProgramInfo,
    ProjectInfo, PullBlockReason, PullStatus, RenameSession, ReviewSnapshot, ServerStatus,
    SessionDetail, SessionInfo, SetProgramsRequest, SetSection, ToggleReviewed, WorkspaceSnapshot,
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
        unread: session.unread,
        stack_parent_session_id: session.stack_parent_session_id,
        pr_base_branch: session.pr_base_branch.clone(),
        pr_merged: session.pr_merged,
        current_section: session.current_section.clone(),
        section_override: session.section_override.clone(),
        entered_section_at: Some(session.entered_section_at),
        last_attached_at: session.last_attached_at,
        keep_alive: session.keep_alive,
        worktree_path: session.worktree_path.to_string_lossy().into_owned(),
        tmux_session_name: session.tmux_session_name.clone(),
    }
}

// -- Internal helpers --

/// Build the [`ProjectInfo`] list from state, sorted by project name (stable
/// ordering for the tree). Sessions are carried by id only; the full
/// [`SessionInfo`] list rides alongside in [`WorkspaceSnapshot`].
fn build_project_info_list(state: &AppState) -> Vec<ProjectInfo> {
    let mut projects: Vec<_> = state.projects.values().collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    projects
        .into_iter()
        .map(|p| ProjectInfo {
            id: p.id,
            name: p.name.clone(),
            repo_path: p.repo_path.clone(),
            main_branch: p.main_branch.clone(),
            session_ids: p.worktrees.clone(),
        })
        .collect()
}

fn build_session_info_list(state: &AppState, include_stopped: bool) -> Vec<SessionInfo> {
    let mut entries: Vec<(String, SessionInfo)> = Vec::new();
    for project in state.projects.values() {
        for session in project
            .worktrees
            .iter()
            .filter_map(|id| state.sessions.get(id))
            .filter(|s| include_stopped || s.status.is_active())
        {
            entries.push((
                project.name.clone(),
                session_info_from_session(session, &project.name),
            ));
        }
    }
    // Canonicalize the order so it never depends on `state.projects`' HashMap
    // iteration order (which would leak into the serialized snapshot and make
    // the wire output nondeterministic across processes/polls). Key: project
    // name, then the session's `created_at`, then its id as a final stable
    // tiebreaker for sessions sharing a timestamp.
    entries.sort_by(|(a_name, a), (b_name, b)| {
        a_name
            .cmp(b_name)
            .then(a.created_at.cmp(&b.created_at))
            .then(a.session_id.cmp(&b.session_id))
    });
    entries.into_iter().map(|(_, info)| info).collect()
}

/// Build a [`WorkspaceSnapshot`] purely from persisted state, with no live
/// gh/tmux probing — the I/O-bearing fields ([`ServerStatus`], pending
/// comments, operations) get cheap placeholders. The full
/// [`CommanderService::workspace_snapshot`] is the production source of truth
/// (the change-feed cache reads it); this synchronous, allocation-only
/// projection is retained for the tree-builder tests, which feed a
/// hand-constructed `AppState` through the same DTO builders.
#[cfg(test)]
pub(crate) fn workspace_snapshot_from_state(state: &AppState) -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        projects: build_project_info_list(state),
        sessions: build_session_info_list(state, true),
        cascade_paused: state.cascade_paused_at,
        pending_comment_sessions: Vec::new(),
        project_pull: BTreeMap::new(),
        operations: Vec::new(),
        server: ServerStatus {
            gh_available: false,
            tmux_ok: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    }
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
    use crate::session::{Project, ProjectId, SessionId, SessionStatus, WorktreeSession};
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
    fn build_session_info_list_order_is_canonical_not_insertion_order() {
        // Six single-session projects (names p0..p5) built up front, then
        // inserted into two states in OPPOSITE orders. The serialized session
        // order must equal the project-name-sorted order in BOTH — never the
        // HashMap iteration order of `state.projects`. Against an unsorted
        // builder the reverse (and almost always the forward) insertion diverges
        // from the canonical order for six projects: red.
        let mut fixtures: Vec<(Project, WorktreeSession)> = (0..6)
            .map(|i| {
                let p = make_project(&format!("p{i}"));
                let s = make_session_for_project("s", p.id);
                (p, s)
            })
            .collect();
        // Canonical order: by project name, i.e. the p0..p5 build order.
        let expected: Vec<SessionId> = fixtures.iter().map(|(_, s)| s.id).collect();

        let build = |order: &[(Project, WorktreeSession)]| -> Vec<SessionId> {
            let mut state = AppState::new();
            for (p, s) in order {
                let mut proj = p.clone();
                proj.add_worktree(s.id);
                state.projects.insert(proj.id, proj);
                state.sessions.insert(s.id, s.clone());
            }
            build_session_info_list(&state, true)
                .into_iter()
                .map(|si| si.session_id)
                .collect()
        };

        let forward = build(&fixtures);
        fixtures.reverse();
        let reverse = build(&fixtures);

        assert_eq!(
            forward, expected,
            "forward-insertion session order must be canonical (name-sorted)"
        );
        assert_eq!(
            reverse, expected,
            "reverse-insertion session order must be canonical (name-sorted)"
        );
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
            model: None,
            base_branch: None,
            section: None,
            stack_parent: None,
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
            model: None,
            base_branch: None,
            section: None,
            stack_parent: None,
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
            model: Some("opus".to_string()),
            base_branch: None,
            section: None,
            stack_parent: None,
        };
        validate_program_flags(&opts, "claude").unwrap();
    }

    #[test]
    fn validate_rejects_unknown_program_with_model() {
        let opts = CreateSessionOpts {
            project_path: PathBuf::from("/tmp/repo"),
            title: "test".to_string(),
            program: Some("bash".to_string()),
            initial_prompt: None,
            effort: None,
            mode: None,
            model: Some("opus".to_string()),
            base_branch: None,
            section: None,
            stack_parent: None,
        };
        let err = validate_program_flags(&opts, "bash").unwrap_err();
        assert!(err.to_string().contains("--model"));
    }

    #[test]
    fn validate_allows_codex_with_model() {
        let opts = CreateSessionOpts {
            project_path: PathBuf::from("/tmp/repo"),
            title: "test".to_string(),
            program: Some("codex".to_string()),
            initial_prompt: None,
            effort: None,
            mode: None,
            model: Some("gpt-5".to_string()),
            base_branch: None,
            section: None,
            stack_parent: None,
        };
        validate_program_flags(&opts, "codex").unwrap();
    }

    #[test]
    fn validate_allows_opencode_with_model() {
        let opts = CreateSessionOpts {
            project_path: PathBuf::from("/tmp/repo"),
            title: "test".to_string(),
            program: Some("opencode".to_string()),
            initial_prompt: None,
            effort: None,
            mode: None,
            model: Some("anthropic/claude-sonnet-4-5".to_string()),
            base_branch: None,
            section: None,
            stack_parent: None,
        };
        validate_program_flags(&opts, "opencode").unwrap();
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
            model: None,
            base_branch: None,
            section: None,
            stack_parent: None,
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

    // -- Workspace-surface service methods (Phase A) --

    use crate::config::storage::AppState as CoreState;
    use crate::config::{ConfigStore, StateStore};

    /// Build a hermetic service over TempDir-backed stores with the given
    /// config. Telemetry is disabled and tmux is isolated onto a throwaway
    /// socket dir, per the project's test-isolation rules.
    fn service_with_config(dir: &tempfile::TempDir, mut config: Config) -> CommanderService {
        config.telemetry.enabled = false;
        let tmux_tmpdir = dir.path().join("tmux");
        std::fs::create_dir_all(&tmux_tmpdir).unwrap();
        config.tmux_tmpdir = Some(tmux_tmpdir);
        let config_store = Arc::new(ConfigStore::with_path(
            config,
            dir.path().join("config.toml"),
        ));
        let store = Arc::new(StateStore::with_path(
            CoreState::default(),
            dir.path().join("state.json"),
        ));
        CommanderService::new(config_store, store, FrontendInfo::new("test", "0.0.0"))
    }

    fn service(dir: &tempfile::TempDir) -> CommanderService {
        service_with_config(dir, Config::default())
    }

    /// Seed one project with one session and return their ids.
    async fn seed_project_session(svc: &CommanderService) -> (ProjectId, SessionId) {
        let project = Project::new("repo", PathBuf::from("/tmp/repo"), "main");
        let pid = project.id;
        let session = WorktreeSession::new(
            pid,
            "task",
            "branch-task",
            PathBuf::from("/tmp/wt"),
            "claude",
        );
        let sid = session.id;
        svc.store()
            .mutate(move |state| {
                state.add_project(project);
                state.add_session(session);
            })
            .await
            .unwrap();
        (pid, sid)
    }

    #[tokio::test]
    async fn cached_tmux_ok_serves_within_ttl_and_repopulates() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);

        // A fresh cache entry is served verbatim without re-probing tmux. Poking
        // BOTH booleans and getting each back proves a genuine cache hit (the
        // real probe can only match one of them).
        for poked in [true, false] {
            *svc.tmux_ok_cache.lock().unwrap() = Some((std::time::Instant::now(), poked));
            assert_eq!(
                svc.cached_tmux_ok().await,
                poked,
                "a fresh cache entry must be served without re-probing"
            );
        }

        // An empty cache populates a fresh entry.
        *svc.tmux_ok_cache.lock().unwrap() = None;
        let _ = svc.cached_tmux_ok().await;
        let (at, _) = svc
            .tmux_ok_cache
            .lock()
            .unwrap()
            .expect("cache must be populated after a probe");
        assert!(
            at.elapsed() < TMUX_OK_CACHE_TTL,
            "a miss must store a fresh entry"
        );
    }

    #[tokio::test]
    async fn workspace_snapshot_carries_projects_and_sessions() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let (pid, sid) = seed_project_session(&svc).await;

        let snap = svc.workspace_snapshot().await.unwrap();
        assert_eq!(snap.projects.len(), 1);
        assert_eq!(snap.projects[0].id, pid);
        assert_eq!(snap.projects[0].session_ids, vec![sid]);
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.sessions[0].session_id, sid);
        assert_eq!(snap.sessions[0].worktree_path, PathBuf::from("/tmp/wt"));
        assert!(snap.sessions[0].tmux_session_name.starts_with("cc-"));
        assert!(snap.cascade_paused.is_none());
        assert!(snap.pending_comment_sessions.is_empty());
        assert!(snap.project_pull.is_empty());
        assert!(snap.operations.is_empty());
        assert_eq!(snap.server.version, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn startup_reconcile_drops_stale_creating_sessions() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let project = Project::new("repo", PathBuf::from("/tmp/repo"), "main");
        let pid = project.id;
        // A session left in `Creating` from a crashed previous run.
        let mut creating = WorktreeSession::new(
            pid,
            "half",
            "branch-half",
            PathBuf::from("/tmp/wt"),
            "claude",
        );
        creating.set_status(SessionStatus::Creating);
        let creating_id = creating.id;
        svc.store()
            .mutate(move |state| {
                state.add_project(project);
                state.add_session(creating);
            })
            .await
            .unwrap();

        svc.startup_reconcile().await.unwrap();

        let state = svc.store().read().await;
        assert!(
            state.get_session(&creating_id).is_none(),
            "stale Creating session should be dropped"
        );
        assert!(state.projects.contains_key(&pid), "project must survive");
    }

    #[tokio::test]
    async fn startup_reconcile_clears_stale_merging_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let (_pid, sid) = seed_project_session(&svc).await;
        svc.store()
            .mutate(move |state| {
                state
                    .get_session_mut(&sid)
                    .unwrap()
                    .set_status(SessionStatus::Merging);
            })
            .await
            .unwrap();

        svc.startup_reconcile().await.unwrap();

        // The transient Merging state must be cleared. With no live tmux session
        // the reconcile then marks it Stopped; either way it is no longer stuck
        // spinning in Merging.
        let state = svc.store().read().await;
        assert_ne!(
            state.get_session(&sid).unwrap().status,
            SessionStatus::Merging
        );
    }

    #[tokio::test]
    async fn mark_unread_sets_flag_for_batch() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let (_pid, sid) = seed_project_session(&svc).await;
        svc.mark_unread(vec![sid]).await.unwrap();
        let state = svc.store().read().await;
        assert!(state.get_session(&sid).unwrap().unread);
    }

    #[tokio::test]
    async fn apply_pr_results_sets_and_clears_pr_fields() {
        use crate::git::{PrCheckResult, PrInfo};
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let (_pid, sid) = seed_project_session(&svc).await;

        let info = PrInfo {
            number: 42,
            url: "https://example/pr/42".to_string(),
            state: crate::git::PrState::Open,
            is_draft: false,
            labels: vec!["x".to_string()],
            review_decision: None,
            reviewers: vec![],
            base_ref_name: Some("main".to_string()),
        };
        svc.apply_pr_results(vec![(sid, PrCheckResult::Found(info))])
            .await
            .unwrap();
        {
            let state = svc.store().read().await;
            let s = state.get_session(&sid).unwrap();
            assert_eq!(s.pr_number, Some(42));
            assert_eq!(s.pr_base_branch.as_deref(), Some("main"));
        }

        // NotFound authoritatively clears; FetchFailed would preserve.
        svc.apply_pr_results(vec![(sid, PrCheckResult::NotFound)])
            .await
            .unwrap();
        let state = svc.store().read().await;
        let s = state.get_session(&sid).unwrap();
        assert!(s.pr_number.is_none());
        assert!(s.pr_base_branch.is_none());
    }

    #[tokio::test]
    async fn workspace_snapshot_reports_paused_cascade() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let (_pid, sid) = seed_project_session(&svc).await;
        svc.store()
            .mutate(move |state| state.cascade_paused_at = Some(sid))
            .await
            .unwrap();
        let snap = svc.workspace_snapshot().await.unwrap();
        assert_eq!(snap.cascade_paused, Some(sid));
    }

    #[tokio::test]
    async fn list_projects_sorted_by_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        svc.store()
            .mutate(|state| {
                state.add_project(Project::new("zzz", PathBuf::from("/z"), "main"));
                state.add_project(Project::new("aaa", PathBuf::from("/a"), "main"));
            })
            .await
            .unwrap();
        let projects = svc.list_projects().await;
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].name, "aaa");
        assert_eq!(projects[1].name, "zzz");
    }

    #[tokio::test]
    async fn create_options_from_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = Config {
            default_program: Some("claude".to_string()),
            programs: vec![crate::config::ProgramEntry {
                label: "Claude (Opus)".to_string(),
                command: "claude --model opus".to_string(),
            }],
            sections: vec![crate::session::SectionConfig {
                name: "Open PRs".to_string(),
                ..Default::default()
            }],
            ..Config::default()
        };
        let svc = service_with_config(&dir, config);
        let opts = svc.create_options();
        // Post-programs-migration semantics: the first configured program is
        // the default; the legacy `default_program` scalar is only a fallback.
        assert_eq!(opts.default_program, "claude --model opus");
        assert_eq!(opts.programs.len(), 1);
        assert_eq!(opts.programs[0].label, "Claude (Opus)");
        assert_eq!(opts.sections, vec!["Open PRs".to_string()]);
    }

    #[tokio::test]
    async fn set_programs_persists_and_reflects_in_create_options() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        // Default config has no configured programs; create_options reports the raw
        // (empty) list — the `claude` fallback lives in the picker, not here.
        assert_eq!(svc.create_options().programs.len(), 0);

        svc.set_programs(vec![
            crate::config::ProgramEntry {
                label: "Claude (Opus)".to_string(),
                command: "claude --model opus".to_string(),
            },
            crate::config::ProgramEntry {
                label: "Shell".to_string(),
                command: "bash".to_string(),
            },
        ])
        .unwrap();

        let opts = svc.create_options();
        assert_eq!(opts.programs.len(), 2);
        assert_eq!(opts.programs[0].command, "claude --model opus");
        assert_eq!(opts.programs[1].label, "Shell");
        // First configured program is the default.
        assert_eq!(opts.default_program, "claude --model opus");

        // The change is durable: it was written to the config file on disk.
        let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(toml.contains("claude --model opus"));

        // An empty list is accepted and persists as empty.
        svc.set_programs(vec![]).unwrap();
        assert_eq!(svc.create_options().programs.len(), 0);
    }

    #[tokio::test]
    async fn rename_session_updates_title() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let (_pid, sid) = seed_project_session(&svc).await;
        svc.rename_session(&sid, "renamed").await.unwrap();
        let state = svc.store().read().await;
        assert_eq!(state.get_session(&sid).unwrap().title, "renamed");
    }

    #[tokio::test]
    async fn rename_session_rejects_empty_title() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let (_pid, sid) = seed_project_session(&svc).await;
        let err = svc.rename_session(&sid, "   ").await.unwrap_err();
        assert!(err.to_string().contains("empty"));
        // The original title is untouched.
        let state = svc.store().read().await;
        assert_eq!(state.get_session(&sid).unwrap().title, "task");
    }

    #[tokio::test]
    async fn rename_missing_session_is_not_found() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let err = svc
            .rename_session(&SessionId::new(), "x")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            crate::Error::Session(SessionError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn set_section_sets_override_and_clears_it() {
        let dir = tempfile::TempDir::new().unwrap();
        // A predicate-less section is a valid manual override target.
        let config = Config {
            sections: vec![crate::session::SectionConfig {
                name: "Parking".to_string(),
                ..Default::default()
            }],
            ..Config::default()
        };
        let svc = service_with_config(&dir, config);
        let (_pid, sid) = seed_project_session(&svc).await;

        svc.set_section(&sid, Some("Parking".to_string()))
            .await
            .unwrap();
        {
            let state = svc.store().read().await;
            let s = state.get_session(&sid).unwrap();
            assert_eq!(s.section_override.as_deref(), Some("Parking"));
            assert_eq!(s.current_section.as_deref(), Some("Parking"));
        }

        svc.set_section(&sid, None).await.unwrap();
        {
            let state = svc.store().read().await;
            let s = state.get_session(&sid).unwrap();
            assert!(s.section_override.is_none());
        }
    }

    #[tokio::test]
    async fn mark_read_clears_unread() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let (_pid, sid) = seed_project_session(&svc).await;
        svc.store()
            .mutate(move |state| {
                state.get_session_mut(&sid).unwrap().unread = true;
            })
            .await
            .unwrap();
        svc.mark_read(&sid).await.unwrap();
        let state = svc.store().read().await;
        assert!(!state.get_session(&sid).unwrap().unread);
    }

    #[tokio::test]
    async fn operation_ledger_records_and_caps() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        // Record more than the cap; the ledger keeps only the newest CAP entries.
        for i in 0..(OPERATION_LEDGER_CAP + 5) {
            svc.record_operation(
                OperationKind::Cascade,
                OperationOutcome::Succeeded {
                    detail: format!("op {i}"),
                },
            );
        }
        let ops = svc.operations_snapshot();
        assert_eq!(ops.len(), OPERATION_LEDGER_CAP);
        // Ids are monotonic; the oldest survivors dropped the first 5.
        assert_eq!(ops.first().unwrap().id, 6);
        assert!(ops.last().unwrap().id > ops.first().unwrap().id);
    }

    #[tokio::test]
    async fn record_cascade_outcome_maps_variants() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let at = SessionId::new();
        let paused = svc.record_cascade_outcome(Ok(CascadeOutcome::PausedOnConflict {
            at,
            sessions_merged: 2,
        }));
        assert!(matches!(paused.outcome, OperationOutcome::Paused { .. }));
        let done = svc.record_cascade_outcome(Ok(CascadeOutcome::Complete { sessions_merged: 3 }));
        match done.outcome {
            OperationOutcome::Succeeded { detail } => assert!(detail.contains('3')),
            other => panic!("expected Succeeded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn request_pr_refresh_notifies_without_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        // With no loop running the notification is simply dropped; the call must
        // still succeed (a manual refresh before startup is harmless).
        assert!(svc.request_pr_refresh().is_ok());
    }

    // -- Background-loop helpers + caches (Phase D) --

    #[test]
    fn unread_transition_working_to_idle_is_flagged() {
        let sid = SessionId::new();
        let prev = BTreeMap::from([(sid, AgentState::Working)]);
        let new = BTreeMap::from([(sid, AgentState::Idle)]);
        assert_eq!(detect_unread_transitions(&prev, &new), vec![sid]);
    }

    #[test]
    fn unread_transition_idle_to_idle_is_not_flagged() {
        let sid = SessionId::new();
        let prev = BTreeMap::from([(sid, AgentState::Idle)]);
        let new = BTreeMap::from([(sid, AgentState::Idle)]);
        assert!(detect_unread_transitions(&prev, &new).is_empty());
    }

    #[test]
    fn unread_transition_empty_prev_is_not_flagged() {
        // A cleared baseline (never polled, or reset after an attach) must not
        // fabricate an unread from the first observation.
        let sid = SessionId::new();
        let new_idle = BTreeMap::from([(sid, AgentState::Idle)]);
        let new_working = BTreeMap::from([(sid, AgentState::Working)]);
        assert!(detect_unread_transitions(&BTreeMap::new(), &new_idle).is_empty());
        assert!(detect_unread_transitions(&BTreeMap::new(), &new_working).is_empty());
    }

    #[test]
    fn unread_transition_flags_commander_sentinel_so_loop_must_filter() {
        // The detector treats the sentinel like any other id, so a commander
        // Working→Idle WOULD be reported; the poll loop filters it out (the
        // commander has no `WorktreeSession` to mark unread).
        let sentinel = crate::commander::commander_sentinel_id();
        let prev = BTreeMap::from([(sentinel, AgentState::Working)]);
        let new = BTreeMap::from([(sentinel, AgentState::Idle)]);
        assert_eq!(detect_unread_transitions(&prev, &new), vec![sentinel]);
    }

    #[test]
    fn poll_tick_skip_and_send_decisions() {
        // Skip only when there's nothing to detect and the commander's running
        // state is unchanged.
        assert!(poll_tick_can_skip(true, false, false));
        assert!(!poll_tick_can_skip(true, true, true));
        assert!(!poll_tick_can_skip(false, true, true));
        // A commander flip (either edge) must not be skipped.
        assert!(!poll_tick_can_skip(true, false, true));
        assert!(!poll_tick_can_skip(true, true, false));

        // Send on fresh states, or on a commander flip (so the chip can turn
        // off on the trailing edge); stay quiet otherwise.
        assert!(poll_tick_should_send(false, false, false));
        assert!(poll_tick_should_send(true, true, false));
        assert!(poll_tick_should_send(true, false, true));
        assert!(!poll_tick_should_send(true, true, true));
        assert!(!poll_tick_should_send(true, false, false));
    }

    #[test]
    fn pr_check_debounce_allows_first_and_blocks_bursts() {
        let base = std::time::Instant::now();
        let debounce = Duration::from_secs(2);
        assert!(pr_check_debounce_passed(None, base, debounce));
        assert!(!pr_check_debounce_passed(
            Some(base),
            base + Duration::from_millis(500),
            debounce
        ));
        assert!(pr_check_debounce_passed(
            Some(base),
            base + Duration::from_secs(2),
            debounce
        ));
    }

    #[test]
    fn with_commander_target_appends_sentinel_only_when_running() {
        // The poll loop and the fresh rebuild both include the commander's
        // sentinel target when it's live, so a fresh `agent_states(true)` after
        // an attach carries the commander's own state (no one-tick chip blink).
        let sentinel = crate::commander::commander_sentinel_id();

        let running = with_commander_target(Vec::new(), true, "claude");
        assert_eq!(running.len(), 1, "the sentinel target must be appended");
        assert_eq!(running[0].0, sentinel);
        assert_eq!(running[0].1, crate::commander::COMMANDER_TMUX_NAME);
        assert_eq!(running[0].2, "claude", "the commander program is carried");

        assert!(
            with_commander_target(Vec::new(), false, "claude").is_empty(),
            "no sentinel when the commander isn't running"
        );
    }

    #[tokio::test]
    async fn agent_states_fresh_recomputes_commander_running_into_cache() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        // Seed a stale `commander_running = true` in the cache, as the poll loop
        // might have left it while the commander was up. The test config has the
        // commander disabled, so the correct recomputed value is `false`.
        svc.agent_states_cache.write().await.commander_running = true;

        // A fresh call re-detects state AND recomputes commander liveness the
        // same way the poll loop does, folding the corrected flag into the
        // cache — rather than rewriting only `states` and leaving the stale
        // `commander_running` untouched.
        let fresh = svc.agent_states(true).await;
        assert!(fresh.states.is_empty());
        assert!(
            !fresh.commander_running,
            "fresh must recompute commander_running (disabled → false)"
        );
        assert!(
            !svc.agent_states_cache.read().await.commander_running,
            "the corrected flag must be folded into the cache"
        );

        // Once primed, the non-fresh read is served from the cache and so
        // reflects the corrected commander_running.
        let after = svc.agent_states(false).await;
        assert!(!after.commander_running);
    }

    #[tokio::test]
    async fn agent_states_unprimed_reports_commander_running_honestly() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        // No background loop has run and no fresh call has primed the cache, so
        // this exercises the on-demand fallback arm. With the commander disabled
        // in the test config it must report `commander_running = false`, not the
        // old hardcoded `true`.
        assert!(
            !svc.agent_states_primed
                .load(std::sync::atomic::Ordering::Relaxed),
            "precondition: the cache must be unprimed"
        );
        let snap = svc.agent_states(false).await;
        assert!(
            !snap.commander_running,
            "unprimed fallback must compute commander_running (disabled → false)"
        );
    }

    #[tokio::test]
    async fn clear_stale_agent_states_empties_cache_and_wakes_feed() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        // Seed the cache as if a running session had been detected on a prior
        // tick (the state the quiet path would otherwise leave behind).
        svc.agent_states_cache
            .write()
            .await
            .states
            .insert(SessionId::new(), AgentState::Working);
        let gen_before = svc.store.generation();

        // The last running session stopped: the quiet tick clears the cache once
        // and wakes the change-feed so clients don't see ghosts.
        assert!(svc.clear_stale_agent_states().await, "cleared stale states");
        assert!(svc.agent_states_cache.read().await.states.is_empty());
        assert!(
            svc.store.generation() > gen_before,
            "clearing must wake the change-feed"
        );

        // Idempotent: a second quiet tick on an already-empty cache is a no-op.
        let gen_after = svc.store.generation();
        assert!(!svc.clear_stale_agent_states().await);
        assert_eq!(
            svc.store.generation(),
            gen_after,
            "no redundant wake on an already-empty cache"
        );
    }

    #[tokio::test]
    async fn spawn_background_tasks_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let first = svc.spawn_background_tasks(BackgroundOpts::default());
        assert!(!first.is_empty(), "first spawn starts the loops");
        let second = svc.spawn_background_tasks(BackgroundOpts::default());
        assert!(second.is_empty(), "second spawn is a no-op");
        first.abort();
    }

    #[tokio::test]
    async fn workspace_snapshot_surfaces_project_pull_cache() {
        use crate::api::{PullBlockReason, PullStatus};
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let (pid, _sid) = seed_project_session(&svc).await;
        // The pull loop maintains this cache; inject an outcome directly to prove
        // `workspace_snapshot` surfaces it in `project_pull`.
        svc.pull_status.lock().unwrap().insert(
            pid,
            PullStatus::Blocked {
                reason: PullBlockReason::Dirty,
            },
        );
        let snap = svc.workspace_snapshot().await.unwrap();
        assert_eq!(
            snap.project_pull.get(&pid),
            Some(&PullStatus::Blocked {
                reason: PullBlockReason::Dirty
            })
        );
    }

    // -- In-session switcher revive (ensure_attachable_by_tmux_name) --

    #[tokio::test]
    async fn ensure_attachable_by_tmux_name_unknown_name_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);
        let err = svc
            .ensure_attachable_by_tmux_name("cc-deadbeef")
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                crate::Error::Session(SessionError::TmuxSessionNotFound(ref n))
                    if n == "cc-deadbeef"
            ),
            "unknown tmux name should be a TmuxSessionNotFound error, got: {err}"
        );
    }

    /// Resolution must accept either of a session's tmux names (primary or
    /// shell pair) and reach the id-based attach validation: a `Creating`
    /// session can't attach, so `InvalidState` for the right id proves the
    /// shell-pair name resolved to the session before any tmux command ran.
    #[tokio::test]
    async fn ensure_attachable_by_tmux_name_resolves_shell_pair_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = service(&dir);

        let project = Project::new("repo", PathBuf::from("/tmp/repo"), "main");
        let mut session =
            WorktreeSession::new_creating(project.id, "task", "branch-task", "claude");
        session.shell_tmux_session_name = Some("cc-task-sh".to_string());
        let sid = session.id;
        svc.store()
            .mutate(move |state| {
                state.add_project(project);
                state.add_session(session);
            })
            .await
            .unwrap();

        let err = svc
            .ensure_attachable_by_tmux_name("cc-task-sh")
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                crate::Error::Session(SessionError::InvalidState(id)) if id == sid
            ),
            "shell-pair name should resolve to the session and hit its attach guard, got: {err}"
        );
    }
}
