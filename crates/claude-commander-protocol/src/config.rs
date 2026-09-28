//! `PATCH /config` request body.
//!
//! The allow-list of config fields a remote client may change is a rule the
//! server and every client must agree on, so it lives here rather than in the
//! server crate. Applying a patch to core's `Config` (and validating the merged
//! result) is the server's job — this crate only owns the shape.

use serde::{Deserialize, Serialize};

/// Partial config update: every field is optional, and only the fields below —
/// a conservative allow-list of benign UI/timing/behaviour options — may be
/// changed. Filesystem-path fields (`worktrees_dir`, `log_file`,
/// `commander_dir`, `per_repo_worktree_dirs`), program-launch fields
/// (`programs`, `shell_program`, `editor`, `editor_gui`,
/// `commander_program`, `commander_enabled`, `nix_develop`), credentials
/// (`server`, `remote_servers`, `stt`, `telemetry`) and complex nested tables
/// (`keybindings`, `theme`, `workspace_themes`, `sections`, `conversation`) are
/// intentionally absent, so a request can neither set nor reset them here —
/// `programs` and the workspace definitions have their own dedicated routes.
///
/// `deny_unknown_fields` means a body that even *mentions* such a field is
/// rejected (4xx) rather than silently dropped — a clear signal to the caller
/// that the field is off-limits. It also keeps the route safe with axum's
/// plain `Json` extractor: serde names an unknown field without echoing its
/// value. **Never add a secret-bearing field here**; see the server's
/// `extract::SafeJson` for why that would change the route's obligations.
///
/// Absent fields are omitted when serialized, so a client sending a patch sends
/// only what it means to change.
///
/// `in_progress_limit` is `Option<Option<u32>>` but, as deserialized by serde, a
/// JSON `null` reads as the *outer* `None` ("leave unchanged") — the inner
/// `None` (clear the limit) is not reachable over the wire today.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(optional_fields))]
pub struct ConfigPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_tmux: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_cache_ttl_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_cache_ttl_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_refresh_fps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_check_interval_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_pull_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_pull_interval_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_review_labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch_before_create: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_session: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_sync_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_state_poll_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invert_pr_label_color: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_session_program: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_number_debounce_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_summary_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rounded_borders: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precompute_review_caches: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_progress_limit: Option<Option<u32>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_patch_serializes_as_an_empty_object() {
        assert_eq!(
            serde_json::to_string(&ConfigPatch::default()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn only_present_fields_serialize_and_round_trip() {
        let patch = ConfigPatch {
            ui_refresh_fps: Some(45),
            resume_session: Some(false),
            ..Default::default()
        };
        let json = serde_json::to_string(&patch).unwrap();
        assert_eq!(json, r#"{"ui_refresh_fps":45,"resume_session":false}"#);
        assert_eq!(serde_json::from_str::<ConfigPatch>(&json).unwrap(), patch);
    }

    /// The allow-list is the security boundary: naming any field outside it —
    /// a credential table, a path, a program — is a hard error, not a no-op.
    #[test]
    fn unknown_fields_are_rejected() {
        for body in [
            r#"{"server":{"bind":"0.0.0.0"}}"#,
            r#"{"worktrees_dir":"/x"}"#,
            r#"{"programs":[]}"#,
            r#"{"stt":{"api_key":"k"}}"#,
        ] {
            assert!(
                serde_json::from_str::<ConfigPatch>(body).is_err(),
                "{body} must be rejected"
            );
        }
    }

    /// Pinned so the quirk documented on the type can't change silently: a
    /// JSON `null` means "leave unchanged", not "clear".
    #[test]
    fn in_progress_limit_null_reads_as_unchanged() {
        let patch: ConfigPatch = serde_json::from_str(r#"{"in_progress_limit":null}"#).unwrap();
        assert_eq!(patch.in_progress_limit, None);
        let patch: ConfigPatch = serde_json::from_str(r#"{"in_progress_limit":3}"#).unwrap();
        assert_eq!(patch.in_progress_limit, Some(Some(3)));
    }
}
