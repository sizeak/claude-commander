//! Workspaces in the TUI: which one is active, how it scopes the views, and the
//! operations that switch, create, rename, delete, reorder and move.
//!
//! A workspace is a label on a project (`ProjectInfo::workspace`, `None` =
//! Main). Switching is a client-side filter over the same backends, so nothing
//! here touches the background loops. Every *decision* about the data — the
//! merged cross-server list, what "in this workspace" means, waiting counts,
//! cycling — is `claude_commander_viewmodel::workspace`'s, shared with the
//! Flutter client; this module only holds the TUI's state and wiring.
//!
//! Definitions are **propagated eagerly** to every backend (create, rename,
//! delete, reorder), best effort: each backend gets the change, and the
//! ones that refuse or cannot be reached are named in a toast. Moving a project
//! writes its tag to the owning backend only, which defines the workspace there
//! if it has to (core's self-heal).

use std::borrow::Cow;

use super::*;
use claude_commander_core::api::Snapshot;
use claude_commander_protocol::workspace::{
    MAIN_WORKSPACE_LABEL, SetWorkspacesRequest, StartupWorkspace, WorkspaceDef, WorkspaceRejection,
    validate_set_workspaces, validate_workspace_label, validate_workspace_name,
};
use claude_commander_viewmodel::workspace::{self as vm, MergedWorkspace};

/// How long one backend gets to apply a propagated workspace change. The calls
/// are awaited in order (so rapid reorders can't land out of order); this bounds
/// how long a slow server can hold the UI.
const PROPAGATE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long workspace toasts stay up.
const WORKSPACE_TOAST_SECS: u64 = 4;

/// Which projects the scoped views (lists, board, Recent, status counts) show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceFilter {
    /// Fewer than two workspaces exist, so there is nothing to filter — and no
    /// snapshot clone on every rebuild.
    All,
    /// Only projects tagged with this workspace (`None` = Main).
    Only(Option<String>),
}

impl WorkspaceFilter {
    /// Whether a project tagged `project_workspace` is shown.
    pub(crate) fn admits(&self, project_workspace: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Only(active) => vm::in_workspace(project_workspace, active.as_deref()),
        }
    }

    /// `snapshot` narrowed to the filter's projects and their sessions. A
    /// session whose project is missing reads as Main (the viewmodel's rule), so
    /// it stays visible somewhere.
    pub(crate) fn scope<'a>(&self, snapshot: &'a Snapshot) -> Cow<'a, Snapshot> {
        let Self::Only(active) = self else {
            return Cow::Borrowed(snapshot);
        };
        let active = active.as_deref();
        let mut scoped = Snapshot {
            projects: Vec::new(),
            sessions: Vec::new(),
            ..snapshot.clone()
        };
        scoped.projects = vm::projects_in_workspace(snapshot, active)
            .into_iter()
            .cloned()
            .collect();
        scoped.sessions = vm::sessions_in_workspace(snapshot, active)
            .into_iter()
            .cloned()
            .collect();
        Cow::Owned(scoped)
    }
}

/// The definitions a merged list stands for, as a `PUT /config/workspaces`
/// body — the *wanted* list, which [`App::apply_workspace_op`] narrows per
/// backend with [`vm::definitions_for_server`]. Main's label is sent
/// only with `touch_main` (an edit of Main): `main: None` means "leave the
/// server's value alone", and pushing a merged label onto a server that never
/// asked for it could clash with one of its own definitions.
pub(crate) fn set_request_for(
    merged: &[MergedWorkspace],
    touch_main: bool,
) -> SetWorkspacesRequest {
    let main = touch_main
        .then(|| merged.iter().find(|w| w.is_main()))
        .flatten()
        .map(|m| WorkspaceDef::named(m.label.clone()));
    SetWorkspacesRequest {
        workspaces: merged
            .iter()
            .filter_map(|w| w.name.clone().map(WorkspaceDef::named))
            .collect(),
        main,
        startup_workspace: None,
    }
}

/// `req` narrowed to what one backend will accept, given its own snapshot:
/// see [`vm::definitions_for_server`].
pub(crate) fn set_request_for_backend(
    req: &SetWorkspacesRequest,
    snapshot: &Snapshot,
) -> SetWorkspacesRequest {
    let main_label = req
        .main
        .as_ref()
        .or(snapshot.main_workspace.as_ref())
        .map(|m| m.name.as_str());
    SetWorkspacesRequest {
        workspaces: vm::definitions_for_server(&req.workspaces, &snapshot.workspaces, main_label),
        main: req.main.clone(),
        startup_workspace: req.startup_workspace.clone(),
    }
}

