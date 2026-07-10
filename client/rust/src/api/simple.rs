//! Route API exposed to Flutter via flutter_rust_bridge.
//!
//! Every `pub fn` here becomes an async-callable function on the Dart side (frb
//! runs the Rust body on a worker thread). They all drive a
//! [`claude_commander_client::RemoteClient`] resolved from the opaque server
//! `handle` (see [`crate::api::registry`]); the transport, auth, ret/timeout, and
//! error classification all live in that shared crate, so this module is a thin
//! handle-resolve → call → DTO-convert layer.
//!
//! The two `health*` probes are the exception: the connect screen calls them
//! *before* a handle exists, so they build a throwaway client from the raw
//! URL/token.

use std::path::PathBuf;

use anyhow::Result;
use claude_commander_client::{RemoteClient, RemoteServerSpec, SecretString};
use claude_commander_protocol::api::{
    BranchInfo, CreateOptions, CreateSessionOpts, ProgramInfo, SessionDetail, SessionInfo,
};
use claude_commander_protocol::session::{SessionId, SessionStatus};

use crate::api::mirrors::{
    AgentStatesSnapshotDto, OperationStatusDto, PreviewDataDto, WorkspaceSnapshotDto,
};
use crate::api::registry::{
    call, map_client_err, parse_project_id, parse_session_id, with_client,
};

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    // Default utilities (logging, panic backtraces) for the bridge.
    flutter_rust_bridge::setup_default_user_utils();
}

/// Build a throwaway [`RemoteClient`] for an unauthenticated/pre-connect probe
/// from a raw base URL + optional token. An empty token means "no bearer".
fn probe_client(base_url: String, token: Option<String>) -> Result<RemoteClient> {
    let token = token.filter(|t| !t.is_empty()).map(SecretString::from);
    RemoteClient::new(RemoteServerSpec {
        name: "probe".to_string(),
        base_url,
        token,
    })
    .map_err(map_client_err)
}

/// Liveness probe: `GET {base_url}/health` (no auth). Returns true on a 2xx.
/// Called by the connect screen before any server handle exists.
pub fn health(base_url: String) -> Result<bool> {
    let client = probe_client(base_url, None)?;
    call(client.health())
}

/// Authenticated tmux probe: `GET {base_url}/api/health/tmux`. 200 → true, 503 →
/// false; a 401/403 surfaces as an auth error. Doubles as an auth check for the
/// connect screen.
pub fn health_tmux(base_url: String, token: String) -> Result<bool> {
    let client = probe_client(base_url, Some(token))?;
    call(client.health_tmux())
}

// -- Workspace surface --

/// The whole workspace snapshot (projects, sessions, cascade/pending/pull state,
/// operations ledger, server health) in one shot.
pub fn workspace_snapshot(handle: String) -> Result<WorkspaceSnapshotDto> {
    let client = with_client(&handle)?;
    Ok(call(client.workspace_snapshot())?.into())
}

/// Bulk agent-state snapshot (the commander sentinel entry is stripped by the
/// DTO). `fresh` forces a re-detection rather than a cached read.
pub fn agent_states(handle: String, fresh: bool) -> Result<AgentStatesSnapshotDto> {
    let client = with_client(&handle)?;
    Ok(call(client.agent_states(fresh))?.into())
}

/// Compatibility shim for the current session-list page: the sessions from the
/// workspace snapshot, optionally filtering out stopped ones client-side. The
/// app moves to snapshot-driven state in Phase 3; until then this keeps the list
/// page + its tests unchanged.
pub fn list_sessions(handle: String, include_stopped: bool) -> Result<Vec<SessionInfo>> {
    let client = with_client(&handle)?;
    let mut sessions = call(client.workspace_snapshot())?.sessions;
    if !include_stopped {
        sessions.retain(|s| s.status != SessionStatus::Stopped);
    }
    Ok(sessions)
}

/// A session's live detail (agent state, diff summary, pane snapshot). `query`
/// is matched loosely server-side (full id, branch, or title prefix); a 404
/// returns `None` so a deleted session reads as "gone".
pub fn get_session_detail(
    handle: String,
    query: String,
    lines: Option<u32>,
) -> Result<Option<SessionDetail>> {
    let client = with_client(&handle)?;
    call(client.session_detail(&query, lines.map(|n| n as usize)))
}

/// Preview payload for a session (agent pane + diff text/stat + shell pane).
pub fn session_preview(
    handle: String,
    id: String,
    lines: Option<u32>,
) -> Result<PreviewDataDto> {
    let client = with_client(&handle)?;
    let sid = parse_session_id(&id)?;
    Ok(call(client.session_preview(sid, lines.map(|n| n as usize)))?.into())
}

/// Preview payload for a project (diff text/stat; no agent pane).
pub fn project_preview(handle: String, id: String) -> Result<PreviewDataDto> {
    let client = with_client(&handle)?;
    let pid = parse_project_id(&id)?;
    Ok(call(client.project_preview(pid))?.into())
}

/// The raw unified diff (base → working tree) for a session's branch.
pub fn branch_diff(handle: String, id: String) -> Result<String> {
    let client = with_client(&handle)?;
    let sid = parse_session_id(&id)?;
    call(client.branch_diff(sid))
}

