//! Pasted-image handling for remote image paste.
//!
//! When the desktop TUI is attached to a session on a **remote** server, the
//! Claude CLI runs on that (typically headless) server and cannot read the
//! user's local clipboard on Ctrl+V. Instead the TUI captures the local
//! clipboard image, ships the PNG bytes to the server, and the server writes
//! them to a temp file and injects the file path into the tmux pane — a form
//! the Claude CLI accepts (a plain-text image path in the prompt).
//!
//! This module holds the transport-agnostic, unit-testable pieces:
//! magic-byte sniffing (the accepted-type allow-list), RGBA→PNG encoding for
//! the clipboard path, and the pruned temp-file [`PasteImageStore`]. The
//! orchestration (resolve session → store → inject path) lives in
//! [`crate::api::CommanderService::paste_image`].

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use uuid::Uuid;

use crate::error::{Result, SessionError};

/// Max accepted pasted-image size (bytes). Clipboard screenshots are large but
/// bounded; this caps memory/disk from a huge or malicious upload. Enforced in
/// [`PasteImageStore::store`] and mirrored by an axum body limit on the route.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// Prune paste-image files older than this on each write. The file is only
/// needed until the Claude CLI reads it when the user submits the prompt, so a
/// short TTL keeps the directory from growing without bound.
pub const IMAGE_TTL: Duration = Duration::from_secs(60 * 60);

/// Hard cap on retained paste-image files, enforced by [`PasteImageStore::prune`]
/// in addition to the TTL. Bounds disk use even under a burst of pastes inside
/// the TTL window (an authed client can otherwise write ≤`MAX_IMAGE_BYTES` each).
pub const MAX_IMAGE_FILES: usize = 64;

/// Sniff an image type from the leading magic bytes, returning the file
/// extension to use. `None` means the bytes are not a recognised image — this
/// doubles as the accept allow-list: the extension is *never* taken from a
/// client-supplied filename or Content-Type, only from the content itself.
pub fn image_ext_from_magic(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.starts_with(PNG) {
        return Some("png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.starts_with(b"BM") {
        return Some("bmp");
    }
    // WEBP: "RIFF" <u32 len> "WEBP".
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

/// Validate pasted-image bytes: within the size cap and a recognised image
/// type. Returns the sniffed file extension on success. Callers validate up
/// front (before resolving the target session) so junk input is a clean
/// rejection independent of session existence.
pub fn validate(bytes: &[u8]) -> Result<&'static str> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(SessionError::InvalidImage(format!(
            "image is {} bytes, over the {} byte limit",
            bytes.len(),
            MAX_IMAGE_BYTES
        ))
        .into());
    }
    image_ext_from_magic(bytes).ok_or_else(|| {
        SessionError::InvalidImage("not a recognised image (png/jpeg/gif/webp/bmp)".into()).into()
    })
}

/// Encode raw RGBA pixels (as produced by a clipboard read) to PNG bytes.
/// Returns [`SessionError::InvalidImage`] if the buffer size doesn't match the
/// declared dimensions or the encode fails.
pub fn encode_rgba_png(width: u32, height: u32, rgba: Vec<u8>) -> Result<Vec<u8>> {
    use std::io::Cursor;

    use image::{ImageBuffer, ImageFormat, Rgba};

    let buf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, rgba)
        .ok_or_else(|| {
            SessionError::InvalidImage(
                "clipboard pixel buffer size does not match dimensions".into(),
            )
        })?;
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buf)
        .write_to(&mut out, ImageFormat::Png)
        .map_err(|e| SessionError::InvalidImage(format!("PNG encode failed: {e}")))?;
    Ok(out.into_inner())
}

/// A pruned temp-file store for pasted images, rooted at `<base_dir>/paste-images`.
/// In production `base_dir` is the OS temp dir (space-free on every platform,
/// and readable by the agent without a permission prompt); tests pass a
/// `TempDir` for isolation.
pub struct PasteImageStore {
    dir: PathBuf,
}

impl PasteImageStore {
    /// Create a store rooted under `<base_dir>/paste-images`. The directory is
    /// created lazily (0700) on the first [`Self::store`] call.
    pub fn new(base_dir: &Path) -> Self {
        Self {
            dir: base_dir.join("paste-images"),
        }
    }