/// A change to the workspace definitions, sent to every backend.
#[derive(Debug, Clone)]
pub(crate) enum WorkspaceOp {
    /// Replace the definition list (create, reorder, Main's label).
    Set(SetWorkspacesRequest),
    Rename {
        from: String,
        to: String,
    },
    Delete(String),
}

impl App {
    /// Every backend's workspaces, merged by name (local first).
    pub(super) fn merged_workspaces(&self) -> Vec<MergedWorkspace> {
        vm::merge_workspaces(self.backends.iter().map(|h| &h.view.snapshot))
    }

    /// The workspace being shown (`None` = Main).
    pub(super) fn active_workspace(&self) -> Option<String> {
        vm::effective_workspace(
            self.ui_state.active_workspace.as_deref(),
            &self.merged_workspaces(),
        )
    }

    /// What the scoped views filter to. `All` until a second workspace exists.
    pub(super) fn workspace_filter(&self) -> WorkspaceFilter {
        let merged = self.merged_workspaces();
        if !vm::workspaces_visible(&merged) {
            return WorkspaceFilter::All;
        }
        WorkspaceFilter::Only(vm::effective_workspace(
            self.ui_state.active_workspace.as_deref(),
            &merged,
        ))
    }

    /// The active workspace's display entry, or `None` while workspace UI is
    /// hidden (fewer than two workspaces).
    pub(super) fn visible_active_workspace(&self) -> Option<MergedWorkspace> {
        let merged = self.merged_workspaces();
        if !vm::workspaces_visible(&merged) {
            return None;
        }
        let active = vm::effective_workspace(self.ui_state.active_workspace.as_deref(), &merged);
        merged.into_iter().find(|w| w.name == active)
    }

    /// Whose theme the UI wears: `Some(active)` (`Some(None)` = Main) once
    /// there are workspaces to be in, `None` — the usual `[theme]` — while
    /// there is only one. With a single workspace the Theme tab edits the
    /// usual theme, so a leftover `[workspace_themes.main]` is not worn then.
    pub(super) fn theme_workspace(&self) -> Option<Option<String>> {
        self.visible_active_workspace().map(|w| w.name)
    }

    /// `workspace`'s resolved theme (`None` = Main), as it would be worn.
    /// While there is only one workspace that is the usual theme, whatever
    /// `[workspace_themes.main]` says ([`Self::theme_workspace`]).
    pub(super) fn workspace_theme(&self, workspace: Option<&str>) -> Theme {
        if self.theme_workspace().is_none() {
            return Theme::from_overrides(&self.config.theme);
        }
        crate::theme::theme_for_workspace(&self.config, workspace)
    }

    /// The accent `workspace` is drawn in outside itself — its status-bar
    /// chip, its waiting hint, its Settings swatch: its theme's `text_accent`.
    pub(super) fn workspace_accent(&self, workspace: Option<&str>) -> Color {
        self.workspace_theme(workspace).text_accent
    }

    /// Whether `workspace` (`None` = Main) wears a theme of its own on this
    /// host, rather than the usual one. Never while there is only one
    /// workspace: a leftover Main entry is not worn then, and the Theme tab
    /// (where Enter on the Workspaces tab's row leads) edits the usual theme.
    pub(super) fn workspace_theme_customised(&self, workspace: Option<&str>) -> bool {
        self.theme_workspace().is_some()
            && self
                .config
                .workspace_themes
                .get(crate::theme::workspace_theme_key(workspace))
                .is_some_and(|entry| *entry != Default::default())
    }

    /// The scope the Theme tab opens on: the active workspace once there are
    /// workspaces, else the usual theme.
    pub(super) fn default_theme_scope(&self) -> ThemeScope {
        self.visible_active_workspace()
            .map_or(ThemeScope::Usual, |w| ThemeScope::Workspace(w.name))
    }

    /// `scope` as the Theme tab can honour it: with one workspace there is only
    /// the usual theme, and a workspace that has since gone falls back to it.
    pub(super) fn effective_theme_scope(&self, scope: &ThemeScope) -> ThemeScope {
        let merged = self.merged_workspaces();
        match scope {
            ThemeScope::Workspace(name)
                if vm::workspaces_visible(&merged) && merged.iter().any(|w| &w.name == name) =>
            {
                scope.clone()
            }
            _ => ThemeScope::Usual,
        }
    }

