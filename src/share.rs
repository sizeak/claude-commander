//! Multi-user session sharing — join code parsing and formatting.
//!
//! A `JoinCode` bundles everything the joiner needs to attach to a session
//! that's been "invited" by another user: the cloudflared tunnel hostname,
//! port, the freshly-provisioned Linux username, the ed25519 private key
//! the joiner uses for SSH auth, the inviter's tmux socket path, and the
//! tmux session name to attach to.
//!
//! Wire format: `cc-share://v1?h=<host>&p=<port>&u=<user>&k=<key>&s=<sock>&t=<tmux>`
//! with each value percent-encoded. Versioned (`v1`) so the schema can
//! evolve without breaking older clients silently.

use std::fmt;
use std::path::PathBuf;

/// Everything the joiner's claude-commander needs to attach to a shared
/// session. Held in memory only; the private key never gets written to
/// disk on the joiner side until the moment of `ssh -i <tempfile>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinCode {
    /// Cloudflared tunnel hostname (e.g. `*.trycloudflare.com`).
    pub host: String,
    /// Local port the joiner will forward to (cloudflared client target).
    /// Encoded so the inviter can choose a free port; in practice the
    /// joiner picks one too.
    pub port: u16,
    /// Linux username of the freshly-provisioned share user on the
    /// inviter's codespace (e.g. `ccshare-a1b2c3`).
    pub user: String,
    /// Linux username of the codespace's *owner* (e.g. `vscode`,
    /// `sizeak`, etc.). The share user runs `sudo -u <inviter_user>
    /// tmux …` so tmux's process uid matches the socket's owner —
    /// otherwise tmux refuses with "access not allowed". The share
    /// user's sudoers entry only authorises this single command, so
    /// embedding the inviter's username here doesn't widen blast
    /// radius beyond what the inviter explicitly granted.
    pub inviter_user: String,
    /// PEM-style OpenSSH ed25519 private key matching the public key
    /// installed in `/home/<user>/.ssh/authorized_keys`.
    pub private_key: String,
    /// Path to the inviter's tmux socket on the codespace (e.g.
    /// `/tmp/tmux-1000/default`). Required because the joiner connects as
    /// a different uid and tmux's default socket is per-uid.
    pub socket: PathBuf,
    /// Tmux session name to attach to (e.g. `cc-deadbeef`).
    pub tmux_session: String,
}

const SCHEME: &str = "cc-share://v1?";

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    BadScheme,
    MissingField(&'static str),
    BadPort,
    BadEncoding,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadScheme => write!(f, "expected `cc-share://v1?...` URL"),
            Self::MissingField(name) => write!(f, "missing field `{name}`"),
            Self::BadPort => write!(f, "invalid port"),
            Self::BadEncoding => write!(f, "malformed percent-encoding"),
        }
    }
}

impl std::error::Error for ParseError {}

impl JoinCode {
    /// Format as a single-line `cc-share://v1?…` URL safe for Slack paste.
    pub fn to_url(&self) -> String {
        let mut s = String::from(SCHEME);
        push_field(&mut s, "h", &self.host);
        push_field(&mut s, "p", &self.port.to_string());
        push_field(&mut s, "u", &self.user);
        push_field(&mut s, "i", &self.inviter_user);
        push_field(&mut s, "k", &self.private_key);
        push_field(&mut s, "s", self.socket.to_str().unwrap_or_default());
        push_field(&mut s, "t", &self.tmux_session);
        // Trim trailing `&`.
        if s.ends_with('&') {
            s.pop();
        }
        s
    }

    /// Parse a `cc-share://v1?…` URL. Tolerant of extra unknown fields so
    /// future schema additions don't break older clients.
    pub fn from_url(input: &str) -> Result<Self, ParseError> {
        let body = input.strip_prefix(SCHEME).ok_or(ParseError::BadScheme)?;

        let mut host: Option<String> = None;
        let mut port: Option<u16> = None;
        let mut user: Option<String> = None;
        let mut inviter_user: Option<String> = None;
        let mut private_key: Option<String> = None;
        let mut socket: Option<String> = None;
        let mut tmux_session: Option<String> = None;

        for pair in body.split('&') {
            if pair.is_empty() {
                continue;
            }
            let Some((k, v)) = pair.split_once('=') else {
                continue; // tolerate flag-only fields
            };
            let v = percent_decode(v)?;
            match k {
                "h" => host = Some(v),
                "p" => port = Some(v.parse().map_err(|_| ParseError::BadPort)?),
                "u" => user = Some(v),
                "i" => inviter_user = Some(v),
                "k" => private_key = Some(v),
                "s" => socket = Some(v),
                "t" => tmux_session = Some(v),
                _ => {} // unknown — silently ignore for forward compat
            }
        }

        Ok(JoinCode {
            host: host.ok_or(ParseError::MissingField("h"))?,
            port: port.ok_or(ParseError::MissingField("p"))?,
            user: user.ok_or(ParseError::MissingField("u"))?,
            inviter_user: inviter_user.ok_or(ParseError::MissingField("i"))?,
            private_key: private_key.ok_or(ParseError::MissingField("k"))?,
            socket: PathBuf::from(socket.ok_or(ParseError::MissingField("s"))?),
            tmux_session: tmux_session.ok_or(ParseError::MissingField("t"))?,
        })
    }
}

