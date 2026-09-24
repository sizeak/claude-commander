//! Workspace bookkeeping over the persisted models: keeping `[[workspaces]]`
//! definitions and project tags consistent when a workspace is renamed,
//! deleted, or used before it was defined.
//!
//! Pure functions over `&mut Config` / `&mut AppState`, so
//! [`CommanderService`](crate::api::CommanderService) can run each one inside
//! the matching store's `mutate` and the rules stay unit-testable. The naming
//! rules themselves are the wire contract's
//! ([`claude_commander_protocol::workspace`]).

use claude_commander_protocol::workspace::{
    StartupWorkspace, WorkspaceDef, WorkspaceRejection, validate_workspace_name,
};

use crate::config::{AppState, Config};
use crate::error::{Error, SessionError};

/// A refused workspace name/definition as a core error. `InvalidName` is what
/// the server maps to a 400 and the backend seam to `InvalidRequest`, so every
/// transport reports the rejection the same way.
pub fn workspace_rejected(name: &str, e: WorkspaceRejection) -> Error {
    SessionError::InvalidName {
        name: name.to_string(),
        reason: e.to_string(),
    }
    .into()
}

/// Validate an optional tag (`None` = Main, always valid), returning the
/// normalised spelling.
pub fn validate_tag(workspace: Option<&str>) -> crate::Result<Option<String>> {
    workspace
        .map(|w| validate_workspace_name(w).map_err(|e| workspace_rejected(w, e)))
        .transpose()
}

/// Append a definition for `name` unless one exists (exact match). Returns
/// whether it appended. This is the self-heal that lets a client move a project
/// into a workspace the owning server has not heard of yet.
pub fn ensure_workspace_defined(config: &mut Config, name: &str) -> bool {
    if config.workspaces.iter().any(|d| d.name == name) {
        return false;
    }
    config.workspaces.push(WorkspaceDef::named(name));
    true
}

/// Rename definition `from` to `to` in config, carrying a pinned
/// `startup_workspace` along. `to` must be valid and must not collide
/// (case-insensitively) with another definition or Main's label. A missing
/// `from` is not an error: returns `Ok(false)` so a rename propagated to a
/// server that never had the workspace is a no-op.
pub fn rename_workspace_def(
    config: &mut Config,
    from: &str,
    to: &str,
) -> Result<bool, WorkspaceRejection> {
    let to = validate_workspace_name(to)?;
    let clashes_main = config
        .main_workspace
        .as_ref()
        .is_some_and(|m| m.name.eq_ignore_ascii_case(&to));
    let clashes_def = config
        .workspaces
        .iter()
        .any(|d| d.name != from && d.name.eq_ignore_ascii_case(&to));
    if clashes_main || clashes_def {
        return Err(WorkspaceRejection::Duplicate { name: to });
    }
    let Some(def) = config.workspaces.iter_mut().find(|d| d.name == from) else {
        return Ok(false);
    };
    def.name = to.clone();
    if config.startup_workspace == StartupWorkspace::Named(from.to_string()) {
        config.startup_workspace = StartupWorkspace::Named(to);
    }
    Ok(true)
}

/// Remove definition `name`; a startup pin on it becomes Main. Returns whether
/// a definition was removed.
pub fn delete_workspace_def(config: &mut Config, name: &str) -> bool {
    let before = config.workspaces.len();
    config.workspaces.retain(|d| d.name != name);
    if config.startup_workspace == StartupWorkspace::Named(name.to_string()) {
        config.startup_workspace = StartupWorkspace::Main;
    }
    config.workspaces.len() != before
}

