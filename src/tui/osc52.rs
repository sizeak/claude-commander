//! OSC 52 clipboard write.
//!
//! Most modern terminal emulators implement the OSC 52 escape sequence,
//! which lets a TUI hand a string to the user's *system* clipboard via
//! the terminal itself — no clipboard daemon, no `xclip`/`pbcopy`. This
//! works through SSH and tmux, which is exactly what we need: we're a
//! Rust TUI sometimes running over `ssh -tt host bash -ilc 'tmux attach'`
//! and we want the user's local clipboard to receive the text.
//!
//! Compatibility notes:
//! - Works in most modern terminals (kitty, alacritty, wezterm, foot,
//!   iTerm2, Windows Terminal).
//! - Requires `set-clipboard on` in tmux (default in recent tmux). We
//!   issue the sequence regardless; if the terminal/tmux ignores it,
//!   the user just sees no clipboard update. No fatal error.
//! - The encoded payload is plain base64 (RFC 4648, standard alphabet).

use std::io::{self, Write};

const OSC_52_PREFIX: &str = "\x1b]52;c;";
const BEL: char = '\x07';

/// Write `text` to the user's system clipboard via OSC 52. Best-effort —
/// silently no-ops if the terminal doesn't support the sequence.
pub fn copy_to_clipboard(text: &str) -> io::Result<()> {
    let encoded = base64_encode(text.as_bytes());
    let mut out = io::stdout().lock();
    out.write_all(OSC_52_PREFIX.as_bytes())?;
    out.write_all(encoded.as_bytes())?;
    write!(out, "{BEL}")?;
    out.flush()
}

/// Standard-alphabet base64 (`A-Za-z0-9+/`) with `=` padding. Self-contained
/// so we don't pull in the `base64` crate just for clipboard writes.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks_exact(3);
    for chunk in &mut chunks {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | (chunk[2] as u32);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHABET[(n & 0x3f) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        0 => {}
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => unreachable!(),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_known_vectors() {
        // RFC 4648 §10 test vectors.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_high_bytes() {
        // Make sure we don't sign-extend a `u8` >= 0x80 into a negative
        // u32 by accident (this used to bite naive implementations).
        let bytes: Vec<u8> = (0u8..=0xffu8).collect();
        let encoded = base64_encode(&bytes);
        // 256 bytes -> 344 base64 chars (256 * 4/3, rounded up to multiple of 4).
        assert_eq!(encoded.len(), 344);
        assert!(encoded.is_ascii());
        assert!(!encoded.contains(' '));
    }
}
