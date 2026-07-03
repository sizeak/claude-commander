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
    OperationStatus, PreviewData, PreviewTarget, ReviewSnapshot, SessionDetail, WorkspaceSnapshot,
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
    fail: Mutex<bool>,
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
            fail: Mutex::new(false),
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
        Ok(Vec::new())
    }

    async fn create_options(&self) -> BResult<CreateOptions> {
        self.unimpl()
    }

    async fn pending_comment_sessions(&self) -> BResult<Vec<SessionId>> {
        self.guard()?;
        Ok(Vec::new())
    }

    async fn create_session(&self, _opts: CreateSessionOpts) -> BResult<SessionId> {
        self.guard()?;
        Ok(SessionId::new())
    }

    async fn kill_session(&self, _id: SessionId) -> BResult<()> {
        self.guard()
    }

    async fn restart_session(&self, _id: SessionId) -> BResult<()> {
        self.guard()
    }

    async fn delete_session(&self, _id: SessionId) -> BResult<()> {
        self.guard()
    }

    async fn rename_session(&self, _id: SessionId, _title: String) -> BResult<()> {
        self.guard()
    }

    async fn set_section(&self, _id: SessionId, _section: Option<String>) -> BResult<()> {
        self.guard()
    }

    async fn mark_read(&self, _id: SessionId) -> BResult<()> {
        self.guard()
    }

    async fn mark_unread(&self, _ids: Vec<SessionId>) -> BResult<()> {
        self.guard()
    }

    async fn request_pr_refresh(&self) -> BResult<()> {
        self.guard()
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

    async fn list_comments(&self, _id: SessionId) -> BResult<Vec<Comment>> {
        self.guard()?;
        Ok(Vec::new())
    }

    async fn open_review(&self, _id: SessionId) -> BResult<ReviewSnapshot> {
        self.unimpl()
    }

    async fn refresh_review_if_changed(
        &self,
        _id: SessionId,
        _prev_hash: u64,
    ) -> BResult<Option<ReviewSnapshot>> {
        self.guard()?;
        Ok(None)
    }

    async fn create_comment(&self, _id: SessionId, _draft: NewComment) -> BResult<Uuid> {
        self.guard()?;
        Ok(Uuid::new_v4())
    }

    async fn delete_comment(&self, _id: SessionId, _comment_id: Uuid) -> BResult<()> {
        self.guard()
    }

    async fn apply_comments(&self, _id: SessionId) -> BResult<ApplyOutcome> {
        self.unimpl()
    }

    async fn toggle_file_reviewed(&self, _id: SessionId, _display_path: String) -> BResult<bool> {
        self.guard()?;
        Ok(false)
    }

    async fn fetch_diff_blob(
        &self,
        _id: SessionId,
        _side: DiffSide,
        _path: String,
    ) -> BResult<Vec<u8>> {
        self.guard()?;
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
