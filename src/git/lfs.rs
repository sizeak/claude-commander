//! git-LFS pointer resolution for the binary-diff seam.
//!
//! For LFS-tracked files, the bytes git stores (and that `read_base_blob` /
//! `read_worktree_file` return) are the small *pointer* text, not the real
//! blob — so an image would render as garbage. This module detects that
//! pointer format and resolves it to the real bytes by piping the pointer
//! through `git lfs smudge`, which reads the pointer on stdin and writes the
//! resolved blob on stdout.
//!
//! Resolution is best-effort: if `git lfs` is not installed, or the object has
//! not been pulled, smudging fails and we fall back to the original pointer
//! bytes (the same degraded render as before LFS resolution existed).

use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::warn;

use crate::error::{GitError, Result};

/// Pointer files are tiny by spec; anything larger is real content, not a
/// pointer. Guarding on size keeps the UTF-8 / line scan cheap and avoids
/// misclassifying a large text blob that happens to start with `version `.
/// `pub(crate)` so the diff parser can bail out of reconstructing a side once
/// it provably exceeds this cap (see `review_diff::reconstruct_side`).
pub(crate) const MAX_POINTER_LEN: usize = 1024;

/// Whether `bytes` is a git-LFS pointer file.
///
/// Per the LFS spec a pointer is small UTF-8 text whose first line is the
/// `version` URL, and which carries `oid sha256:<hex>` and `size <n>` lines.
/// The check is strict on purpose: a false positive would send real image
/// bytes through `git lfs smudge` (harmless but wasteful), while the strict
/// form costs nothing on the common non-pointer path.
pub fn is_lfs_pointer(bytes: &[u8]) -> bool {
    if bytes.len() > MAX_POINTER_LEN {
        return false;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut lines = text.lines();
    // First line must be the spec version URL. Accept the current spec host
    // and the legacy `hawser` host LFS still recognises.
    let Some(first) = lines.next() else {
        return false;
    };
    let is_version = first.starts_with("version https://git-lfs.github.com/spec/")
        || first.starts_with("version https://hawser.github.com/spec/");
    if !is_version {
        return false;
    }
    // `oid sha256:<64 hex>` and `size <u64>` — require the exact shapes the
    // spec mandates, so near-miss text (e.g. `size abc`) isn't smudged.
    let has_oid = text.lines().any(|l| {
        l.strip_prefix("oid sha256:")
            .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
    });
    let has_size = text.lines().any(|l| {
        l.strip_prefix("size ")
            .is_some_and(|n| n.parse::<u64>().is_ok())
    });
    has_oid && has_size
}

/// Pipe an LFS pointer through `git lfs smudge -- <path>` to get the real
/// blob bytes. The pointer is fed on stdin; the resolved content comes back on
/// stdout. Runs as a subprocess so the work stays off the async runtime's
/// worker threads, matching the other blob reads in this crate.
///
/// Deadlock invariant: writing the whole `pointer` to stdin *before* draining
/// stdout (`wait_with_output`) is safe only because callers reach this through
/// `resolve_if_pointer`, which gates on `is_lfs_pointer` — so `pointer` is
/// always ≤ `MAX_POINTER_LEN` (1 KiB), far under the OS pipe buffer, and can't
/// block on a full pipe before the child starts reading. A future caller that
/// fed a large payload here would deadlock; keep the `is_lfs_pointer` gate.
async fn smudge(worktree: &Path, path: &str, pointer: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new("git")
        .current_dir(worktree)
        .args(["lfs", "smudge", "--", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GitError::OperationFailed(e.to_string()))?;

    // Write the pointer to stdin and drop the handle so smudge sees EOF.
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| GitError::OperationFailed("git lfs smudge: no stdin".into()))?;
    stdin
        .write_all(pointer)
        .await
        .map_err(|e| GitError::OperationFailed(e.to_string()))?;
    drop(stdin);

    let out = child
        .wait_with_output()
        .await
        .map_err(|e| GitError::OperationFailed(e.to_string()))?;
    if !out.status.success() {
        return Err(GitError::OperationFailed(format!(
            "git lfs smudge -- {path}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .into());
    }
    Ok(out.stdout)
}

/// Fetch and check out the real content for every LFS-tracked file in
/// `worktree`, replacing the pointer files left behind when the worktree was
/// created with `GIT_LFS_SKIP_SMUDGE=1`.
///
/// Best-effort: a non-zero exit (git-lfs not installed, network failure, repo
/// not using LFS) is returned as an error for the caller to log; it is not
/// fatal to the session. On a non-LFS repo this is a near no-op.
pub async fn pull(worktree: &Path) -> Result<()> {
    let output = Command::new("git")
        .current_dir(worktree)
        .args(["lfs", "pull"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| GitError::OperationFailed(e.to_string()))?;
    if !output.status.success() {
        return Err(GitError::OperationFailed(format!(
            "git lfs pull: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    Ok(())
}

/// If `bytes` is an LFS pointer, resolve it to the real blob; otherwise return
/// `bytes` unchanged. Best-effort: a smudge failure (LFS not installed, object
/// not pulled) is logged and the original pointer bytes are returned so the
/// caller still gets *something* rather than an error.
pub async fn resolve_if_pointer(worktree: &Path, path: &str, bytes: Vec<u8>) -> Vec<u8> {
    if !is_lfs_pointer(&bytes) {
        return bytes;
    }
    match smudge(worktree, path, &bytes).await {
        Ok(resolved) => resolved,
        Err(e) => {
            warn!(path, error = %e, "git lfs smudge failed; returning pointer bytes");
            bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POINTER: &str = "version https://git-lfs.github.com/spec/v1\n\
oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\n\
size 12345\n";

    #[test]
    fn detects_canonical_pointer() {
        assert!(is_lfs_pointer(POINTER.as_bytes()));
    }

    #[test]
    fn detects_legacy_hawser_pointer() {
        let p = POINTER.replace(
            "https://git-lfs.github.com/spec/v1",
            "https://hawser.github.com/spec/v1",
        );
        assert!(is_lfs_pointer(p.as_bytes()));
    }

    #[test]
    fn png_header_is_not_a_pointer() {
        // PNG magic + IHDR start — real image bytes, not a pointer.
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        assert!(!is_lfs_pointer(png));
    }

    #[test]
    fn arbitrary_text_is_not_a_pointer() {
        assert!(!is_lfs_pointer(b"version 1.2.3\nsome other file\n"));
    }

    #[test]
    fn pointer_missing_oid_is_rejected() {
        let p = "version https://git-lfs.github.com/spec/v1\nsize 12345\n";
        assert!(!is_lfs_pointer(p.as_bytes()));
    }

    #[test]
    fn pointer_missing_size_is_rejected() {
        let p = "version https://git-lfs.github.com/spec/v1\noid sha256:abc\n";
        assert!(!is_lfs_pointer(p.as_bytes()));
    }

    #[test]
    fn non_numeric_size_is_rejected() {
        // Near-miss text that would have slipped past a "non-empty suffix" check.
        let p = "version https://git-lfs.github.com/spec/v1\n\
oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\n\
size abc\n";
        assert!(!is_lfs_pointer(p.as_bytes()));
    }

    #[test]
    fn non_hex_or_wrong_length_oid_is_rejected() {
        // Right prefix, wrong oid shape (too short / non-hex) must not match.
        let short = "version https://git-lfs.github.com/spec/v1\noid sha256:abcd\nsize 12345\n";
        assert!(!is_lfs_pointer(short.as_bytes()));
        let non_hex = "version https://git-lfs.github.com/spec/v1\n\
oid sha256:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz\n\
size 12345\n";
        assert!(!is_lfs_pointer(non_hex.as_bytes()));
    }

    #[test]
    fn oversized_input_is_not_a_pointer() {
        // A blob larger than the spec's pointer size, even if it starts like one.
        let mut big = POINTER.to_string();
        big.push_str(&"x".repeat(MAX_POINTER_LEN));
        assert!(!is_lfs_pointer(big.as_bytes()));
    }

    #[test]
    fn invalid_utf8_is_not_a_pointer() {
        assert!(!is_lfs_pointer(&[0xff, 0xfe, 0x00, 0x01]));
    }
}
