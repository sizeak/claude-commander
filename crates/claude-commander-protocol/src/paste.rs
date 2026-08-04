//! Pasted-image wire contract for `POST /sessions/{id}/paste-image`.
//!
//! This lives in the protocol crate because it *is* part of the route's
//! contract, agreed by every party rather than owned by one of them: the server
//! enforces [`MAX_IMAGE_BYTES`] as its request body limit, the service
//! re-[`validate`]s the body, and clients (the desktop TUI, the CLI, the Flutter
//! app) check the same rules locally so a doomed upload never leaves the device.
//!
//! Everything here is pure byte inspection — no `image` crate, no filesystem —
//! so it cross-compiles wherever the protocol crate does. The *effectful* halves
//! of image paste stay in `claude-commander-core`: RGBA→PNG encoding, the pruned
//! temp-file store, and the pane injection.

use std::fmt;

/// Max accepted pasted-image size (bytes). Clipboard screenshots are large but
/// bounded; this caps memory/disk from a huge or malicious upload. Enforced as
/// the server's axum body limit, re-checked by the service, and pre-checked by
/// clients.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// Why a would-be image was refused. Carries no borrowed data so callers can map
/// it into their own error type (core wraps it in `SessionError::InvalidImage`,
/// the HTTP client in `ClientError::InvalidRequest`).
///
/// Implemented by hand rather than via `thiserror`: this crate is deliberately
/// dependency-light (serde only), and the messages are part of the 400 response
/// body users see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageRejection {
    /// Body exceeded [`MAX_IMAGE_BYTES`].
    TooLarge { len: usize, max: usize },
    /// Leading bytes matched none of the accepted formats.
    Unrecognised,
}

impl fmt::Display for ImageRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { len, max } => {
                write!(f, "image is {len} bytes, over the {max} byte limit")
            }
            Self::Unrecognised => {
                write!(f, "not a recognised image (png/jpeg/gif/webp/bmp)")
            }
        }
    }
}

impl std::error::Error for ImageRejection {}

/// An accepted image type. This enum *is* the allow-list: a value can only be
/// produced by [`format_from_magic`] sniffing real content, never from a
/// client-supplied filename or `Content-Type` — so a caller cannot name a type
/// the contract doesn't accept.
///
/// If you add a variant here, also extend `_formats` in the Flutter client's
/// `lib/services/clipboard_image_reader.dart`, which lists the clipboard formats
/// it will offer and is the one place this allow-list is mirrored outside Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Webp,
}

impl ImageFormat {
    /// File extension for a stored image. The server names pasted files
    /// `<uuid>.<ext>`, so this is the only thing that decides the extension.
    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Webp => "webp",
        }
    }

    /// `Content-Type` for an upload. Advisory only: the server re-sniffs the
    /// body and never trusts the header, so a wrong value cannot widen what it
    /// accepts.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Webp => "image/webp",
        }
    }
}

/// Sniff an image type from the leading magic bytes. `None` means the bytes are
/// not a recognised image.
pub fn format_from_magic(bytes: &[u8]) -> Option<ImageFormat> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.starts_with(PNG) {
        return Some(ImageFormat::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(ImageFormat::Gif);
    }
    if bytes.starts_with(b"BM") {
        return Some(ImageFormat::Bmp);
    }
    // WEBP: "RIFF" <u32 len> "WEBP".
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::Webp);
    }
    None
}

/// Validate pasted-image bytes: within the size cap and a recognised image
/// type. Callers validate up front (before resolving the target session) so junk
/// input is a clean rejection independent of session existence.
///
/// Size is checked *first*, so an over-cap body is [`ImageRejection::TooLarge`]
/// even when it is a perfectly valid image.
pub fn validate(bytes: &[u8]) -> Result<ImageFormat, ImageRejection> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ImageRejection::TooLarge {
            len: bytes.len(),
            max: MAX_IMAGE_BYTES,
        });
    }
    format_from_magic(bytes).ok_or(ImageRejection::Unrecognised)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but valid 1×1 PNG.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn magic_recognises_known_formats() {
        assert_eq!(format_from_magic(TINY_PNG), Some(ImageFormat::Png));
        assert_eq!(
            format_from_magic(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(format_from_magic(b"GIF89a....."), Some(ImageFormat::Gif));
        assert_eq!(format_from_magic(b"BM......"), Some(ImageFormat::Bmp));
        assert_eq!(
            format_from_magic(b"RIFF\0\0\0\0WEBP...."),
            Some(ImageFormat::Webp)
        );
    }

    #[test]
    fn magic_rejects_non_images() {
        assert_eq!(format_from_magic(b""), None);
        assert_eq!(format_from_magic(b"#!/bin/sh\n"), None);
        assert_eq!(format_from_magic(b"not an image at all"), None);
        // A truncated RIFF header that isn't actually WEBP.
        assert_eq!(format_from_magic(b"RIFF\0\0\0\0AVI "), None);
    }

    #[test]
    fn validate_accepts_recognised_image() {
        assert_eq!(validate(TINY_PNG), Ok(ImageFormat::Png));
    }

    #[test]
    fn validate_rejects_non_image_as_unrecognised() {
        assert_eq!(validate(b"not an image"), Err(ImageRejection::Unrecognised));
        assert_eq!(
            ImageRejection::Unrecognised.to_string(),
            "not a recognised image (png/jpeg/gif/webp/bmp)"
        );
    }

    /// Oversized bodies are refused on length alone — before the magic-byte
    /// check, so a huge *valid* PNG is still `TooLarge` rather than accepted.
    #[test]
    fn validate_rejects_oversized_even_when_valid_png() {
        let mut huge = TINY_PNG.to_vec();
        huge.resize(MAX_IMAGE_BYTES + 1, 0);
        assert_eq!(
            validate(&huge),
            Err(ImageRejection::TooLarge {
                len: MAX_IMAGE_BYTES + 1,
                max: MAX_IMAGE_BYTES,
            })
        );
        assert_eq!(
            ImageRejection::TooLarge {
                len: 20,
                max: MAX_IMAGE_BYTES
            }
            .to_string(),
            format!("image is 20 bytes, over the {MAX_IMAGE_BYTES} byte limit")
        );
    }

    /// Exactly at the cap is accepted — the limit is inclusive.
    #[test]
    fn validate_accepts_body_exactly_at_cap() {
        let mut at_cap = TINY_PNG.to_vec();
        at_cap.resize(MAX_IMAGE_BYTES, 0);
        assert_eq!(validate(&at_cap), Ok(ImageFormat::Png));
    }

    /// Every variant maps to a distinct extension and content type. Exhaustive
    /// by construction — a new variant added without updating `ext()` or
    /// `content_type()` fails to compile, so this only has to pin the values.
    #[test]
    fn every_format_has_an_extension_and_content_type() {
        for (format, ext, content_type) in [
            (ImageFormat::Png, "png", "image/png"),
            (ImageFormat::Jpeg, "jpg", "image/jpeg"),
            (ImageFormat::Gif, "gif", "image/gif"),
            (ImageFormat::Bmp, "bmp", "image/bmp"),
            (ImageFormat::Webp, "webp", "image/webp"),
        ] {
            assert_eq!(format.ext(), ext);
            assert_eq!(format.content_type(), content_type);
        }
    }
}