/// Branches for a project's base-branch picker. `fetch` runs a `git fetch` first.
pub fn list_branches(handle: String, project_id: String, fetch: bool) -> Result<Vec<BranchInfo>> {
    let client = with_client(&handle)?;
    let pid = parse_project_id(&project_id)?;
    call(client.list_branches(pid, fetch))
}

/// Options for the new-session dialog (default program, program list, sections).
pub fn create_options(handle: String) -> Result<CreateOptions> {
    let client = with_client(&handle)?;
    call(client.create_options())
}

/// Replace the server's configured program list wholesale.
pub fn set_programs(handle: String, programs: Vec<ProgramInfo>) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.set_programs(programs))
}

/// Sessions with at least one not-yet-applied review comment.
pub fn pending_comment_sessions(handle: String) -> Result<Vec<SessionId>> {
    let client = with_client(&handle)?;
    call(client.pending_comment_sessions())
}

// -- Session mutations --

/// Create a session; returns the new session's full-id string. `project_path` is
/// a path on the *server's* filesystem.
#[allow(clippy::too_many_arguments)]
pub fn create_session(
    handle: String,
    project_path: String,
    title: String,
    program: Option<String>,
    initial_prompt: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    base_branch: Option<String>,
) -> Result<String> {
    let client = with_client(&handle)?;
    let opts = CreateSessionOpts {
        project_path: PathBuf::from(project_path),
        title,
        program,
        initial_prompt,
        effort,
        mode,
        model: None,
        base_branch,
        section: None,
        stack_parent: None,
    };
    let id = call(client.create_session(opts))?;
    Ok(id.as_uuid().to_string())
}

/// Stop a running session (its worktree is kept).
pub fn kill_session(handle: String, id: String) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.kill_session(parse_session_id(&id)?))
}

/// Restart a session's program.
pub fn restart_session(handle: String, id: String) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.restart_session(parse_session_id(&id)?))
}

/// Delete a session, its branch, and its worktree.
pub fn delete_session(handle: String, id: String) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.delete_session(parse_session_id(&id)?))
}

/// Rename a session's title.
pub fn rename_session(handle: String, id: String, title: String) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.rename_session(parse_session_id(&id)?, title))
}

/// Move a session to a section; `section: None` clears the manual override.
pub fn set_section(handle: String, id: String, section: Option<String>) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.set_section(parse_session_id(&id)?, section))
}

/// Mark a session read (clears its unread indicator).
pub fn mark_read(handle: String, id: String) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.mark_read(parse_session_id(&id)?))
}

/// Mark a batch of sessions unread (unknown ids are skipped server-side).
pub fn mark_unread(handle: String, ids: Vec<String>) -> Result<()> {
    let client = with_client(&handle)?;
    let ids = ids
        .iter()
        .map(|id| parse_session_id(id))
        .collect::<Result<Vec<_>>>()?;
    call(client.mark_unread(ids))
}

/// Toggle a session's keep-alive (idle-hibernation exemption); returns the new
/// state.
pub fn toggle_keep_alive(handle: String, id: String) -> Result<bool> {
    let client = with_client(&handle)?;
    call(client.toggle_keep_alive(parse_session_id(&id)?))
}

// -- Projects --

/// Register a project (git repo) by server-side path; returns the new project's
/// full-id string.
pub fn add_project(handle: String, path: String) -> Result<String> {
    let client = with_client(&handle)?;
    let id = call(client.add_project(PathBuf::from(path)))?;
    Ok(id.as_uuid().to_string())
}

/// Remove a project (its sessions must already be gone).
pub fn remove_project(handle: String, id: String) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.remove_project(parse_project_id(&id)?))
}

/// Result of scanning a directory for git repos to register.
pub struct ScanResultDto {
    pub added: u32,
    pub skipped: u32,
}

/// Scan a server-side directory for git repos, registering any new ones.
pub fn scan_directory(handle: String, path: String) -> Result<ScanResultDto> {
    let client = with_client(&handle)?;
    let scan = call(client.scan_directory(PathBuf::from(path)))?;
    Ok(ScanResultDto {
        added: scan.added as u32,
        skipped: scan.skipped as u32,
    })
}

// -- Cascade / push-stack --

/// Cascade-merge a session down its stack; returns the recorded operation.
pub fn cascade_merge(handle: String, id: String) -> Result<OperationStatusDto> {
    let client = with_client(&handle)?;
    Ok(call(client.cascade_merge(parse_session_id(&id)?))?.into())
}

/// Push a session's whole stack; returns the recorded operation.
pub fn push_stack(handle: String, id: String) -> Result<OperationStatusDto> {
    let client = with_client(&handle)?;
    Ok(call(client.push_stack(parse_session_id(&id)?))?.into())
}

/// Resume a paused cascade (after the conflict was resolved); returns the op.
pub fn cascade_resume(handle: String) -> Result<OperationStatusDto> {
    let client = with_client(&handle)?;
    Ok(call(client.cascade_resume())?.into())
}

/// Abandon a paused cascade.
pub fn cascade_abandon(handle: String) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.cascade_abandon())
}

/// Ask the server to re-check PR metadata (runs its PR-status loop).
pub fn request_pr_refresh(handle: String) -> Result<()> {
    let client = with_client(&handle)?;
    call(client.request_pr_refresh())
}