    /// Rebuild the theme if the workspace it was built for is no longer the
    /// one being shown — a switch, a startup pick taking effect once the
    /// snapshots arrive, a rename, or a second workspace appearing.
    pub(super) fn sync_workspace_theme(&mut self) {
        if self.theme_workspace() != self.ui_state.theme_workspace {
            self.reload_theme();
        }
    }

    /// Pick the startup workspace from the local config and `tui.json`.
    pub(super) fn apply_startup_workspace(&mut self) {
        let last = self.tui_prefs.prefs().last_workspace;
        self.ui_state.active_workspace =
            vm::requested_startup_workspace(&self.config.startup_workspace, last.as_deref());
    }

    /// Show `target` (`None` = Main): remember it for `startup_workspace =
    /// "last"`, drop a board filter from the old workspace, and rebuild. With
    /// `land_on_first` the cursor goes to the first row — there is no
    /// per-workspace selection to restore.
    pub(super) async fn switch_workspace(&mut self, target: Option<String>, land_on_first: bool) {
        self.ui_state.active_workspace = target.clone();
        self.ui_state.board_filter = None;
        self.tui_prefs.set_last_workspace(target).await;
        self.refresh_list_items().await;
        if land_on_first {
            self.nav_first();
            self.update_selection();
            self.spawn_preview_update();
        }
    }

    /// `w` / "Previous workspace": cycle, wrapping. With one workspace there
    /// is nowhere to go, so say so rather than silently doing nothing.
    pub(super) async fn handle_cycle_workspace(&mut self, forward: bool) {
        let merged = self.merged_workspaces();
        if !vm::workspaces_visible(&merged) {
            self.toast("Only one workspace — create another with \"New workspace…\"".to_string());
            return;
        }
        let active = self.active_workspace();
        let target = vm::cycle_workspace(&merged, active.as_deref(), forward);
        self.switch_workspace(target, true).await;
    }

    /// `W`: the workspace picker palette.
    pub(super) async fn open_workspace_picker(&mut self) {
        self.open_quick_switch_with_mode(PaletteMode::WorkspacePicker)
            .await;
    }

    /// Rows for [`PaletteMode::WorkspacePicker`]: every merged workspace, the
    /// active one marked, each with its waiting-for-input count.
    pub(super) fn gather_workspace_picker_items(&self, filter_query: &str) -> Vec<QuickSwitchItem> {
        let merged = self.merged_workspaces();
        let active = vm::effective_workspace(self.ui_state.active_workspace.as_deref(), &merged);
        let counts = vm::waiting_counts(
            &merged,
            self.backends
                .iter()
                .map(|h| (&h.view.snapshot, &h.view.agent_states)),
        );
        merged
            .iter()
            .zip(counts)
            .filter(|(w, _)| {
                claude_commander_viewmodel::fuzzy_score(&w.label, filter_query).is_some()
            })
            .map(|(w, count)| {
                let mut label = w.label.clone();
                if count.waiting > 0 {
                    label.push_str(&format!("  \u{25cf}{} waiting", count.waiting));
                }
                if w.name == active {
                    label.push_str("  (current)");
                }
                QuickSwitchItem::Workspace {
                    name: w.name.clone(),
                    label,
                }
            })
            .collect()
    }

    /// "New workspace…": ask for a name.
    pub(super) fn handle_new_workspace(&mut self) {
        self.ui_state.modal = Modal::Input {
            title: "New Workspace".to_string(),
            prompt: "Workspace name:".to_string(),
            value: Input::default(),
            on_submit: InputAction::NewWorkspace,
            existing_branches: None,
            project_picker: None,
            program_picker: None,
            server_picker: None,
            section_picker: None,
            focus: InputFocus::Name,
            expanded: false,
            mask: false,
        };
    }

    /// Validate a new workspace name against the protocol rules and the merged
    /// list, returning the cleaned name or the reason it was refused.
    pub(super) fn check_new_workspace_name(
        &self,
        raw: &str,
    ) -> std::result::Result<String, String> {
        let name = validate_workspace_name(raw).map_err(|e| e.to_string())?;
        if vm::workspace_name_taken(&self.merged_workspaces(), &name, None) {
            return Err(WorkspaceRejection::Duplicate { name }.to_string());
        }
        Ok(name)
    }

