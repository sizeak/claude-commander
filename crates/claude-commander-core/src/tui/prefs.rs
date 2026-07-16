//! TUI-owned UI preferences, persisted to `tui.json`.
//!
//! These are frontend preferences (which session-list view, the last-selected
//! row, the left-pane width) that belong to the operator's terminal, not to any
//! backend's shared state. Keeping them out of `state.json` means a remote
//! backend's session data can never land in a file the TUI persists, and a
//! future multi-backend TUI keeps a single local prefs file regardless of how
//! many servers it drives.
//!
//! The store reuses [`StateStore`](crate::config::StateStore)'s crash-safe
//! write plumbing — advisory `flock` + write-to-temp + atomic rename — via the
//! shared [`atomic_write`]/[`open_lock_file`] helpers, rather than duplicating
//! it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nix::fcntl::{Flock, FlockArg};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::ViewMode;
use crate::config::store::{atomic_write, open_lock_file};
use crate::error::{ConfigError, Result};
use crate::session::{ProjectId, SessionId};

/// Current `tui.json` schema version. Bump this and extend [`migrate_prefs_schema`]
/// whenever an upgrade must rewrite existing prefs exactly once.
const CURRENT_PREFS_SCHEMA: u32 = 1;

/// Persisted TUI preferences. Every field is optional so an absent/empty
/// `tui.json` (or a field a newer version added) loads as "no preference".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiPrefs {
    /// Schema version of this prefs file, used to gate one-time migrations.
    /// Absent (a file written before this field existed) deserialises to 0.
    /// See [`migrate_prefs_schema`].
    #[serde(default)]
    pub schema_version: u32,
    /// Last-selected session-list view (Project / Sections / Stacks). `None`
    /// means the user never chose one, so the TUI picks a section-aware default.
    #[serde(default)]
    pub view_mode: Option<ViewMode>,
    /// Last-focused session, restored on the next launch.
    #[serde(default)]
    pub last_selected_session: Option<SessionId>,
    /// Last-focused project (used when no session was focused).
    #[serde(default)]
    pub last_selected_project: Option<ProjectId>,
    /// Name of the backend that owned the last selection. Stored by *name*, not
    /// by positional `BackendId`, because config order can change between
    /// launches; restore resolves the name back to a backend and falls back to
    /// the local backend when it no longer exists. `None`/absent means the local
    /// backend (back-compat with prefs written before multi-backend).
    #[serde(default)]
    pub last_selected_backend: Option<String>,
    /// Persisted left-pane width, as a percentage of terminal width.
    #[serde(default)]
    pub left_pane_pct: Option<u16>,
}

/// The UI-pref fields as they lived in `state.json` before `tui.json` existed,
/// read directly (not through [`StateStore`](crate::config::StateStore)) for the
/// one-time migration. Deserialised leniently: any other `state.json` fields are
/// ignored, and a missing/corrupt file yields defaults.
#[derive(Debug, Default, Deserialize)]
struct LegacyStatePrefs {
    #[serde(default)]
    view_mode: Option<ViewMode>,
    #[serde(default)]
    last_selected_session: Option<SessionId>,
    #[serde(default)]
    last_selected_project: Option<ProjectId>,
    #[serde(default)]
    left_pane_pct: Option<u16>,
}

impl From<LegacyStatePrefs> for TuiPrefs {
    fn from(l: LegacyStatePrefs) -> Self {
        Self {
            // Legacy state.json predates the schema field, so start at 0 and let
            // `migrate_prefs_schema` bring it forward on this same load.
            schema_version: 0,
            view_mode: l.view_mode,
            last_selected_session: l.last_selected_session,
            last_selected_project: l.last_selected_project,
            // Legacy state.json predates multi-backend; its selection is local.
            last_selected_backend: None,
            left_pane_pct: l.left_pane_pct,
        }
    }
}

/// Concurrent-safe `tui.json` persistence with an in-memory cache. Cheap to hold
/// on `App`; writes go through a blocking `flock`+atomic-rename cycle so two TUI
/// instances sharing a data dir can't corrupt the file.
pub struct TuiPrefsStore {
    path: PathBuf,
    lock_path: PathBuf,
    cache: Mutex<TuiPrefs>,
}

impl TuiPrefsStore {
    /// Load `tui.json` from `data_dir`. On the first launch after upgrade
    /// (`tui.json` absent) the UI-pref fields are migrated out of the sibling
    /// `state.json` and written through, so the user keeps their view mode,
    /// selection, and pane width exactly once.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("tui.json");
        let lock_path = path.with_extension("json.lock");

