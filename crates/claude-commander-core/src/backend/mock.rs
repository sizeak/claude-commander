//! An in-memory [`CommanderBackend`] test double.
//!
//! [`MockBackend`] serves a fixed [`WorkspaceSnapshot`] + agent states and
//! exposes a drivable connection watch + change feed, so multi-backend TUI tests
//! can stand up a fake remote server without any network or tmux. Mutations are
//! accepted as no-ops (tests assert on rendering/selection, not persistence);
//! when constructed "failing", every query returns
//! [`Unavailable`](BackendError::Unavailable) so a degraded backend can be
//! exercised.

use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::watch;
use uuid::Uuid;

use crate::api::{
    AgentStatesSnapshot, BranchInfo, CreateOptions, CreateSessionOpts, DiffSide, NewComment,
    OperationStatus, PreviewData, PreviewTarget, ProgramInfo, ReviewSnapshot, SessionDetail,
    WorkspaceSnapshot,
};
use crate::comment::{ApplyOutcome, Comment};
use crate::session::{ProjectId, ScanResult, SessionId};

use super::{
    AttachConnection, AttachKind, BResult, BackendCapabilities, BackendChangeFeed,
    BackendDescriptor, BackendError, BackendKind, CommanderBackend, ConnectionState,
};

/// See the module docs.
pub struct MockBackend {
    descriptor: BackendDescriptor,
    snapshot: Mutex<WorkspaceSnapshot>,
    states: Mutex<AgentStatesSnapshot>,
    branches: Mutex<Vec<BranchInfo>>,
    fail: Mutex<bool>,
    /// Sessions passed to [`Self::delete_session`], for call-recording asserts.
    deleted: Mutex<Vec<SessionId>>,
    /// Options passed to [`Self::create_session`], for call-recording asserts.
    created: Mutex<Vec<CreateSessionOpts>>,
    /// Sessions passed to [`Self::reconcile_one_section`], for routing asserts.
    reconciled: Mutex<Vec<SessionId>>,
    /// Sessions passed to [`Self::restart_session`], for routing asserts.
    restarted: Mutex<Vec<SessionId>>,
    /// `(session, program)` pairs passed to [`Self::change_program`], for
    /// call-recording asserts.
    program_changes: Mutex<Vec<(SessionId, String)>>,
    /// Count of [`Self::request_pr_refresh`] calls, for call-recording asserts.
    pr_refresh_calls: Mutex<usize>,
    /// Sessions passed to [`Self::mark_read`], for call-recording asserts.
    read_marked: Mutex<Vec<SessionId>>,
    /// When set, [`Self::mark_read`] awaits this gate before recording — lets a
    /// test hold the call open to prove the caller doesn't block on it.
    mark_read_gate: Mutex<Option<std::sync::Arc<tokio::sync::Notify>>>,
    /// Sessions passed to [`Self::refresh_review_if_changed`], for routing asserts.
    review_refreshed: Mutex<Vec<SessionId>>,
    /// Sessions passed to [`Self::list_comments`], for routing asserts.
    listed_comments: Mutex<Vec<SessionId>>,
    /// Sessions passed to [`Self::create_comment`], for routing asserts.
    created_comments: Mutex<Vec<SessionId>>,
    /// Sessions passed to [`Self::apply_comments`], for routing asserts.
    applied_comments: Mutex<Vec<SessionId>>,
    /// `(session, display_path)` passed to [`Self::toggle_file_reviewed`].
    toggled_reviewed: Mutex<Vec<(SessionId, String)>>,
    /// `(session, side, path)` passed to [`Self::fetch_diff_blob`].
    fetched_blobs: Mutex<Vec<(SessionId, DiffSide, String)>>,
    conn_tx: watch::Sender<ConnectionState>,
    conn_rx: watch::Receiver<ConnectionState>,
    gen_tx: watch::Sender<u64>,
    gen_rx: watch::Receiver<u64>,
}