    /// Define workspace `name` on every backend (appended last). Returns the
    /// name only when some backend took it — i.e. it is in the merged list
    /// afterwards, so switching to it shows a workspace that exists rather than
    /// recording a `last_workspace` no server has.
    pub(super) async fn create_workspace(&mut self, raw: &str) -> Option<String> {
        let name = match self.check_new_workspace_name(raw) {
            Ok(name) => name,
            Err(reason) => {
                self.toast(format!("Workspace not created: {reason}"));
                return None;
            }
        };
        let mut merged = self.merged_workspaces();
        merged.push(MergedWorkspace {
            name: Some(name.clone()),
            label: name.clone(),
        });
        self.apply_workspace_op(WorkspaceOp::Set(set_request_for(&merged, false)))
            .await;
        self.merged_workspaces()
            .iter()
            .any(|w| w.name.as_deref() == Some(name.as_str()))
            .then_some(name)
    }

    /// "Move project to workspace…": the target picker for the selected project.
    pub(super) async fn handle_move_project_to_workspace(&mut self) {
        let Some((_, project_id)) = self.ui_state.selected_project_id else {
            return;
        };
        self.open_quick_switch_with_mode(PaletteMode::MoveProjectPicker { project_id })
            .await;
    }

    /// Rows for [`PaletteMode::MoveProjectPicker`]: every workspace but the
    /// project's own.
    pub(super) fn gather_move_project_items(
        &self,
        project_id: ProjectId,
        filter_query: &str,
    ) -> Vec<QuickSwitchItem> {
        let current = self.project(project_id).and_then(|p| p.workspace.clone());
        self.merged_workspaces()
            .into_iter()
            .filter(|w| w.name != current)
            .filter(|w| claude_commander_viewmodel::fuzzy_score(&w.label, filter_query).is_some())
            .map(|w| QuickSwitchItem::ProjectWorkspace {
                project_id,
                label: w.label.clone(),
                target: w.name,
            })
            .collect()
    }

    /// Tag `project_id` with `target` on the backend that owns it. That
    /// backend defines the workspace itself if it has to; the change feed and
    /// the refresh below move the project out of (or into) the current view.
    pub(super) async fn move_project_to_workspace(
        &mut self,
        project_id: ProjectId,
        target: Option<String>,
    ) {
        let owner = self.backend_of_project(project_id);
        let project_name = self
            .project(project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "project".to_string());
        let label = self
            .merged_workspaces()
            .into_iter()
            .find(|w| w.name == target)
            .map(|w| w.label)
            .or_else(|| target.clone())
            .unwrap_or_else(|| MAIN_WORKSPACE_LABEL.to_string());
        match self
            .backend_arc(owner)
            .set_project_workspace(project_id, target)
            .await
        {
            Ok(()) => {
                self.refresh_backend_view(owner).await;
                self.config = self.service.read_config();
                self.refresh_list_items().await;
                self.toast(format!("Moved {project_name} to {label}"));
            }
            Err(e) => self.toast(format!("Failed to move {project_name}: {e}")),
        }
    }

