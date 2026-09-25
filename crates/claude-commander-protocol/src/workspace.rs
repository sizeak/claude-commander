//! Workspace wire contract: definitions, the startup choice, the request bodies
//! that edit them, and the naming rules every party enforces.
//!
//! A workspace is a **label on a project** (`ProjectInfo::workspace`), not a
//! separate state tree: every workspace shares one state file, one server and
//! one set of background loops, and switching between them is a client-side
//! filter. `None` on a project means the built-in **Main** workspace, which is
//! never defined by name — it merges across servers by being untagged — so its
//! display label rides separately ([`SetWorkspacesRequest::main`]).
//!
//! A workspace's look is not part of this contract: each frontend keeps its own
//! per-workspace theme locally (the TUI in `config.toml`'s
//! `[workspace_themes]`, the Flutter app per device), keyed by name.
//!
//! The rules live here rather than in core because the server and its clients
//! must agree on them: the TUI and the Flutter app validate a name before
//! sending it, and the server re-validates what arrives. One definition, like
//! [`crate::paste`]'s size cap.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The label the built-in untagged workspace shows until someone renames it.
pub const MAIN_WORKSPACE_LABEL: &str = "Main";

/// Longest accepted workspace name / label, in characters. Workspace names sit
/// in a status-bar chip, so a sentence-length one is a mistake, not a choice.
pub const MAX_WORKSPACE_NAME_CHARS: usize = 40;

/// Names no user workspace may take, compared case-insensitively. They are the
/// two non-name spellings of [`StartupWorkspace`], so reserving them keeps that
/// string form unambiguous.
pub const RESERVED_WORKSPACE_NAMES: [&str; 2] = ["last", "main"];

/// One user-defined workspace. Serialised as a `[[workspaces]]` TOML table in
/// `config.toml` and as an element of `Snapshot::workspaces` on the wire.
///
/// FLUTTER: mirror this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDef {
    /// The workspace's name, which is also its identity: projects are tagged
    /// with it, and servers merge definitions by it (exact match).
    pub name: String,
}

impl WorkspaceDef {
    /// A definition for `name`.
    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// Which workspace a frontend opens on. `config.toml`'s
/// `startup_workspace = "last" | "main" | "<name>"`; the same string on the
/// wire.
///
/// FLUTTER: mirror this (as its string form).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum StartupWorkspace {
    /// Reopen whichever workspace this client last had active (the default).
    #[default]
    Last,
    /// Always open on the built-in Main workspace.
    Main,
    /// Always open on this named workspace (falling back to Main when it no
    /// longer exists).
    Named(String),
}

impl From<String> for StartupWorkspace {
    fn from(s: String) -> Self {
        if s.eq_ignore_ascii_case("last") {
            Self::Last
        } else if s.eq_ignore_ascii_case("main") {
            Self::Main
        } else {
            Self::Named(s)
        }
    }
}

impl From<StartupWorkspace> for String {
    fn from(s: StartupWorkspace) -> Self {
        match s {
            StartupWorkspace::Last => "last".to_string(),
            StartupWorkspace::Main => "main".to_string(),
            StartupWorkspace::Named(name) => name,
        }
    }
}

/// Body for `PUT /config/workspaces`: replaces the server's workspace
/// definitions wholesale (array order is display order). It never re-tags a
/// project — renaming and deleting have their own routes because they must
/// rewrite tags, which a replace cannot tell apart from create/remove.
///
/// FLUTTER: mirror this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetWorkspacesRequest {
    pub workspaces: Vec<WorkspaceDef>,
    /// Label for the built-in Main workspace. `None` leaves the
    /// server's current value untouched. Its `name` is a display label only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<WorkspaceDef>,
    /// `None` leaves the server's current startup choice untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_workspace: Option<StartupWorkspace>,
}

/// Body for `POST /config/workspaces/rename`: rename a definition and rewrite
/// every project tagged with `from`.
///
/// FLUTTER: mirror this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameWorkspaceRequest {
    pub from: String,
    pub to: String,
}

/// Body for `POST /config/workspaces/delete`: remove a definition and move
/// its projects to Main. A body rather than a path segment because any string
/// is a valid name — including `rename` and `delete` themselves.
///
/// FLUTTER: mirror this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteWorkspaceRequest {
    pub name: String,
}