        let existed = path.exists();
        let mut prefs = if existed {
            read_prefs(&path)
        } else {
            migrate_from_state_json(&data_dir.join("state.json"))
        };

        // Persist through when we either lifted legacy UI prefs out of
        // `state.json` (first run after `tui.json` was introduced) or applied a
        // schema migration (e.g. resetting to the new SectionStacks default).
        // Either way the single write makes each migration a one-time event even
        // if the user changes nothing this session.
        let migrated_legacy = !existed && prefs != TuiPrefs::default();
        let migrated_schema = migrate_prefs_schema(&mut prefs);
        if (migrated_legacy || migrated_schema)
            && let Err(e) = persist(&path, &lock_path, &prefs)
        {
            warn!("Failed to persist migrated TUI prefs: {e}");
        }

        Self {
            path,
            lock_path,
            cache: Mutex::new(prefs),
        }
    }

    /// A snapshot of the current preferences.
    pub fn prefs(&self) -> TuiPrefs {
        self.cache.lock().expect("tui prefs mutex poisoned").clone()
    }

    /// Update the cached prefs with `f`, then persist the result off-thread.
    /// Persistence failures are logged, not surfaced — a lost UI-pref write is
    /// cosmetic.
    async fn update(&self, f: impl FnOnce(&mut TuiPrefs)) {
        let snapshot = {
            let mut guard = self.cache.lock().expect("tui prefs mutex poisoned");
            f(&mut guard);
            guard.clone()
        };
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        match tokio::task::spawn_blocking(move || persist(&path, &lock_path, &snapshot)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("Failed to persist TUI prefs: {e}"),
            Err(e) => warn!("TUI prefs persist task panicked: {e}"),
        }
    }

    /// Persist the chosen session-list view.
    pub async fn set_view_mode(&self, view: ViewMode) {
        self.update(|p| p.view_mode = Some(view)).await;
    }

    /// Persist the last-focused session/project selection, qualified by the
    /// owning backend's name (so it survives config-order changes).
    pub async fn set_selection(
        &self,
        session: Option<SessionId>,
        project: Option<ProjectId>,
        backend: Option<String>,
    ) {
        self.update(|p| {
            p.last_selected_session = session;
            p.last_selected_project = project;
            p.last_selected_backend = backend;
        })
        .await;
    }

    /// Persist the left-pane width.
    pub async fn set_left_pane_pct(&self, pct: u16) {
        self.update(|p| p.left_pane_pct = Some(pct)).await;
    }
}

/// Read `tui.json`, falling back to defaults (with a warning) on any read/parse
/// error rather than losing the whole UI over a corrupt prefs file.
fn read_prefs(path: &Path) -> TuiPrefs {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            warn!("Failed to parse {}: {e}; using defaults", path.display());
            TuiPrefs::default()
        }),
        Err(e) => {
            warn!("Failed to read {}: {e}; using defaults", path.display());
            TuiPrefs::default()
        }
    }
}

/// Extract the legacy UI-pref fields from `state.json` for migration. A missing
/// or unparseable file yields empty prefs (nothing to migrate).
fn migrate_from_state_json(state_path: &Path) -> TuiPrefs {
    let Ok(content) = std::fs::read_to_string(state_path) else {
        return TuiPrefs::default();
    };
    serde_json::from_str::<LegacyStatePrefs>(&content)
        .unwrap_or_default()
        .into()
}

/// Apply one-time schema migrations to `prefs` in place, returning `true` if
/// anything changed (so the caller persists the result and the migration runs
/// exactly once). Idempotent: prefs already at [`CURRENT_PREFS_SCHEMA`] are left
/// untouched.
fn migrate_prefs_schema(prefs: &mut TuiPrefs) -> bool {
    if prefs.schema_version >= CURRENT_PREFS_SCHEMA {
        return false;
    }
    // v0 -> v1: SectionStacks became the default session-list view. Drop any
    // previously-persisted view choice so every existing user re-defers to the
    // new section-aware default exactly once; their next manual toggle
    // re-persists a choice as normal.
    if prefs.schema_version < 1 {
        prefs.view_mode = None;
    }
    prefs.schema_version = CURRENT_PREFS_SCHEMA;
    true
}

