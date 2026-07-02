//! Core-config + health handlers.
//!
//! Thin wrappers over `CommanderService`: `read_config`, a PATCH-style
//! partial `update`, `reload_config`, and `check_tmux` (the `/health/tmux`
//! 200/503 probe).
//!
//! Note: the server's own bind/token config is deliberately NOT exposed here —
//! it lives only in the server crate, so this endpoint can't leak or clobber it.
//!
//! `update` is a **partial** update over an explicit allow-list (see
//! [`ConfigPatch`]). A full-replace `PUT` would let a remote client rewrite
//! filesystem-path and program-launch fields (`worktrees_dir`, `editor`,
//! `default_program`, …); restricting to a conservative set of benign
//! UI/timing fields keeps those off-limits even though they remain readable via
//! `GET /config`.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use claude_commander_core::Config;
use claude_commander_core::error::SessionError;
use serde::Deserialize;
use serde_json::json;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /config` → `read_config`.
pub async fn read(State(state): State<AppState>) -> Json<Config> {
    Json(state.service.read_config())
}

/// Partial config update: every field is optional, and only the fields below —
/// a conservative allow-list of benign UI/timing/behaviour options — may be
/// changed. Filesystem-path fields (`worktrees_dir`, `log_file`,
/// `commander_dir`, `per_repo_worktree_dirs`), program-launch fields
/// (`default_program`, `shell_program`, `editor`, `editor_gui`,
/// `commander_program`, `commander_enabled`, `nix_develop`), and complex nested
/// tables (`keybindings`, `theme`, `sections`, `conversation`, `stt`,
/// `telemetry`) are intentionally absent, so a request can neither set nor
/// reset them. `deny_unknown_fields` means a body that even *mentions* such a
/// field is rejected (400) rather than silently dropped — a clear signal to the
/// caller that the field is off-limits.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigPatch {
    pub branch_prefix: Option<String>,
    pub max_concurrent_tmux: Option<usize>,
    pub capture_cache_ttl_ms: Option<u64>,
    pub diff_cache_ttl_ms: Option<u64>,
    pub ui_refresh_fps: Option<u32>,
    pub pr_check_interval_secs: Option<u64>,
    pub project_pull_enabled: Option<bool>,
    pub project_pull_interval_secs: Option<u64>,
    pub pr_review_labels: Option<Vec<String>>,
    pub fetch_before_create: Option<bool>,
    pub resume_session: Option<bool>,
    pub state_sync_interval_ms: Option<u64>,
    pub agent_state_poll_interval_ms: Option<u64>,
    pub invert_pr_label_color: Option<bool>,
    pub show_session_program: Option<bool>,
    pub dim_unfocused_preview: Option<bool>,
    pub dim_unfocused_opacity: Option<f32>,
    pub session_number_debounce_ms: Option<u64>,
    pub ai_summary_enabled: Option<bool>,
    pub rounded_borders: Option<bool>,
    pub precompute_review_caches: Option<bool>,
    pub in_progress_limit: Option<Option<u32>>,
}

impl ConfigPatch {
    /// Apply the present fields onto `cfg`, leaving everything else untouched.
    fn apply_to(self, cfg: &mut Config) {
        macro_rules! set {
            ($field:ident) => {
                if let Some(v) = self.$field {
                    cfg.$field = v;
                }
            };
        }
        set!(branch_prefix);
        set!(max_concurrent_tmux);
        set!(capture_cache_ttl_ms);
        set!(diff_cache_ttl_ms);
        set!(ui_refresh_fps);
        set!(pr_check_interval_secs);
        set!(project_pull_enabled);
        set!(project_pull_interval_secs);
        set!(pr_review_labels);
        set!(fetch_before_create);
        set!(resume_session);
        set!(state_sync_interval_ms);
        set!(agent_state_poll_interval_ms);
        set!(invert_pr_label_color);
        set!(show_session_program);
        set!(dim_unfocused_preview);
        set!(dim_unfocused_opacity);
        set!(session_number_debounce_ms);
        set!(ai_summary_enabled);
        set!(rounded_borders);
        set!(precompute_review_caches);
        set!(in_progress_limit);
    }
}

/// Validate a merged config before persisting. Catches values that would break
/// the running app (a zero refresh rate, an out-of-range opacity, a zero tmux
/// concurrency). Returns a 400 `ApiError` on failure.
fn validate(cfg: &Config) -> Result<(), ApiError> {
    let invalid = |key: &str, reason: &str| {
        ApiError(
            SessionError::InvalidName {
                name: key.to_string(),
                reason: reason.to_string(),
            }
            .into(),
        )
    };
    if cfg.ui_refresh_fps == 0 {
        return Err(invalid("ui_refresh_fps", "must be greater than zero"));
    }
    if cfg.max_concurrent_tmux == 0 {
        return Err(invalid("max_concurrent_tmux", "must be greater than zero"));
    }
    if !(0.0..=1.0).contains(&cfg.dim_unfocused_opacity) {
        return Err(invalid(
            "dim_unfocused_opacity",
            "must be between 0.0 and 1.0",
        ));
    }
    Ok(())
}

/// `PATCH /config` → partial, allow-listed update → 204.
///
/// Merges the supplied fields onto the current config, validates the result,
/// then persists. Sensitive/path/program fields are not in [`ConfigPatch`], so
/// they cannot be changed here.
pub async fn update(
    State(state): State<AppState>,
    Json(patch): Json<ConfigPatch>,
) -> Result<StatusCode, ApiError> {
    let mut merged = state.service.read_config();
    patch.apply_to(&mut merged);
    validate(&merged)?;
    state.service.update_config(merged)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /config/reload` → `reload_config` → `{ "reloaded": bool }`