    /// Run `op` against every backend, in backend order, then refresh the views
    /// that took it. Best effort: a backend that is degraded, refuses, or
    /// times out is named in a toast and the rest still get the change.
    ///
    /// A [`WorkspaceOp::Set`] carries the *wanted* list; each backend gets it
    /// narrowed to what it can accept ([`set_request_for_backend`]) and checked
    /// against the protocol's rule before sending, so one server whose
    /// definitions disagree with another's cannot block the edit everywhere.
    /// Returns the names of the backends that failed.
    pub(super) async fn apply_workspace_op(&mut self, op: WorkspaceOp) -> Vec<String> {
        struct Target {
            id: BackendId,
            name: String,
            backend: Arc<dyn CommanderBackend>,
            degraded: bool,
            /// What this backend is sent, or why a `Set` cannot be.
            op: std::result::Result<WorkspaceOp, WorkspaceRejection>,
        }
        let targets: Vec<Target> = self
            .backends
            .iter()
            .map(|h| Target {
                id: h.id,
                name: h.backend.descriptor().name,
                backend: h.backend.clone(),
                degraded: h.id != LOCAL_BACKEND_ID
                    && matches!(h.view.connection, ConnectionState::Degraded { .. }),
                op: match &op {
                    WorkspaceOp::Set(req) => {
                        validate_set_workspaces(&set_request_for_backend(req, &h.view.snapshot))
                            .map(WorkspaceOp::Set)
                    }
                    other => Ok(other.clone()),
                },
            })
            .collect();
        let mut failed: Vec<String> = Vec::new();
        let mut reasons: Vec<String> = Vec::new();
        let mut applied: Vec<BackendId> = Vec::new();
        for Target {
            id,
            name,
            backend,
            degraded,
            op,
        } in targets
        {
            if degraded {
                reasons.push(format!("{name} (unreachable)"));
                failed.push(name);
                continue;
            }
            let op = match op {
                Ok(op) => op,
                Err(e) => {
                    reasons.push(format!("{name} ({e})"));
                    failed.push(name);
                    continue;
                }
            };
            let call = async {
                match op {
                    WorkspaceOp::Set(req) => backend.set_workspaces(req).await,
                    WorkspaceOp::Rename { from, to } => backend.rename_workspace(from, to).await,
                    WorkspaceOp::Delete(name) => backend.delete_workspace(name).await,
                }
            };
            match tokio::time::timeout(PROPAGATE_TIMEOUT, call).await {
                Ok(Ok(())) => applied.push(id),
                Ok(Err(e)) => {
                    warn!("Workspace change refused by {name}: {e}");
                    reasons.push(format!("{name} ({e})"));
                    failed.push(name);
                }
                Err(_) => {
                    warn!("Workspace change timed out on {name}");
                    reasons.push(format!("{name} (timed out)"));
                    failed.push(name);
                }
            }
        }
        for id in applied {
            self.refresh_backend_view(id).await;
        }
        // The local backend's config changed underneath the cached copy — a
        // rename or delete also moved or dropped a `[workspace_themes]` entry.
        self.config = self.service.read_config();
        self.reload_theme();
        self.refresh_list_items().await;
        if !reasons.is_empty() {
            self.toast(format!(
                "Workspace change not applied on: {}",
                reasons.join(", ")
            ));
        }
        failed
    }

    /// Rename a workspace everywhere. Main (`from = None`) changes only its
    /// label; a named workspace's projects are re-tagged by each server. An
    /// active workspace follows its new name.
    pub(super) async fn rename_workspace_everywhere(
        &mut self,
        from: Option<String>,
        raw_to: &str,
    ) -> bool {
        let merged = self.merged_workspaces();
        let Some(current) = merged.iter().find(|w| w.name == from).cloned() else {
            return false;
        };
        let checked = match &from {
            None => validate_workspace_label(raw_to),
            Some(_) => validate_workspace_name(raw_to),
        };
        let to = match checked {
            Ok(to) => to,
            Err(e) => {
                self.toast(format!("Workspace not renamed: {e}"));
                return false;
            }
        };
        if to == current.label {
            return false;
        }
        if vm::workspace_name_taken(&merged, &to, Some(&current)) {
            self.toast(format!(
                "Workspace not renamed: {}",
                WorkspaceRejection::Duplicate { name: to }
            ));
            return false;
        }
        match from {
            None => {
                let renamed: Vec<MergedWorkspace> = merged
                    .into_iter()
                    .map(|mut w| {
                        if w.is_main() {
                            w.label = to.clone();
                        }
                        w
                    })
                    .collect();
                self.apply_workspace_op(WorkspaceOp::Set(set_request_for(&renamed, true)))
                    .await;
            }
            Some(from) => {
                let was_active = self.ui_state.active_workspace.as_deref() == Some(from.as_str());
                self.apply_workspace_op(WorkspaceOp::Rename {
                    from,
                    to: to.clone(),
                })
                .await;
                if was_active {
                    self.ui_state.active_workspace = Some(to.clone());
                    self.tui_prefs.set_last_workspace(Some(to)).await;
                    self.refresh_list_items().await;
                }
            }
        }
        true
    }

    /// Delete a named workspace everywhere; its projects return to Main. Main
    /// itself cannot be deleted.
    pub(super) async fn delete_workspace_everywhere(&mut self, name: &str) {
        let was_active = self.ui_state.active_workspace.as_deref() == Some(name);
        self.apply_workspace_op(WorkspaceOp::Delete(name.to_string()))
            .await;
        if was_active {
            self.switch_workspace(None, true).await;
        }
    }