/// Body for `PUT /projects/{id}/workspace`. `None` moves the project to Main.
///
/// FLUTTER: mirror this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetProjectWorkspace {
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Why a workspace name or definition list was refused.
///
/// Hand-written `Display` rather than `thiserror`: this crate is serde-only,
/// and the message is part of the 400 body a user sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceRejection {
    /// Empty (or whitespace-only) name.
    Empty,
    /// Longer than [`MAX_WORKSPACE_NAME_CHARS`].
    TooLong { max: usize },
    /// Contains a control character (newline, tab, escape, …).
    ControlChar,
    /// One of [`RESERVED_WORKSPACE_NAMES`].
    Reserved { name: String },
    /// Two definitions (or a definition and Main's label) share a name,
    /// compared case-insensitively.
    Duplicate { name: String },
    /// `startup_workspace` names a workspace that is not in the list.
    UnknownStartup { name: String },
}

impl fmt::Display for WorkspaceRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "workspace name must not be empty"),
            Self::TooLong { max } => {
                write!(f, "workspace name must be at most {max} characters")
            }
            Self::ControlChar => {
                write!(f, "workspace name must not contain control characters")
            }
            Self::Reserved { name } => write!(f, "\"{name}\" is a reserved workspace name"),
            Self::Duplicate { name } => write!(f, "workspace \"{name}\" is defined twice"),
            Self::UnknownStartup { name } => {
                write!(f, "startup workspace \"{name}\" is not a defined workspace")
            }
        }
    }
}

impl std::error::Error for WorkspaceRejection {}

/// Validate a display label (Main's): trimmed, non-empty, bounded, printable.
/// Returns the trimmed label.
pub fn validate_workspace_label(raw: &str) -> Result<String, WorkspaceRejection> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(WorkspaceRejection::Empty);
    }
    if name.chars().count() > MAX_WORKSPACE_NAME_CHARS {
        return Err(WorkspaceRejection::TooLong {
            max: MAX_WORKSPACE_NAME_CHARS,
        });
    }
    if name.chars().any(char::is_control) {
        return Err(WorkspaceRejection::ControlChar);
    }
    Ok(name.to_string())
}

/// Validate a user workspace name: every label rule, plus not reserved.
/// Returns the trimmed name — the spelling to store and tag with.
pub fn validate_workspace_name(raw: &str) -> Result<String, WorkspaceRejection> {
    let name = validate_workspace_label(raw)?;
    if RESERVED_WORKSPACE_NAMES
        .iter()
        .any(|r| name.eq_ignore_ascii_case(r))
    {
        return Err(WorkspaceRejection::Reserved { name });
    }
    Ok(name)
}

/// Validate one definition, returning its normalised form.
pub fn validate_workspace_def(def: &WorkspaceDef) -> Result<WorkspaceDef, WorkspaceRejection> {
    Ok(WorkspaceDef {
        name: validate_workspace_name(&def.name)?,
    })
}