/// (true when the on-disk config differed and was re-read).
pub async fn reload(State(state): State<AppState>) -> Result<Response, ApiError> {
    let reloaded = state.service.reload_config()?;
    Ok(Json(json!({ "reloaded": reloaded })).into_response())
}

/// `GET /health/tmux` → `check_tmux` → 200 on Ok, 503 on Err.
///
/// Distinct from the rest: a tmux probe failure is the *expected* signal of an
/// unhealthy backing service, not a 500 — so it short-circuits to 503 rather
/// than going through [`ApiError`]'s variant mapping.
pub async fn health_tmux(State(state): State<AppState>) -> StatusCode {
    match state.service.check_tmux().await {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::{Router, routing::get};
    use claude_commander_core::Config;
    use tempfile::TempDir;

    use crate::handlers::test_support::{get as do_get, json, send, test_state};
    use crate::state::AppState;

    fn router(state: AppState) -> Router {
        Router::new()
            .route("/config", get(super::read).patch(super::update))
            .with_state(state)
    }

    async fn patch(state: AppState, body: serde_json::Value) -> axum::http::StatusCode {
        let req = Request::patch("/config")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        send(router(state), req).await.0
    }

    /// `GET /config` returns the live config as JSON (here, defaults).
    #[tokio::test]
    async fn read_config_is_200_json() {
        let dir = TempDir::new().unwrap();
        let (status, body) = do_get(router(test_state(&dir)), "/config").await;
        assert_eq!(status, 200);
        // Round-trips into a `Config` (the default), proving it's the real shape.
        let _config: Config = json(&body);
    }

    /// An allow-listed field updates and persists; nothing else changes.
    #[tokio::test]
    async fn patch_updates_allowed_field() {
        let dir = TempDir::new().unwrap();
        let state = test_state(&dir);
        let before = state.service.read_config();
        assert_ne!(before.ui_refresh_fps, 45, "fixture must differ from target");

        let status = patch(state.clone(), serde_json::json!({ "ui_refresh_fps": 45 })).await;
        assert_eq!(status, 204);

        let after = state.service.read_config();
        assert_eq!(after.ui_refresh_fps, 45);
        // An untouched field keeps its prior value.
        assert_eq!(after.worktrees_dir, before.worktrees_dir);
    }

    /// A sensitive path field cannot be changed: `deny_unknown_fields` rejects a
    /// body that even names `worktrees_dir`, and the stored config is unchanged.
    #[tokio::test]
    async fn patch_rejects_sensitive_field() {
        let dir = TempDir::new().unwrap();
        let state = test_state(&dir);
        let before = state.service.read_config();

        let status = patch(
            state.clone(),
            serde_json::json!({ "worktrees_dir": "/tmp/evil" }),
        )
        .await;
        assert!(
            status.is_client_error(),
            "patching a sensitive field must be a 4xx, got {status}"
        );

        let after = state.service.read_config();
        assert_eq!(
            after.worktrees_dir, before.worktrees_dir,
            "worktrees_dir must be unchanged after a rejected patch"
        );
    }

    /// A merged config that fails validation (zero fps) is a 400 and is not
    /// persisted.
    #[tokio::test]
    async fn patch_rejects_invalid_merged_value() {
        let dir = TempDir::new().unwrap();
        let state = test_state(&dir);
        let before = state.service.read_config();

        let status = patch(state.clone(), serde_json::json!({ "ui_refresh_fps": 0 })).await;
        assert_eq!(status, 400);

        let after = state.service.read_config();
        assert_eq!(after.ui_refresh_fps, before.ui_refresh_fps);
    }
}