impl MockBackend {
    /// A remote-kind mock named `name` serving `snapshot`, initially connected.
    pub fn new(name: impl Into<String>, snapshot: WorkspaceSnapshot) -> Self {
        let (conn_tx, conn_rx) = watch::channel(ConnectionState::Connected);
        let (gen_tx, gen_rx) = watch::channel(0u64);
        Self {
            descriptor: BackendDescriptor {
                name: name.into(),
                kind: BackendKind::Remote,
            },
            snapshot: Mutex::new(snapshot),
            states: Mutex::new(AgentStatesSnapshot {
                states: Default::default(),
                commander_running: false,
            }),
            branches: Mutex::new(Vec::new()),
            fail: Mutex::new(false),
            deleted: Mutex::new(Vec::new()),
            created: Mutex::new(Vec::new()),
            reconciled: Mutex::new(Vec::new()),
            restarted: Mutex::new(Vec::new()),
            program_changes: Mutex::new(Vec::new()),
            pr_refresh_calls: Mutex::new(0),
            read_marked: Mutex::new(Vec::new()),
            mark_read_gate: Mutex::new(None),
            review_refreshed: Mutex::new(Vec::new()),
            listed_comments: Mutex::new(Vec::new()),
            created_comments: Mutex::new(Vec::new()),
            applied_comments: Mutex::new(Vec::new()),
            toggled_reviewed: Mutex::new(Vec::new()),
            fetched_blobs: Mutex::new(Vec::new()),
            conn_tx,
            conn_rx,
            gen_tx,
            gen_rx,
        }
    }

    /// Drive the connection watch to `state` (tests assert the header/gating
    /// react). Also bumps the change feed so the TUI re-reads.
    pub fn set_connection(&self, state: ConnectionState) {
        let _ = self.conn_tx.send(state);
        self.gen_tx.send_modify(|g| *g = g.wrapping_add(1));
    }

    /// Make every query fail with `Unavailable` (a downed server).
    pub fn set_failing(&self, fail: bool) {
        *self.fail.lock().unwrap() = fail;
    }

    /// Set the branch list served by [`Self::list_branches`].
    pub fn set_branches(&self, branches: Vec<BranchInfo>) {
        *self.branches.lock().unwrap() = branches;
    }

    /// Session ids passed to [`Self::delete_session`], in call order.
    pub fn deleted_sessions(&self) -> Vec<SessionId> {
        self.deleted.lock().unwrap().clone()
    }

    /// Options passed to [`Self::create_session`], in call order.
    pub fn created_sessions(&self) -> Vec<CreateSessionOpts> {
        self.created.lock().unwrap().clone()
    }

    /// Sessions passed to [`Self::reconcile_one_section`], in call order.
    pub fn reconciled_sessions(&self) -> Vec<SessionId> {
        self.reconciled.lock().unwrap().clone()
    }

    /// `(session, program)` pairs passed to [`Self::change_program`], in call order.
    pub fn program_changes(&self) -> Vec<(SessionId, String)> {
        self.program_changes.lock().unwrap().clone()
    }

    /// Sessions passed to [`Self::restart_session`], in call order.
    pub fn restarted_sessions(&self) -> Vec<SessionId> {
        self.restarted.lock().unwrap().clone()
    }

    /// How many times [`Self::request_pr_refresh`] has been called.
    pub fn pr_refresh_count(&self) -> usize {
        *self.pr_refresh_calls.lock().unwrap()
    }

    /// Sessions passed to [`Self::mark_read`], in call order.
    pub fn read_marked_sessions(&self) -> Vec<SessionId> {
        self.read_marked.lock().unwrap().clone()
    }

    /// Gate [`Self::mark_read`] on the returned [`Notify`]: the call parks until
    /// the test calls `notify_one`, so a test can prove the caller returned
    /// without awaiting the mark-read. Returns the notify handle to release with.
    pub fn block_mark_read(&self) -> std::sync::Arc<tokio::sync::Notify> {
        let gate = std::sync::Arc::new(tokio::sync::Notify::new());
        *self.mark_read_gate.lock().unwrap() = Some(gate.clone());
        gate
    }

    /// Sessions passed to [`Self::refresh_review_if_changed`], in call order.
    pub fn review_refreshed_sessions(&self) -> Vec<SessionId> {
        self.review_refreshed.lock().unwrap().clone()
    }

    /// Sessions passed to [`Self::list_comments`], in call order.
    pub fn listed_comment_sessions(&self) -> Vec<SessionId> {
        self.listed_comments.lock().unwrap().clone()
    }

    /// Sessions passed to [`Self::create_comment`], in call order.
    pub fn created_comment_sessions(&self) -> Vec<SessionId> {
        self.created_comments.lock().unwrap().clone()
    }