/// Validate a whole [`SetWorkspacesRequest`], returning its normalised form:
/// every name valid, no two names equal case-insensitively (Main's
/// label included), and a named startup workspace present in the list.
pub fn validate_set_workspaces(
    req: &SetWorkspacesRequest,
) -> Result<SetWorkspacesRequest, WorkspaceRejection> {
    let workspaces = req
        .workspaces
        .iter()
        .map(validate_workspace_def)
        .collect::<Result<Vec<_>, _>>()?;
    let main = req
        .main
        .as_ref()
        .map(|m| {
            Ok::<_, WorkspaceRejection>(WorkspaceDef {
                name: validate_workspace_label(&m.name)?,
            })
        })
        .transpose()?;

    let mut seen: Vec<&str> = main.iter().map(|m| m.name.as_str()).collect();
    for def in &workspaces {
        if seen.iter().any(|s| s.eq_ignore_ascii_case(&def.name)) {
            return Err(WorkspaceRejection::Duplicate {
                name: def.name.clone(),
            });
        }
        seen.push(&def.name);
    }

    let startup_workspace = match &req.startup_workspace {
        Some(StartupWorkspace::Named(n)) => {
            let n = n.trim();
            if !workspaces.iter().any(|d| d.name == n) {
                return Err(WorkspaceRejection::UnknownStartup {
                    name: n.to_string(),
                });
            }
            Some(StartupWorkspace::Named(n.to_string()))
        }
        other => other.clone(),
    };

    Ok(SetWorkspacesRequest {
        workspaces,
        main,
        startup_workspace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The definition is just its name; a `color` from a pre-themes build of
    /// this branch is ignored rather than refused.
    #[test]
    fn workspace_def_is_just_a_name() {
        let json = serde_json::to_string(&WorkspaceDef::named("Work")).unwrap();
        assert_eq!(json, r#"{"name":"Work"}"#);
        let back: WorkspaceDef =
            serde_json::from_str(r##"{"name":"Work","color":"#ff8800"}"##).unwrap();
        assert_eq!(back, WorkspaceDef::named("Work"));
    }

    #[test]
    fn startup_workspace_string_forms() {
        for (value, s) in [
            (StartupWorkspace::Last, "\"last\""),
            (StartupWorkspace::Main, "\"main\""),
            (StartupWorkspace::Named("Work".into()), "\"Work\""),
        ] {
            assert_eq!(serde_json::to_string(&value).unwrap(), s);
            assert_eq!(serde_json::from_str::<StartupWorkspace>(s).unwrap(), value);
        }
        // The reserved spellings are case-insensitive, like the reservation.
        assert_eq!(
            serde_json::from_str::<StartupWorkspace>("\"LAST\"").unwrap(),
            StartupWorkspace::Last
        );
        assert_eq!(StartupWorkspace::default(), StartupWorkspace::Last);
    }

    #[test]
    fn names_are_trimmed_bounded_printable_and_unreserved() {
        assert_eq!(validate_workspace_name("  Work ").unwrap(), "Work");
        assert_eq!(
            validate_workspace_name("   "),
            Err(WorkspaceRejection::Empty)
        );
        assert!(matches!(
            validate_workspace_name(&"x".repeat(MAX_WORKSPACE_NAME_CHARS + 1)),
            Err(WorkspaceRejection::TooLong { .. })
        ));
        assert_eq!(
            validate_workspace_name("a\nb"),
            Err(WorkspaceRejection::ControlChar)
        );
        for reserved in ["last", "Main", "MAIN"] {
            assert!(matches!(
                validate_workspace_name(reserved),
                Err(WorkspaceRejection::Reserved { .. })
            ));
        }
        // Main's *label* may be "Main" — the reservation is for user names.
        assert_eq!(validate_workspace_label("Main").unwrap(), "Main");
    }

    #[test]
    fn set_workspaces_rejects_duplicates_including_mains_label() {
        let dup = SetWorkspacesRequest {
            workspaces: vec![WorkspaceDef::named("Work"), WorkspaceDef::named("work")],
            main: None,
            startup_workspace: None,
        };
        assert!(matches!(
            validate_set_workspaces(&dup),
            Err(WorkspaceRejection::Duplicate { .. })
        ));

        let clash = SetWorkspacesRequest {
            workspaces: vec![WorkspaceDef::named("Home")],
            main: Some(WorkspaceDef::named("home")),
            startup_workspace: None,
        };
        assert!(matches!(
            validate_set_workspaces(&clash),
            Err(WorkspaceRejection::Duplicate { .. })
        ));
    }

    #[test]
    fn set_workspaces_requires_a_named_startup_to_exist() {
        let req = SetWorkspacesRequest {
            workspaces: vec![WorkspaceDef::named("Work")],
            main: None,
            startup_workspace: Some(StartupWorkspace::Named("Play".into())),
        };
        assert!(matches!(
            validate_set_workspaces(&req),
            Err(WorkspaceRejection::UnknownStartup { .. })
        ));
    }

    #[test]
    fn set_workspaces_normalises_names() {
        let req = SetWorkspacesRequest {
            workspaces: vec![WorkspaceDef::named(" Work ")],
            main: Some(WorkspaceDef::named(" Home ")),
            startup_workspace: Some(StartupWorkspace::Named("Work".into())),
        };
        let ok = validate_set_workspaces(&req).unwrap();
        assert_eq!(ok.workspaces[0].name, "Work");
        assert_eq!(ok.main.unwrap().name, "Home");
    }

    /// An older client's body (no `main`, no `startup_workspace`) is valid and
    /// means "leave those alone".
    #[test]
    fn set_workspaces_minimal_body_deserializes() {
        let req: SetWorkspacesRequest =
            serde_json::from_str(r#"{"workspaces":[{"name":"Work"}]}"#).unwrap();
        assert!(req.main.is_none());
        assert!(req.startup_workspace.is_none());
    }

    #[test]
    fn set_project_workspace_null_and_absent_both_mean_main() {
        let absent: SetProjectWorkspace = serde_json::from_str("{}").unwrap();
        let null: SetProjectWorkspace = serde_json::from_str(r#"{"workspace":null}"#).unwrap();
        assert_eq!(absent.workspace, None);
        assert_eq!(null.workspace, None);
    }
}
