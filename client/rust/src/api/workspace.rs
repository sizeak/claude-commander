//! Workspace decisions, bridged from `claude-commander-viewmodel` and
//! `claude-commander-protocol`.
//!
//! The app shows one merged workspace list across every connected server, picks
//! which one to open on, checks a typed name before sending it, and narrows an
//! edited definition list to what each server accepts. All of these are rules
//! the TUI applies too, so they are called here rather than
//! re-implemented in Dart — the drift `api::query` was created to end.
//!
//! # Why `#[frb(sync)]`
//!
//! The merged list is read while building the fleet header and the list views,
//! and the name check runs per keystroke in a form field, so an async call would
//! make those builds asynchronous for a few microseconds of pure CPU. The inputs
//! are small (a handful of definitions and project tags per server), which is
//! the bar CLAUDE.md sets for a sync bridge.
//!
//! # Why the sources are owned DTOs
//!
//! `viewmodel::workspace::WorkspaceSource` borrows from a protocol `Snapshot`,
//! which the Dart side never holds — it holds the flattened `SnapshotDto`. So
//! Dart sends just the three inputs the merge reads, and this module rebuilds
//! borrowed sources over them. Nothing else about the rule is restated here.

use claude_commander_protocol::workspace::{
    validate_workspace_label, validate_workspace_name, StartupWorkspace,
};
pub use claude_commander_viewmodel::workspace::MergedWorkspace;
use claude_commander_viewmodel::workspace::{
    definitions_for_server as vm_definitions_for_server, merge_workspace_sources,
    resolve_startup_workspace as vm_resolve_startup,
    workspace_name_taken as vm_workspace_name_taken, WorkspaceSource,
};
use flutter_rust_bridge::frb;

use crate::api::mirrors::WorkspaceDef;

/// One server's contribution to the merged workspace list: its definitions, its
/// Main label override, and the tags on its projects (so a tag with no
/// definition still shows up as a workspace, and its projects stay reachable).
pub struct WorkspaceSourceDto {
    pub defs: Vec<WorkspaceDef>,
    pub main: Option<WorkspaceDef>,
    pub project_tags: Vec<String>,
}

/// One entry of the merged list. `name` is the identity (`None` = Main); `label`
/// is what to show, which only differs from `name` for Main.
#[frb(mirror(MergedWorkspace))]
pub struct _MergedWorkspace {
    pub name: Option<String>,
    pub label: String,
}

/// Merge every server's workspaces into the one list the app shows, in the
/// order the sources are given (the first configured server first). Main is
/// always the first entry. See `viewmodel::workspace::merge_workspace_sources`
/// for the rules.
#[frb(sync)]
pub fn merge_workspaces(sources: Vec<WorkspaceSourceDto>) -> Vec<MergedWorkspace> {
    let borrowed: Vec<WorkspaceSource<'_>> = sources
        .iter()
        .map(|s| WorkspaceSource {
            defs: &s.defs,
            main: s.main.as_ref(),
            project_tags: s.project_tags.iter().map(String::as_str).collect(),
        })
        .collect();
    merge_workspace_sources(&borrowed)
}

/// Which workspace to show, given a server's `startup_workspace` string
/// (`"last"`, `"main"` or a name), the one this device last had active, and the
/// merged list. `None` is Main — including when the wanted workspace does not
/// exist (yet).
#[frb(sync)]
pub fn resolve_startup_workspace(
    startup: String,
    last: Option<String>,
    workspaces: Vec<MergedWorkspace>,
) -> Option<String> {
    vm_resolve_startup(
        &StartupWorkspace::from(startup),
        last.as_deref(),
        &workspaces,
    )
}

/// Whether `name` is already taken — by a workspace or by Main's label,
/// trimmed and ignoring case, as a server compares them. `except` is the
/// workspace being renamed, which may keep (or re-case) its own name.
#[frb(sync)]
pub fn workspace_name_taken(
    workspaces: Vec<MergedWorkspace>,
    name: String,
    except: Option<MergedWorkspace>,
) -> bool {
    vm_workspace_name_taken(&workspaces, &name, except.as_ref())
}