    /// Validate, prune stale files, and write the image, returning its absolute
    /// path. Validation rejects oversized uploads and anything that isn't a
    /// recognised image (see [`validate`]). The filename is a fresh UUID + the
    /// sniffed extension — never client-controlled — so there is no
    /// path-traversal or arbitrary-extension surface.
    pub fn store(&self, bytes: &[u8]) -> Result<PathBuf> {
        let ext = validate(bytes)?;

        std::fs::create_dir_all(&self.dir)?;
        // Guard against a squatted temp dir: on a shared, world-writable `/tmp`,
        // another user could pre-create `<tmp>/paste-images` so it's owned by
        // them — `create_dir_all` then succeeds silently — and later swap/delete
        // our files between injection and the agent reading the path. Refuse to
        // proceed unless it's a real directory we own. Checked *before*
        // `harden_dir` (whose `chmod` follows symlinks) and before prune/write,
        // so we never touch — or chmod through — a foreign dir or planted symlink.
        verify_owned_dir(&self.dir)?;
        harden_dir(&self.dir);
        self.prune();

        let path = self.dir.join(format!("{}.{}", Uuid::new_v4(), ext));
        std::fs::write(&path, bytes)?;
        harden_file(&path);
        Ok(path)
    }

    /// Remove files older than [`IMAGE_TTL`], then enforce [`MAX_IMAGE_FILES`] by
    /// dropping the oldest survivors. Best-effort: any error (a racing removal, a
    /// permission hiccup) is ignored so a prune failure never blocks a paste.
    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let now = SystemTime::now();
        let mut survivors: Vec<(PathBuf, SystemTime)> = Vec::new();
        for entry in entries.flatten() {
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            let stale = now
                .duration_since(modified)
                .map(|age| age > IMAGE_TTL)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            } else {
                survivors.push((entry.path(), modified));
            }
        }
        // Count cap: drop the oldest survivors beyond the limit so a burst of
        // pastes within the TTL window can't grow the directory unbounded.
        if survivors.len() > MAX_IMAGE_FILES {
            survivors.sort_by_key(|(_, mtime)| *mtime);
            let excess = survivors.len() - MAX_IMAGE_FILES;
            for (path, _) in &survivors[..excess] {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Compose the literal text injected into the Claude pane for a stored image
/// `path`: the absolute path with spaces backslash-escaped — the form terminals
/// emit for a drag-dropped path, which the CLI accepts — wrapped in single
/// spaces so it doesn't merge with any text already in the prompt input. No
/// trailing newline: the user adds prompt text and submits. The path is
/// server-generated (space-free temp dir + UUID name) so the escape is
/// defensive belt-and-suspenders for any base dir that does contain a space.
pub fn compose_injection(path: &Path) -> String {
    let escaped = path.to_string_lossy().replace(' ', "\\ ");
    format!(" {escaped} ")
}

/// Verify the store directory is a real directory owned by the current user, so
/// a squatted `<tmp>/paste-images` (foreign-owned, or a symlink redirecting our
/// writes) can't silently capture pasted images. Uses `symlink_metadata` so a
/// symlink is rejected rather than followed. No-op on non-Unix (no shared
/// world-writable temp dir with the same exposure).
fn verify_owned_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.file_type().is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("paste-image dir {} is not a directory", path.display()),
            )
            .into());
        }
        let uid = nix::unistd::getuid().as_raw();
        if meta.uid() != uid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "paste-image dir {} is owned by uid {}, not the current uid {}",
                    path.display(),
                    meta.uid(),
                    uid
                ),
            )
            .into());
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Restrict a directory to owner-only access (0700) on Unix. No-op elsewhere.
fn harden_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Restrict a file to owner-only access (0600) on Unix. No-op elsewhere.
fn harden_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but valid 1×1 PNG, used to exercise the store without pulling
    /// in the encoder.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn magic_recognises_known_formats() {
        assert_eq!(image_ext_from_magic(TINY_PNG), Some("png"));
        assert_eq!(image_ext_from_magic(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(image_ext_from_magic(b"GIF89a....."), Some("gif"));
        assert_eq!(image_ext_from_magic(b"BM......"), Some("bmp"));
        assert_eq!(image_ext_from_magic(b"RIFF\0\0\0\0WEBP...."), Some("webp"));
    }

    #[test]
    fn magic_rejects_non_images() {
        assert_eq!(image_ext_from_magic(b""), None);
        assert_eq!(image_ext_from_magic(b"#!/bin/sh\n"), None);
        assert_eq!(image_ext_from_magic(b"not an image at all"), None);
        // A truncated RIFF header that isn't actually WEBP.
        assert_eq!(image_ext_from_magic(b"RIFF\0\0\0\0AVI "), None);
    }

    #[test]
    fn encode_rgba_png_produces_png_bytes() {
        // 2×2 RGBA = 16 bytes.
        let rgba = vec![255u8; 2 * 2 * 4];
        let png = encode_rgba_png(2, 2, rgba).expect("encode");
        assert_eq!(image_ext_from_magic(&png), Some("png"));
    }

    #[test]
    fn encode_rgba_png_rejects_size_mismatch() {
        // 3 bytes can't be a 2×2 RGBA image.
        assert!(encode_rgba_png(2, 2, vec![0, 0, 0]).is_err());
    }

    #[test]
    fn store_writes_recognised_image() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PasteImageStore::new(dir.path());
        let path = store.store(TINY_PNG).expect("store");
        assert!(path.starts_with(dir.path().join("paste-images")));
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(std::fs::read(&path).unwrap(), TINY_PNG);
    }

    #[test]
    fn store_rejects_non_image() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PasteImageStore::new(dir.path());
        assert!(store.store(b"definitely not an image").is_err());
    }

    #[test]
    fn store_rejects_oversized() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PasteImageStore::new(dir.path());
        // Valid PNG header but padded past the limit.
        let mut big = TINY_PNG.to_vec();
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        assert!(store.store(&big).is_err());
    }

    #[test]
    fn store_prunes_stale_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PasteImageStore::new(dir.path());
        let images = dir.path().join("paste-images");
        std::fs::create_dir_all(&images).unwrap();
        // A pre-existing file with an old mtime should be pruned on the next store.
        let stale = images.join("stale.png");
        std::fs::write(&stale, b"old").unwrap();
        let old = SystemTime::now() - IMAGE_TTL - Duration::from_secs(60);
        filetime::set_file_mtime(&stale, filetime::FileTime::from_system_time(old)).unwrap();

        let fresh = store.store(TINY_PNG).expect("store");
        assert!(!stale.exists(), "stale file should have been pruned");
        assert!(fresh.exists(), "freshly stored file should remain");
    }

    #[test]
    fn store_enforces_file_count_cap() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PasteImageStore::new(dir.path());
        let images = dir.path().join("paste-images");
        std::fs::create_dir_all(&images).unwrap();
        // Seed more than the cap of fresh files with staggered mtimes so "oldest"
        // is well-defined.
        let base = SystemTime::now() - Duration::from_secs(1000);
        for i in 0..(MAX_IMAGE_FILES + 5) {
            let p = images.join(format!("seed-{i}.png"));
            std::fs::write(&p, b"x").unwrap();
            let mtime = base + Duration::from_secs(i as u64);
            filetime::set_file_mtime(&p, filetime::FileTime::from_system_time(mtime)).unwrap();
        }
        // Storing prunes to the cap, then writes one new file.
        store.store(TINY_PNG).expect("store");
        let count = std::fs::read_dir(&images).unwrap().count();
        assert!(
            count <= MAX_IMAGE_FILES + 1,
            "count {count} should be capped near {MAX_IMAGE_FILES}"
        );
        // The very oldest seed must have been evicted.
        assert!(!images.join("seed-0.png").exists());
    }

    #[cfg(unix)]
    #[test]
    fn store_rejects_symlinked_dir() {
        // A squatted store dir that's a symlink (a redirection an attacker could
        // plant on shared /tmp) is rejected rather than followed.
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("real-target");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("paste-images")).unwrap();
        let store = PasteImageStore::new(dir.path());
        assert!(
            store.store(TINY_PNG).is_err(),
            "a symlinked store dir must be rejected, not written through"
        );
    }

    #[test]
    fn compose_injection_wraps_and_escapes() {
        use std::path::Path;
        // Space-free path: wrapped in single spaces, no escaping needed.
        assert_eq!(compose_injection(Path::new("/tmp/x.png")), " /tmp/x.png ");
        // Interior spaces are backslash-escaped (the drag-drop form the CLI
        // accepts); the wrapping separator spaces stay unescaped.
        assert_eq!(
            compose_injection(Path::new("/a b/c d.png")),
            " /a\\ b/c\\ d.png "
        );
        // No trailing newline — the user submits.
        assert!(!compose_injection(Path::new("/tmp/x.png")).contains('\n'));
    }
}