    /// Move the named workspace at merged index `idx` one place up or down
    /// among the named workspaces (Main stays first), everywhere.
    pub(super) async fn reorder_workspace(&mut self, idx: usize, down: bool) -> Option<usize> {
        let mut merged = self.merged_workspaces();
        let other = if down { idx + 1 } else { idx.checked_sub(1)? };
        // Index 0 is Main: neither end of the swap may be it.
        if idx == 0 || other == 0 || other >= merged.len() || idx >= merged.len() {
            return None;
        }
        merged.swap(idx, other);
        self.apply_workspace_op(WorkspaceOp::Set(set_request_for(&merged, false)))
            .await;
        Some(other)
    }

    /// Change `startup_workspace` in the local config — the one this TUI
    /// honours. Other servers keep theirs (each client reads its own). Sent
    /// through the local backend with the local server's *own* definitions, so
    /// workspaces only remote servers define are not copied into the local
    /// config — except a pinned one, which the server's rule requires to be
    /// defined, so it is appended (the same self-heal as moving a project).
    pub(super) async fn set_startup_workspace(&mut self, startup: StartupWorkspace) {
        let mut workspaces = self.local_view().snapshot.workspaces.clone();
        if let StartupWorkspace::Named(name) = &startup
            && !workspaces.iter().any(|d| &d.name == name)
        {
            workspaces.push(WorkspaceDef::named(name.clone()));
        }
        let req = SetWorkspacesRequest {
            workspaces,
            main: None,
            startup_workspace: Some(startup),
        };
        match self.local_arc().set_workspaces(req).await {
            Ok(()) => {
                self.refresh_local_view().await;
                self.config = self.service.read_config();
            }
            Err(e) => self.toast(format!("Startup workspace not saved: {e}")),
        }
    }

    /// The status bar's workspace zone: a coloured chip naming the active
    /// workspace, then a `Label ●N` hint for each other workspace with
    /// sessions waiting for input. Empty while there is only one workspace.
    pub(super) fn workspace_status_spans(&self, base: Style) -> Vec<Span<'static>> {
        let merged = self.merged_workspaces();
        if !vm::workspaces_visible(&merged) {
            return Vec::new();
        }
        let active = vm::effective_workspace(self.ui_state.active_workspace.as_deref(), &merged);
        let Some(current) = merged.iter().find(|w| w.name == active) else {
            return Vec::new();
        };
        // The chip wears the active workspace's accent, so it names the theme
        // the whole UI has just changed into.
        let accent = self.theme.text_accent;
        let chip_style = base
            .bg(accent)
            .fg(contrasting_fg(accent))
            .add_modifier(Modifier::BOLD);
        let mut spans = vec![Span::styled(format!(" {} ", current.label), chip_style)];
        let counts = vm::waiting_counts(
            &merged,
            self.backends
                .iter()
                .map(|h| (&h.view.snapshot, &h.view.agent_states)),
        );
        for waiting in vm::waiting_elsewhere(&counts, active.as_deref()) {
            let label = merged
                .iter()
                .find(|w| w.name == waiting.name)
                .map_or(MAIN_WORKSPACE_LABEL, |w| w.label.as_str());
            // Each hint in *its* workspace's accent, as its chip would be.
            spans.push(Span::styled(
                format!(" {label} "),
                base.fg(self
                    .theme
                    .on_status_bar(self.workspace_accent(waiting.name.as_deref()))),
            ));
            spans.push(Span::styled(
                format!("\u{25cf}{}", waiting.waiting),
                base.fg(self.theme.on_status_bar(self.theme.agent_waiting)),
            ));
        }
        spans
    }

    /// A transient status-bar message.
    pub(super) fn toast(&mut self, message: String) {
        self.ui_state.status_message = Some((
            message,
            Instant::now() + Duration::from_secs(WORKSPACE_TOAST_SECS),
        ));
    }
}

/// Black or white, whichever reads on `bg`. Named and indexed colours are
/// judged by their standard xterm value; `Reset` (the terminal's own
/// background, unknown) gets black.
pub(crate) fn contrasting_fg(bg: Color) -> Color {
    let Some((r, g, b)) = crate::theme::color_to_hex(bg)
        .as_deref()
        .and_then(crate::widgets::parse_hex_color)
        .and_then(|c| match c {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            _ => None,
        })
    else {
        return Color::Black;
    };
    let luma = 299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b);
    if luma > 128_000 {
        Color::Black
    } else {
        Color::White
    }
}