    /// Sessions passed to [`Self::apply_comments`], in call order.
    pub fn applied_comment_sessions(&self) -> Vec<SessionId> {
        self.applied_comments.lock().unwrap().clone()
    }

    /// `(session, display_path)` pairs passed to [`Self::toggle_file_reviewed`].
    pub fn toggled_reviewed_files(&self) -> Vec<(SessionId, String)> {
        self.toggled_reviewed.lock().unwrap().clone()
    }

    /// `(session, side, path)` tuples passed to [`Self::fetch_diff_blob`].
    pub fn fetched_diff_blobs(&self) -> Vec<(SessionId, DiffSide, String)> {
        self.fetched_blobs.lock().unwrap().clone()
    }

    fn guard(&self) -> BResult<()> {
        if *self.fail.lock().unwrap() {
            Err(BackendError::Unavailable {
                reason: "mock backend is failing".to_string(),
            })
        } else {
            Ok(())
        }
    }

    /// Methods the multi-backend TUI tests never exercise return this rather
    /// than fabricating DTOs that lack a `Default`.
    fn unimpl<T>(&self) -> BResult<T> {
        Err(BackendError::Unavailable {
            reason: "unimplemented in mock".to_string(),
        })
    }
}

#[async_trait]
impl CommanderBackend for MockBackend {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            open_editor: false,
            switcher_popup: false,
            commander_session: false,
            shell_toggle: false,
            client_side_image_paste: false,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn change_feed(&self) -> BackendChangeFeed {
        BackendChangeFeed::new(self.gen_rx.clone())
    }

    fn connection_watch(&self) -> Option<watch::Receiver<ConnectionState>> {
        Some(self.conn_rx.clone())
    }

    async fn workspace_snapshot(&self) -> BResult<WorkspaceSnapshot> {
        self.guard()?;
        Ok(self.snapshot.lock().unwrap().clone())
    }

    async fn agent_states(&self, _fresh: bool) -> BResult<AgentStatesSnapshot> {
        self.guard()?;
        Ok(self.states.lock().unwrap().clone())
    }

    async fn session_detail(
        &self,
        _query: &str,
        _lines: Option<usize>,
    ) -> BResult<Option<SessionDetail>> {
        self.guard()?;
        Ok(None)
    }

    async fn preview(&self, _target: PreviewTarget) -> BResult<PreviewData> {
        self.unimpl()
    }

    async fn branch_diff(&self, _id: SessionId) -> BResult<String> {
        self.unimpl()
    }

    async fn list_branches(&self, _project: ProjectId, _fetch: bool) -> BResult<Vec<BranchInfo>> {
        self.guard()?;
        Ok(self.branches.lock().unwrap().clone())
    }

    async fn create_options(&self) -> BResult<CreateOptions> {
        self.unimpl()
    }

    async fn set_programs(&self, _programs: Vec<ProgramInfo>) -> BResult<()> {
        self.unimpl()
    }

    async fn pending_comment_sessions(&self) -> BResult<Vec<SessionId>> {
        self.guard()?;
        Ok(Vec::new())
    }

    async fn create_session(&self, opts: CreateSessionOpts) -> BResult<SessionId> {
        self.guard()?;
        self.created.lock().unwrap().push(opts);
        let id = SessionId::new();
        // Surface the new session in the served snapshot so a TUI refresh picks
        // it up (mirrors a real backend committing the row), by cloning an
        // existing session's shape under the fresh id. Leaves the snapshot
        // unchanged when the mock has no template session.
        {
            let mut snap = self.snapshot.lock().unwrap();
            if let Some(mut created) = snap.sessions.first().cloned() {
                let project_id = created.project_id;
                created.session_id = id;
                created.id = id.to_string();
                created.title = "created".to_string();
                snap.sessions.push(created);
                // The project-grouped tree renders from each project's
                // `session_ids`, so register the new session there too.
                if let Some(project) = snap.projects.iter_mut().find(|p| p.id == project_id) {
                    project.session_ids.push(id);
                }
            }
        }
        Ok(id)
    }

    async fn reconcile_one_section(&self, id: SessionId) -> BResult<()> {
        self.guard()?;
        self.reconciled.lock().unwrap().push(id);
        Ok(())
    }

    async fn kill_session(&self, _id: SessionId) -> BResult<()> {
        self.guard()
    }