fn push_field(s: &mut String, key: &str, value: &str) {
    s.push_str(key);
    s.push('=');
    percent_encode_into(s, value);
    s.push('&');
}

/// RFC 3986 unreserved + a few common-in-paths characters left as-is.
/// Everything else gets `%XX`-encoded.
fn percent_encode_into(out: &mut String, s: &str) {
    for b in s.bytes() {
        if is_url_safe(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0x0f));
        }
    }
}

fn is_url_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

fn hex_digit(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => unreachable!(),
    }
}

fn percent_decode(s: &str) -> Result<String, ParseError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(ParseError::BadEncoding);
            }
            let hi = from_hex(bytes[i + 1]).ok_or(ParseError::BadEncoding)?;
            let lo = from_hex(bytes[i + 2]).ok_or(ParseError::BadEncoding)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ParseError::BadEncoding)
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> JoinCode {
        JoinCode {
            host: "abc-def-ghi.trycloudflare.com".to_string(),
            port: 22,
            user: "ccshare-a1b2c3".to_string(),
            inviter_user: "vscode".to_string(),
            private_key: concat!(
                "-----BEGIN OPENSSH PRIVATE KEY-----\n",
                "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n",
                "QyNTUxOQAAACBahQ4eDxxJjZ8gXGYxOiKdHnvrf48EpDX7yXDTaCgrAQAAAJDqnKsj6pyr\n",
                "-----END OPENSSH PRIVATE KEY-----\n",
            )
            .to_string(),
            socket: PathBuf::from("/tmp/tmux-1000/default"),
            tmux_session: "cc-deadbeef".to_string(),
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let original = sample();
        let url = original.to_url();
        assert!(url.starts_with("cc-share://v1?"));
        let parsed = JoinCode::from_url(&url).expect("round-trip");
        assert_eq!(parsed, original);
    }

    #[test]
    fn url_is_single_line_no_special_break_chars() {
        // Slack paste / IRC / email survives if there are no spaces or
        // unescaped newlines in the URL.
        let url = sample().to_url();
        assert!(!url.contains(' '));
        assert!(!url.contains('\n'));
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert_eq!(
            JoinCode::from_url("https://example.com").unwrap_err(),
            ParseError::BadScheme,
        );
        // v0 (or any non-v1) shouldn't pretend to be parseable.
        assert_eq!(
            JoinCode::from_url("cc-share://v2?h=foo").unwrap_err(),
            ParseError::BadScheme,
        );
    }

    #[test]
    fn rejects_missing_required_field() {
        // Drop the `k=` (private key) field.
        let url = "cc-share://v1?h=foo&p=22&u=bob&i=alice&s=/tmp/sock&t=cc-x";
        assert_eq!(
            JoinCode::from_url(url).unwrap_err(),
            ParseError::MissingField("k"),
        );
    }

    #[test]
    fn rejects_bad_port() {
        let url = "cc-share://v1?h=foo&p=notanumber&u=bob&k=K&s=/sock&t=cc-x";
        assert_eq!(JoinCode::from_url(url).unwrap_err(), ParseError::BadPort);
    }

    #[test]
    fn ignores_unknown_fields_for_forward_compat() {
        // A future schema might add `x=`; older clients should still parse.
        let mut url = sample().to_url();
        url.push_str("&x=ignoreme");
        assert!(JoinCode::from_url(&url).is_ok());
    }

    #[test]
    fn percent_encoding_handles_special_chars() {
        let mut code = sample();
        // A tmux name with `.` and `/` is unusual but we shouldn't choke.
        code.tmux_session = "cc/with.dots".to_string();
        code.socket = PathBuf::from("/tmp/path with spaces/sock");
        let url = code.to_url();
        assert!(!url.contains(' '), "spaces must be encoded: {url}");
        let parsed = JoinCode::from_url(&url).unwrap();
        assert_eq!(parsed, code);
    }

    #[test]
    fn private_key_with_newlines_round_trips() {
        // ed25519 private keys span multiple lines. Newlines must survive
        // the encode/decode without mangling.
        let code = sample();
        let url = code.to_url();
        let parsed = JoinCode::from_url(&url).unwrap();
        assert_eq!(parsed.private_key, code.private_key);
        assert!(
            parsed.private_key.contains('\n'),
            "newlines preserved across round trip"
        );
    }

    #[test]
    fn malformed_percent_encoding_errors() {
        // `%9` is incomplete (needs 2 hex chars).
        let url = "cc-share://v1?h=foo&p=22&u=bob&k=ab%9&s=/s&t=t";
        assert_eq!(
            JoinCode::from_url(url).unwrap_err(),
            ParseError::BadEncoding
        );
        // Non-hex after `%`.
        let url = "cc-share://v1?h=foo&p=22&u=bob&k=ab%ZZ&s=/s&t=t";
        assert_eq!(
            JoinCode::from_url(url).unwrap_err(),
            ParseError::BadEncoding
        );
    }
}