/// Re-tag every project in `from` to `to` (`None` = Main). Returns how many
/// projects moved.
pub fn retag_projects(state: &mut AppState, from: &str, to: Option<&str>) -> usize {
    let mut moved = 0;
    for project in state.projects.values_mut() {
        if project.workspace.as_deref() == Some(from) {
            project.workspace = to.map(str::to_string);
            moved += 1;
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::session::Project;

    fn state_with(tags: &[Option<&str>]) -> AppState {
        let mut state = AppState::default();
        for (i, tag) in tags.iter().enumerate() {
            let mut p = Project::new(format!("p{i}"), PathBuf::from(format!("/p{i}")), "main");
            p.workspace = tag.map(str::to_string);
            state.add_project(p);
        }
        state
    }

    fn tags(state: &AppState) -> Vec<Option<String>> {
        let mut t: Vec<_> = state
            .projects
            .values()
            .map(|p| p.workspace.clone())
            .collect();
        t.sort();
        t
    }

    #[test]
    fn ensure_appends_only_a_missing_definition() {
        let mut c = Config::default();
        assert!(ensure_workspace_defined(&mut c, "Work"));
        assert!(!ensure_workspace_defined(&mut c, "Work"));
        assert_eq!(c.workspaces, vec![WorkspaceDef::named("Work")]);
    }

    fn config(defs: &[&str], startup: StartupWorkspace) -> Config {
        Config {
            workspaces: defs.iter().map(|d| WorkspaceDef::named(*d)).collect(),
            startup_workspace: startup,
            ..Config::default()
        }
    }

    #[test]
    fn rename_moves_the_definition_and_a_startup_pin() {
        let mut c = config(&["Work", "Play"], StartupWorkspace::Named("Work".into()));
        assert_eq!(rename_workspace_def(&mut c, "Work", " Job "), Ok(true));
        assert_eq!(c.workspaces[0].name, "Job");
        assert_eq!(c.startup_workspace, StartupWorkspace::Named("Job".into()));
    }

    #[test]
    fn rename_refuses_a_collision_but_not_a_case_change_of_itself() {
        let mut c = config(&["Work", "Play"], StartupWorkspace::Last);
        assert!(matches!(
            rename_workspace_def(&mut c, "Work", "play"),
            Err(WorkspaceRejection::Duplicate { .. })
        ));
        assert_eq!(rename_workspace_def(&mut c, "Work", "WORK"), Ok(true));
        c.main_workspace = Some(WorkspaceDef::named("Home"));
        assert!(rename_workspace_def(&mut c, "Play", "home").is_err());
    }

    #[test]
    fn rename_of_an_unknown_workspace_is_a_no_op() {
        let mut c = Config::default();
        assert_eq!(rename_workspace_def(&mut c, "Gone", "New"), Ok(false));
        assert!(c.workspaces.is_empty());
    }

    #[test]
    fn delete_removes_the_definition_and_unpins_startup() {
        let mut c = config(&["Work"], StartupWorkspace::Named("Work".into()));
        assert!(delete_workspace_def(&mut c, "Work"));
        assert!(c.workspaces.is_empty());
        assert_eq!(c.startup_workspace, StartupWorkspace::Main);
        assert!(!delete_workspace_def(&mut c, "Work"));
    }

    #[test]
    fn retag_moves_only_matching_projects() {
        let mut s = state_with(&[Some("Work"), Some("Play"), None, Some("Work")]);
        assert_eq!(retag_projects(&mut s, "Work", Some("Job")), 2);
        assert_eq!(
            tags(&s),
            vec![
                None,
                Some("Job".into()),
                Some("Job".into()),
                Some("Play".into())
            ]
        );
        assert_eq!(retag_projects(&mut s, "Play", None), 1);
        assert_eq!(
            tags(&s),
            vec![None, None, Some("Job".into()), Some("Job".into())]
        );
    }

    #[test]
    fn validate_tag_accepts_main_and_trims_names() {
        assert_eq!(validate_tag(None).unwrap(), None);
        assert_eq!(
            validate_tag(Some(" Work ")).unwrap().as_deref(),
            Some("Work")
        );
        assert!(validate_tag(Some("last")).is_err());
    }
}
