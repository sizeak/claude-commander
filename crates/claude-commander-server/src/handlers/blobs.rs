//! Diff-blob handler — serves raw file bytes for the review view.
//!
//! Wraps `CommanderService::fetch_diff_blob`, returning the file's raw bytes as
//! `application/octet-stream` (NOT JSON). The `path` query param is validated
//! against traversal before it reaches the (unguarded) core reader: core does a
//! plain `worktree.join(path)` / `git show <ref>:<path>`, so a `..`/absolute
//! path would otherwise escape the worktree.

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
};
use claude_commander_core::api::DiffSide;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

use super::parse_session_id;

#[derive(Debug, Deserialize)]
pub struct BlobQuery {
    pub side: DiffSide,
    pub path: String,
}

/// `GET /sessions/{id}/blob?side=&path=` → `fetch_diff_blob` → raw bytes.
///
/// `side` is `old`/`new` (the `DiffSide` serde repr). `path` is the diff
/// display path, validated to stay within the worktree. axum's `Query`
/// extractor has already percent-decoded both.
pub async fn fetch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<BlobQuery>,
) -> Result<Response, ApiError> {
    let id = parse_session_id(&id)?;
    let rel = validate_blob_path(&q.path)?;
    let bytes = state.service.fetch_diff_blob(&id, q.side, rel).await?;

    // A basename for Content-Disposition; never the full (possibly nested) path.
    let filename = rel.rsplit('/').next().unwrap_or(rel);
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("inline; filename=\"{}\"", sanitize_filename(filename)),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// Reject anything that could escape the worktree: empty, absolute, a Windows
/// drive/UNC path, or any `..` / `.` component. Returns the validated path on
/// success. The error maps to 400 via [`ApiError`].
fn validate_blob_path(path: &str) -> Result<&str, ApiError> {
    let bad = |reason: &str| {
        ApiError(
            claude_commander_core::error::SessionError::InvalidName {
                name: path.to_string(),
                reason: reason.to_string(),
            }
            .into(),
        )
    };

    if path.is_empty() {
        return Err(bad("blob path is empty"));
    }
    // Absolute (Unix) or rooted/drive (Windows) paths escape the worktree.
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(bad("blob path must be relative"));
    }
    if path.as_bytes().get(1) == Some(&b':') {
        return Err(bad("blob path must not be a drive path"));
    }
    // No traversal or current-dir components, on either separator.
    for component in path.split(['/', '\\']) {
        if component == ".." || component == "." {
            return Err(bad("blob path must not contain '.' or '..' components"));
        }
    }
    Ok(path)
}

/// Strip characters that would break the quoted `filename="..."` parameter
/// (quotes, backslashes, control bytes). The path is already traversal-checked;
/// this only keeps the header well-formed.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::validate_blob_path;

    #[test]
    fn accepts_plain_relative_paths() {
        assert_eq!(validate_blob_path("src/main.rs").unwrap(), "src/main.rs");
        assert_eq!(validate_blob_path("a.txt").unwrap(), "a.txt");
    }

    #[test]
    fn rejects_traversal_and_absolute() {
        assert!(validate_blob_path("").is_err());
        assert!(validate_blob_path("/etc/passwd").is_err());
        assert!(validate_blob_path("../secret").is_err());
        assert!(validate_blob_path("src/../../secret").is_err());
        assert!(validate_blob_path("./src/x").is_err());
        assert!(validate_blob_path("..\\windows").is_err());
        assert!(validate_blob_path("\\\\server\\share").is_err());
        assert!(validate_blob_path("C:\\x").is_err());
    }

    #[test]
    fn validate_error_maps_to_400() {
        use axum::http::StatusCode;
        let err = validate_blob_path("../x").unwrap_err();
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    /// A bad session id on the blob route maps to 400 before any blob read.
    #[tokio::test]
    async fn bad_session_id_is_400() {
        use crate::handlers::test_support::{get as do_get, test_state};
        use axum::{Router, routing::get};
        let dir = tempfile::TempDir::new().unwrap();
        let router = Router::new()
            .route("/sessions/{id}/blob", get(super::fetch))
            .with_state(test_state(&dir));
        let (status, _) = do_get(router, "/sessions/not-a-uuid/blob?side=new&path=a.txt").await;
        assert_eq!(status, 400);
    }
}