/// Write `prefs` to `path` under an exclusive advisory lock, reusing the
/// [`StateStore`](crate::config::StateStore) atomic-write plumbing.
fn persist(path: &Path, lock_path: &Path, prefs: &TuiPrefs) -> Result<()> {
    let lock_file = open_lock_file(lock_path)?;
    let _lock = Flock::lock(lock_file, FlockArg::LockExclusive).map_err(|(_, e)| {
        ConfigError::SaveFailed(format!("Failed to acquire tui.json lock: {e}"))
    })?;
    atomic_write(path, prefs)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn absent_tui_json_migrates_prefs_out_of_state_json() {
        let dir = TempDir::new().unwrap();
        // Seed a state.json carrying the legacy UI prefs (and unrelated fields
        // the migration must ignore). The legacy state predates the schema
        // field, so the schema migration also runs and resets the view choice.
        std::fs::write(
            dir.path().join("state.json"),
            r#"{
                "projects": {},
                "sessions": {},
                "view_mode": "ProjectGrouped",
                "left_pane_pct": 42
            }"#,
        )
        .unwrap();

        // First load with no tui.json lifts the values across, then the v0->v1
        // schema migration drops the view choice so this user re-defers to the
        // new SectionStacks default. Unrelated prefs (pane width) survive.
        let store = TuiPrefsStore::load(dir.path());
        let prefs = store.prefs();
        assert_eq!(prefs.view_mode, None);
        assert_eq!(prefs.left_pane_pct, Some(42));
        assert_eq!(prefs.schema_version, CURRENT_PREFS_SCHEMA);

        // …and writes tui.json so a second, independent load reads it back
        // without touching state.json or re-running the migration.
        assert!(dir.path().join("tui.json").exists());
        let reloaded = TuiPrefsStore::load(dir.path()).prefs();
        assert_eq!(reloaded.view_mode, None);
        assert_eq!(reloaded.left_pane_pct, Some(42));
        assert_eq!(reloaded.schema_version, CURRENT_PREFS_SCHEMA);
    }

    #[test]
    fn missing_both_files_yields_current_schema_default_prefs() {
        let dir = TempDir::new().unwrap();
        let prefs = TuiPrefsStore::load(dir.path()).prefs();
        assert_eq!(
            prefs,
            TuiPrefs {
                schema_version: CURRENT_PREFS_SCHEMA,
                ..Default::default()
            }
        );
    }

    #[test]
    fn schema_migration_resets_existing_view_choice_once() {
        let dir = TempDir::new().unwrap();
        // A pre-migration tui.json: an explicit view choice, no schema field.
        std::fs::write(
            dir.path().join("tui.json"),
            r#"{"view_mode": "ProjectGrouped", "left_pane_pct": 30}"#,
        )
        .unwrap();

        // The v0->v1 migration clears the choice (forcing the new default) but
        // leaves unrelated prefs intact, and persists the bump.
        let prefs = TuiPrefsStore::load(dir.path()).prefs();
        assert_eq!(prefs.view_mode, None);
        assert_eq!(prefs.left_pane_pct, Some(30));
        assert_eq!(prefs.schema_version, CURRENT_PREFS_SCHEMA);

        let reloaded = TuiPrefsStore::load(dir.path()).prefs();
        assert_eq!(reloaded.view_mode, None);
        assert_eq!(reloaded.schema_version, CURRENT_PREFS_SCHEMA);
    }

    #[test]
    fn schema_migration_preserves_choice_made_after_migration() {
        let dir = TempDir::new().unwrap();
        // A tui.json already at the current schema with an explicit choice —
        // i.e. the user toggled *after* upgrading. It must be left untouched.
        std::fs::write(
            dir.path().join("tui.json"),
            format!(
                r#"{{"schema_version": {CURRENT_PREFS_SCHEMA}, "view_mode": "ProjectGrouped"}}"#
            ),
        )
        .unwrap();

        let prefs = TuiPrefsStore::load(dir.path()).prefs();
        assert_eq!(prefs.view_mode, Some(ViewMode::ProjectGrouped));
        assert_eq!(prefs.schema_version, CURRENT_PREFS_SCHEMA);
    }

    #[tokio::test]
    async fn setters_persist_and_survive_reload() {
        let dir = TempDir::new().unwrap();
        let session = SessionId::new();
        let project = ProjectId::new();
        {
            let store = TuiPrefsStore::load(dir.path());
            store.set_view_mode(ViewMode::ProjectGrouped).await;
            store
                .set_selection(Some(session), Some(project), Some("buildbox".to_string()))
                .await;
            store.set_left_pane_pct(30).await;
        }
        let reloaded = TuiPrefsStore::load(dir.path()).prefs();
        assert_eq!(reloaded.view_mode, Some(ViewMode::ProjectGrouped));
        assert_eq!(reloaded.last_selected_session, Some(session));
        assert_eq!(reloaded.last_selected_project, Some(project));
        assert_eq!(reloaded.last_selected_backend.as_deref(), Some("buildbox"));
        assert_eq!(reloaded.left_pane_pct, Some(30));
    }
}