    async fn restart_session(&self, id: SessionId) -> BResult<()> {
        self.guard()?;
        self.restarted.lock().unwrap().push(id);
        Ok(())
    }

    async fn delete_session(&self, id: SessionId) -> BResult<()> {
        self.guard()?;
        self.deleted.lock().unwrap().push(id);
        Ok(())
    }

    async fn rename_session(&self, _id: SessionId, _title: String) -> BResult<()> {
        self.guard()
    }

    async fn change_program(&self, id: SessionId, program: String) -> BResult<()> {
        self.guard()?;
        self.program_changes.lock().unwrap().push((id, program));
        Ok(())
    }

    async fn set_section(&self, _id: SessionId, _section: Option<String>) -> BResult<()> {
        self.guard()
    }

    async fn toggle_keep_alive(&self, _id: SessionId) -> BResult<bool> {
        self.guard()?;
        Ok(true)
    }

    async fn mark_read(&self, id: SessionId) -> BResult<()> {
        self.guard()?;
        let gate = self.mark_read_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        self.read_marked.lock().unwrap().push(id);
        Ok(())
    }

    async fn mark_unread(&self, _ids: Vec<SessionId>) -> BResult<()> {
        self.guard()
    }

    async fn request_pr_refresh(&self) -> BResult<()> {
        self.guard()?;
        *self.pr_refresh_calls.lock().unwrap() += 1;
        Ok(())
    }

    async fn add_project(&self, _path: std::path::PathBuf) -> BResult<ProjectId> {
        self.guard()?;
        Ok(ProjectId::new())
    }

    async fn remove_project(&self, _id: ProjectId) -> BResult<()> {
        self.guard()
    }

    async fn scan_directory(&self, _dir: std::path::PathBuf) -> BResult<ScanResult> {
        self.unimpl()
    }

    async fn cascade_merge(&self, _id: SessionId) -> BResult<OperationStatus> {
        self.unimpl()
    }

    async fn cascade_resume(&self) -> BResult<OperationStatus> {
        self.unimpl()
    }

    async fn cascade_abandon(&self) -> BResult<()> {
        self.guard()
    }

    async fn push_stack(&self, _id: SessionId) -> BResult<OperationStatus> {
        self.unimpl()
    }

    async fn list_comments(&self, id: SessionId) -> BResult<Vec<Comment>> {
        self.guard()?;
        self.listed_comments.lock().unwrap().push(id);
        Ok(Vec::new())
    }

    async fn open_review(&self, _id: SessionId) -> BResult<ReviewSnapshot> {
        self.unimpl()
    }

    async fn refresh_review_if_changed(
        &self,
        id: SessionId,
        _prev_hash: u64,
    ) -> BResult<Option<ReviewSnapshot>> {
        self.guard()?;
        self.review_refreshed.lock().unwrap().push(id);
        Ok(None)
    }

    async fn create_comment(&self, id: SessionId, _draft: NewComment) -> BResult<Uuid> {
        self.guard()?;
        self.created_comments.lock().unwrap().push(id);
        Ok(Uuid::new_v4())
    }

    async fn delete_comment(&self, _id: SessionId, _comment_id: Uuid) -> BResult<()> {
        self.guard()
    }

    async fn apply_comments(&self, id: SessionId) -> BResult<ApplyOutcome> {
        self.guard()?;
        self.applied_comments.lock().unwrap().push(id);
        Ok(ApplyOutcome::Nothing)
    }

    async fn toggle_file_reviewed(&self, id: SessionId, display_path: String) -> BResult<bool> {
        self.guard()?;
        self.toggled_reviewed
            .lock()
            .unwrap()
            .push((id, display_path));
        Ok(false)
    }

    async fn fetch_diff_blob(
        &self,
        id: SessionId,
        side: DiffSide,
        path: String,
    ) -> BResult<Vec<u8>> {
        self.guard()?;
        self.fetched_blobs.lock().unwrap().push((id, side, path));
        Ok(Vec::new())
    }

    async fn attach(
        &self,
        _id: SessionId,
        _cols: u16,
        _rows: u16,
        _kind: AttachKind,
    ) -> BResult<Box<dyn AttachConnection>> {
        Err(BackendError::Unavailable {
            reason: "mock backend does not support attach".to_string(),
        })
    }
}