/// The definition list to send one server: `wanted` (what an edit produced
/// from the merged list) narrowed to what that server accepts, given its `own`
/// definitions and the Main label it will have (`main_label`: the one being
/// sent, else its stored one). Keeps the server's own spellings and drops
/// another server's case-insensitive clash, so servers that disagree never
/// block an edit on each other.
#[frb(sync)]
pub fn definitions_for_server(
    wanted: Vec<WorkspaceDef>,
    own: Vec<WorkspaceDef>,
    main_label: Option<String>,
) -> Vec<WorkspaceDef> {
    vm_definitions_for_server(&wanted, &own, main_label.as_deref())
}

/// Why `raw` cannot be a workspace name, or `None` when it can. The message is
/// the one the server would answer a 400 with.
#[frb(sync)]
pub fn workspace_name_error(raw: String) -> Option<String> {
    validate_workspace_name(&raw).err().map(|e| e.to_string())
}

/// Why `raw` cannot be Main's display label, or `None` when it can. Laxer than
/// [`workspace_name_error`]: Main may be labelled "Main".
#[frb(sync)]
pub fn workspace_label_error(raw: String) -> Option<String> {
    validate_workspace_label(&raw).err().map(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str) -> WorkspaceDef {
        WorkspaceDef::named(name)
    }

    /// The owned-DTO adapter must feed the viewmodel the same three inputs a
    /// borrowed `WorkspaceSource::of(&snapshot)` would, in source order.
    #[test]
    fn merge_keeps_source_order_and_appends_orphan_tags() {
        let merged = merge_workspaces(vec![
            WorkspaceSourceDto {
                defs: vec![def("Work")],
                main: Some(def("Home")),
                project_tags: vec!["Work".into()],
            },
            WorkspaceSourceDto {
                defs: vec![def("Personal"), def("Work")],
                main: None,
                project_tags: vec!["Orphan".into()],
            },
        ]);
        let names: Vec<Option<&str>> = merged.iter().map(|m| m.name.as_deref()).collect();
        assert_eq!(
            names,
            [None, Some("Work"), Some("Personal"), Some("Orphan")]
        );
        assert_eq!(merged[0].label, "Home");
    }

    #[test]
    fn startup_resolution_parses_the_wire_string() {
        let merged = merge_workspaces(vec![WorkspaceSourceDto {
            defs: vec![def("Work")],
            main: None,
            project_tags: vec![],
        }]);
        let resolve = |s: &str, last: Option<&str>| {
            resolve_startup_workspace(s.into(), last.map(Into::into), merged.clone())
        };
        assert_eq!(resolve("last", Some("Work")).as_deref(), Some("Work"));
        assert_eq!(resolve("main", Some("Work")), None);
        assert_eq!(resolve("Work", None).as_deref(), Some("Work"));
        assert_eq!(
            resolve("Gone", None),
            None,
            "a missing target falls back to Main"
        );
    }

    /// Thin adapters: the rules are the viewmodel's (and tested there); these
    /// pin that the owned arguments reach it unchanged.
    #[test]
    fn name_taken_and_per_server_definitions_delegate_to_the_viewmodel() {
        let merged = merge_workspaces(vec![WorkspaceSourceDto {
            defs: vec![def("Work")],
            main: Some(def("Home")),
            project_tags: vec![],
        }]);
        assert!(workspace_name_taken(merged.clone(), " work ".into(), None));
        assert!(workspace_name_taken(merged.clone(), "HOME".into(), None));
        assert!(!workspace_name_taken(
            merged.clone(),
            "home".into(),
            Some(merged[0].clone())
        ));
        assert_eq!(
            definitions_for_server(
                vec![def("Work"), def("work"), def("Play")],
                vec![def("work")],
                Some("play".into()),
            ),
            vec![def("work")]
        );
    }

    #[test]
    fn validators_report_the_protocol_message() {
        assert_eq!(workspace_name_error("Work".into()), None);
        assert!(workspace_name_error("main".into())
            .unwrap()
            .contains("reserved"));
        assert!(workspace_name_error("  ".into()).is_some());
        assert_eq!(workspace_label_error("Main".into()), None);
    }
}
