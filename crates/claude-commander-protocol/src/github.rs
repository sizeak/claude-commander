//! GitHub repo listing + clone wire contract for the "add a project" flow.
//!
//! This lives in the protocol crate for the same reason [`crate::paste`] does:
//! the rules here are *agreed by every party*, not owned by one of them. The
//! server validates a [`CloneRequest`] before it shells out to `git`/`gh`, the
//! Rust client validates before it sends, and the Flutter client validates as
//! the user types so a doomed request never leaves the device. One definition,
//! enforced independently in several places.
//!
//! Two of the three functions here are security boundaries, not conveniences:
//!
//! * [`validate_clone_url`] and [`validate_repo_slug`] gate strings that become
//!   **argv elements** of a `git clone` / `gh repo clone` invocation, and whose
//!   derived directory name becomes a **path** under the user's projects dir.
//!   Both hazards (a leading `-` read as a flag; a `..` segment escaping the
//!   projects dir) are cheap to get wrong and invisible when you do.
//! * [`canonical_repo_slug`] is not a security boundary — it exists so the repo
//!   picker can tell "you already have this one" reliably. `gh repo clone`
//!   honours the user's configured `git_protocol`, so a project cloned by `gh`
//!   often has an `ssh://` origin while the API reports an `https://` clone URL.
//!   Comparing the raw strings misses every such match.
//!
//! Everything here is pure string inspection — no filesystem, no network, no
//! `url` crate — so it cross-compiles wherever the protocol crate does. The
//! *effectful* half (running `gh repo list`, running the clone, registering the
//! resulting project) stays in `claude-commander-core`.

use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::session::ProjectId;

/// URL schemes a clone source may use.
///
/// Deliberately an allow-list rather than a deny-list: git supports transports
/// we have no business invoking from a GUI action (`ext::` runs an arbitrary
/// command, and helper-provided transports vary by machine), so anything not
/// named here is refused.
///
/// `http` is admitted alongside `https` because self-hosted GitHub Enterprise
/// and Gitea instances on internal networks are commonly plain HTTP; the
/// transport-security tradeoff is the user's, made explicitly by typing the URL.
/// `file` is admitted so the clone path is exercisable in tests without network.
///
/// Public for the same reason [`MAX_IMAGE_BYTES`](crate::paste::MAX_IMAGE_BYTES)
/// is: it is part of the contract, so a frontend can tell the user what it
/// accepts rather than maintaining its own copy of the list.
pub const CLONE_SCHEMES: &[&str] = &["https", "http", "ssh", "git", "file"];

/// A GitHub repository as offered by the repo picker.
///
/// Field names match the GitHub REST API's repo object so a `gh api` / `gh repo
/// list --json` payload deserializes into this directly.
///
/// FLUTTER: mirror this DTO in the Dart model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubRepo {
    /// `owner/name`, the form `gh repo clone` takes as an argument.
    pub full_name: String,
    pub owner: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub private: bool,
    pub fork: bool,
    pub archived: bool,
    pub default_branch: String,
    /// HTTPS clone URL, as reported by the API.
    pub clone_url: String,
    /// SSH clone URL (`git@github.com:owner/name.git`).
    pub ssh_url: String,
    /// Last push time. `None` for a repo with no commits — the API returns
    /// `null` there, and a picker sorted by recency has to cope with it rather
    /// than failing to decode the whole page.
    #[serde(default)]
    pub pushed_at: Option<DateTime<Utc>>,
}

/// Where a clone should come from.
///
/// The two arms are genuinely different *invocations*, not two spellings of one:
/// [`CloneSource::Github`] runs `gh repo clone` (which resolves the user's
/// configured protocol and credentials), while [`CloneSource::Url`] runs a plain
/// `git clone`. Keeping them distinct on the wire means the server never has to
/// guess which tool the string was meant for.
///
/// FLUTTER: mirror this DTO in the Dart model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CloneSource {
    /// An `owner/name` slug — validate with [`validate_repo_slug`].
    Github { full_name: String },
    /// Any other clone source — validate with [`validate_clone_url`].
    Url { url: String },
}

/// Request body for starting a clone.
///
/// FLUTTER: mirror this DTO in the Dart model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneRequest {
    pub source: CloneSource,
    /// Override for the destination directory name. `None` means "use the name
    /// derived from the source" ([`CloneTarget::default_dir_name`], or the repo
    /// name for a [`CloneSource::Github`]). A supplied name is subject to the
    /// same safety rules as a derived one.
    #[serde(default)]
    pub dest_name: Option<String>,
}

/// Identifier for an in-flight clone.
///
/// Unlike [`ProjectId`]/[`SessionId`](crate::session::SessionId), whose `Display`
/// truncates to an 8-char prefix for the session tree, this one displays the
/// **full** UUID: a clone job id is a URL path segment (`GET /clone/{id}`), and
/// a truncating `Display` would silently build routes that 404.
///
/// The inner `Uuid` is `pub` so flutter_rust_bridge can mirror the newtype;
/// prefer the `from_uuid`/`as_uuid` accessors in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CloneJobId(pub Uuid);

impl CloneJobId {
    /// Create a new random clone-job ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Get the inner UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for CloneJobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CloneJobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How a clone is going.
///
/// [`CloneStatus::DestinationExists`] is a distinct arm rather than a
/// [`CloneStatus::Failed`] message because it is the one outcome a frontend can
/// *act* on: the directory is already there, and whether it is a git repo
/// decides whether the sensible offer is "add the existing checkout as a
/// project" or "pick a different name".
///
/// FLUTTER: mirror this DTO in the Dart model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CloneStatus {
    /// Clone in progress.
    Running,
    /// Clone finished and the checkout was registered as a project.
    Succeeded { project_id: ProjectId },
    /// Clone failed; `message` is the user-facing reason.
    Failed { message: String },
    /// Nothing was cloned because the destination path is already occupied.
    DestinationExists { dest: PathBuf, is_git_repo: bool },
}

/// A clone, as reported back to a frontend polling for progress.
///
/// FLUTTER: mirror this DTO in the Dart model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneJob {
    pub id: CloneJobId,
    /// What to show the user as the source — the `owner/name` slug or the URL.
    pub source_label: String,
    /// Absolute destination path the clone is writing to.
    pub dest: PathBuf,
    pub status: CloneStatus,
}

/// An accepted clone source, with the directory name derived from it.
///
/// Only [`validate_clone_url`] produces one, so holding a `CloneTarget` is proof
/// the checks ran: `source` does not begin with `-` and `default_dir_name` is a
/// single path component that cannot escape the projects directory.
///
/// `source` is the caller's string unchanged (bar surrounding whitespace) — this
/// crate has no filesystem access, so it never resolves or canonicalises a path.
///
/// FLUTTER: mirror this DTO in the Dart model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneTarget {
    pub source: String,
    pub default_dir_name: String,
}

/// Why a would-be clone source was refused.
///
/// Carries no borrowed data so callers can map it into their own error type, and
/// is implemented by hand rather than via `thiserror`: this crate is
/// deliberately dependency-light (serde only), and these messages are part of
/// the 400 response body users see. Same precedent as
/// [`ImageRejection`](crate::paste::ImageRejection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneRejection {
    /// Empty or whitespace-only.
    Empty,
    /// Begins with `-`, so git would read it as a flag rather than a source.
    LooksLikeFlag,
    /// Contains a control character (NUL, newline, …). Never legitimate in a
    /// clone source, and a newline in particular corrupts logs downstream.
    ControlCharacter,
    /// A local source that is not an absolute path. This also catches git's
    /// scp-style shorthand written by accident (`foo:bar`), which git resolves
    /// by ssh-ing to host `foo` rather than as the relative path it looks like.
    NotAbsolute,
    /// URL scheme outside [`CLONE_SCHEMES`].
    UnsupportedScheme { scheme: String },
    /// A network URL with no host (`https:///owner/repo`).
    MissingHost,
    /// The destination directory name derived from the source is unusable — it
    /// is empty, or it would escape the projects directory.
    UnsafeDirectoryName { name: String },
    /// Not a plain `owner/name` GitHub slug.
    MalformedSlug { slug: String },
}

impl fmt::Display for CloneRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "no clone source given"),
            Self::LooksLikeFlag => {
                write!(
                    f,
                    "a clone source cannot start with '-' (git would read it as an option)"
                )
            }
            Self::ControlCharacter => {
                write!(f, "a clone source cannot contain control characters")
            }
            Self::NotAbsolute => write!(
                f,
                "expected a URL, an ssh remote, or an absolute path (a relative path is ambiguous \
                 with git's `host:path` ssh shorthand)"
            ),
            Self::UnsupportedScheme { scheme } => {
                let allowed = CLONE_SCHEMES.join(", ");
                write!(
                    f,
                    "unsupported scheme '{scheme}' (expected one of: {allowed})"
                )
            }
            Self::MissingHost => write!(f, "clone URL has no host"),
            Self::UnsafeDirectoryName { name } if name.is_empty() => {
                write!(f, "cannot work out a directory name to clone into")
            }
            Self::UnsafeDirectoryName { name } => {
                write!(f, "'{name}' is not a safe directory name to clone into")
            }
            Self::MalformedSlug { slug } => {
                write!(f, "'{slug}' is not an owner/name repository slug")
            }
        }
    }
}

impl std::error::Error for CloneRejection {}

/// How git would read a clone source. Both [`validate_clone_url`] and
/// [`canonical_repo_slug`] classify through here so they can never disagree
/// about what a given string *is* — one deciding a source is an ssh remote while
/// the other treats it as a local path is exactly how the "already added" badge
/// goes wrong.
#[derive(Debug)]
enum SourceShape<'a> {
    /// `<scheme>://[user@]host[:port]/<path>`.
    Url {
        scheme: &'a str,
        /// Host with any `user@` and `:port` stripped, as written (not lowered).
        host: &'a str,
        path: &'a str,
    },
    /// git's scp-like ssh shorthand, `[user@]host:<path>`.
    Scp { host: &'a str, path: &'a str },
    /// An absolute filesystem path.
    LocalPath(&'a str),
    /// A relative path, a bare word, or anything else git would not accept as a
    /// remote.
    Other,
}

/// Strip `user[:password]@` and `:port` from a URL authority, leaving the host.
fn host_of(authority: &str) -> &str {
    // rsplit: a userinfo component may itself contain '@' when percent-decoded
    // sloppily, and the *last* '@' is the delimiter either way.
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // An IPv6 literal is bracketed, and its colons are not port delimiters.
    if let Some(rest) = host_port.strip_prefix('[')
        && let Some(close) = rest.find(']')
    {
        return &rest[..close];
    }
    host_port.split_once(':').map_or(host_port, |(h, _)| h)
}

/// Decide which form a clone source is written in.
fn classify(source: &str) -> SourceShape<'_> {
    if let Some((scheme, rest)) = source.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        return SourceShape::Url {
            scheme,
            host: host_of(authority),
            path,
        };
    }
    if source.starts_with('/') {
        return SourceShape::LocalPath(source);
    }
    // A colon before the first slash is git's scp shorthand. It is also what a
    // mistyped relative path looks like, and git resolves the ambiguity by
    // ssh-ing — so we only accept the reading that is unambiguous to a human:
    // an explicit `user@`, or a dotted hostname. `foo:bar` is neither, and a
    // host that genuinely has no dot (`git@localhost:repo`) can be written in
    // the explicit `ssh://` form.
    if let Some((authority, path)) = source.split_once(':')
        && !authority.contains('/')
        && (authority.contains('@') || host_of(authority).contains('.'))
    {
        return SourceShape::Scp {
            host: host_of(authority),
            path,
        };
    }
    SourceShape::Other
}

/// Last path component of a clone source's path, with any `.git` suffix removed.
///
/// `strip_suffix` rather than `trim_end_matches`, which would strip repeatedly
/// and turn `repo.git.git` into `repo`.
fn derive_dir_name(path: &str) -> &str {
    let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    last.strip_suffix(".git").unwrap_or(last)
}

/// Reject a directory name that is empty or would not stay put inside the
/// projects directory. `..` is the one that matters; `.` and an embedded
/// separator are the same class of mistake, and a leading `-` is the argv hazard
/// again for anything that later passes the name to a command.
fn check_dir_name(name: &str) -> Result<(), CloneRejection> {
    let unsafe_name = name.is_empty()
        || name == "."
        || name == ".."
        || name.starts_with('-')
        || name.contains(['/', '\\']);
    if unsafe_name {
        return Err(CloneRejection::UnsafeDirectoryName {
            name: name.to_string(),
        });
    }
    Ok(())
}

/// Validate a free-form clone source and derive its default directory name.
///
/// Accepts network URLs on [`CLONE_SCHEMES`], git's `[user@]host:path` ssh
/// shorthand, and absolute local paths (`file://` included, so the clone path is
/// testable without network). Everything else is refused — see
/// [`CloneRejection`] for what and why.
pub fn validate_clone_url(source: &str) -> Result<CloneTarget, CloneRejection> {
    let source = source.trim();
    if source.is_empty() {
        return Err(CloneRejection::Empty);
    }
    // Checked before anything else: a leading '-' makes the string an *option*
    // to git no matter how well-formed the rest of it looks.
    if source.starts_with('-') {
        return Err(CloneRejection::LooksLikeFlag);
    }
    if source.chars().any(char::is_control) {
        return Err(CloneRejection::ControlCharacter);
    }

    let path = match classify(source) {
        SourceShape::Url { scheme, host, path } => {
            if !CLONE_SCHEMES.iter().any(|s| scheme.eq_ignore_ascii_case(s)) {
                return Err(CloneRejection::UnsupportedScheme {
                    scheme: scheme.to_string(),
                });
            }
            // `file://` is the one scheme with no host: `file:///srv/mirrors/r`
            // has an empty authority by construction.
            if host.is_empty() && !scheme.eq_ignore_ascii_case("file") {
                return Err(CloneRejection::MissingHost);
            }
            path
        }
        SourceShape::Scp { host, path } => {
            // The same rule as the `Url` arm, and for the same reason: `git@:o/r`
            // is scp-shaped with an empty host. Accepting it here while
            // `canonical_repo_slug` returns `None` for it is precisely the drift
            // `SourceShape` exists to prevent.
            if host.is_empty() {
                return Err(CloneRejection::MissingHost);
            }
            path
        }
        SourceShape::LocalPath(path) => path,
        SourceShape::Other => return Err(CloneRejection::NotAbsolute),
    };

    let default_dir_name = derive_dir_name(path);
    check_dir_name(default_dir_name)?;

    Ok(CloneTarget {
        source: source.to_string(),
        default_dir_name: default_dir_name.to_string(),
    })
}

/// Validate an `owner/name` GitHub slug destined for `gh repo clone`'s argv.
///
/// Exactly two segments, each restricted to the characters GitHub actually
/// allows in owner and repository names, and each subject to the same
/// directory-name check a URL-derived name gets — so neither may be empty,
/// start with `-` (which would make the slug an option), or be `.`/`..`.
///
/// The check is applied to *both* segments, not just the one that becomes the
/// clone directory. Only `name` builds a path today, but a rule that holds for
/// half a slug is one refactor away from being wrong, and the asymmetry is
/// invisible at the call site.
pub fn validate_repo_slug(slug: &str) -> Result<(), CloneRejection> {
    let malformed = || CloneRejection::MalformedSlug {
        slug: slug.to_string(),
    };
    let (owner, name) = slug.split_once('/').ok_or_else(malformed)?;
    if name.contains('/') {
        return Err(malformed());
    }
    for segment in [owner, name] {
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(malformed());
        }
        // Covers empty, a leading `-`, `.`/`..`, and embedded separators.
        check_dir_name(segment).map_err(|_| malformed())?;
    }
    Ok(())
}

/// Reduce a clone source to a stable `host/owner/name` identity, or `None` when
/// it has none.
///
/// This is what makes "you already have this repo" reliable. A project's stored
/// origin and a picker row's `clone_url` describe the same repository through
/// different spellings — ssh vs https, `.git` suffix or not, and GitHub's
/// case-insensitive owner/name — so the badge has to compare canonical forms,
/// not strings.
///
/// Returns `None` for local paths and `file://` URLs. That is intentional rather
/// than a gap: a local clone source has no GitHub identity, so it simply never
/// earns an "already added" badge.
pub fn canonical_repo_slug(source: &str) -> Option<String> {
    let (host, path) = match classify(source.trim()) {
        // A `file://` URL is a local checkout wearing a URL's clothes.
        SourceShape::Url { scheme, .. } if scheme.eq_ignore_ascii_case("file") => return None,
        SourceShape::Url { host, path, .. } => (host, path),
        SourceShape::Scp { host, path } => (host, path),
        SourceShape::LocalPath(_) | SourceShape::Other => return None,
    };
    if host.is_empty() {
        return None;
    }
    // Lowercase *before* stripping, not after. Hostnames, GitHub owner/repo
    // names and the `.git` suffix are all ASCII and case-insensitive, but a
    // trailing lowercase pass would strip nothing from `repo.GIT` and then lower
    // it to `repo.git` — a second identity for the same repo, which is exactly
    // what this function exists to prevent.
    let host = host.to_ascii_lowercase();
    let lowered = path.to_ascii_lowercase();
    let path = lowered.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    // A `.git` that was its own trailing segment (`o/repo/.git`) leaves the
    // separator behind, so trim again.
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalises_every_url_form_to_one_slug() {
        // The badge bug in one test: an SSH origin and an HTTPS clone_url for the
        // same repo must compare equal, because `gh repo clone` honours the user's
        // configured `git_protocol` (often ssh) while the API returns https.
        let expect = Some("github.com/sizeak/claude-commander".to_string());
        for form in [
            "git@github.com:sizeak/claude-commander.git",
            "ssh://git@github.com/sizeak/claude-commander",
            "https://github.com/sizeak/claude-commander.git",
            "https://github.com/SizeAk/Claude-Commander",
        ] {
            assert_eq!(canonical_repo_slug(form), expect, "form: {form}");
        }
    }

    #[test]
    fn canonicalises_non_remote_sources_to_none() {
        assert_eq!(canonical_repo_slug("/srv/mirrors/foo"), None);
        assert_eq!(canonical_repo_slug("not a url"), None);
    }

    #[test]
    fn accepts_remote_and_absolute_local_sources() {
        assert_eq!(
            validate_clone_url("https://github.com/o/r.git")
                .unwrap()
                .default_dir_name,
            "r"
        );
        assert!(validate_clone_url("git@github.com:o/r.git").is_ok());
        assert!(validate_clone_url("ssh://git@github.com/o/r").is_ok());
        // Absolute local paths and file:// are admitted so the clone path is
        // testable without network.
        assert!(validate_clone_url("/srv/mirrors/r").is_ok());
        assert!(validate_clone_url("file:///srv/mirrors/r").is_ok());
    }

    #[test]
    fn rejects_argv_and_path_traversal_hazards() {
        // A source beginning with `-` would read as a git flag.
        assert!(validate_clone_url("--upload-pack=evil").is_err());
        // A *relative* path with a colon before the first slash is scp syntax to
        // git: it would try to ssh to host `foo`. Local sources must be absolute.
        assert!(validate_clone_url("foo:bar").is_err());
        assert!(validate_clone_url("relative/path").is_err());
        // Derived directory name must not escape the projects dir.
        assert!(validate_clone_url("https://example.com/o/..").is_err());
        assert!(validate_clone_url("ftp://example.com/o/r").is_err());
    }

    #[test]
    fn validates_repo_slug_shape_for_gh_argv() {
        assert!(validate_repo_slug("owner/name").is_ok());
        assert!(validate_repo_slug("-flag/name").is_err());
        assert!(validate_repo_slug("owner").is_err());
        assert!(validate_repo_slug("owner/name/extra").is_err());
        assert!(validate_repo_slug("owner/na me").is_err());
    }

    #[test]
    fn rejection_displays_without_thiserror() {
        let msg = validate_clone_url("ftp://example.com/o/r")
            .unwrap_err()
            .to_string();
        assert!(!msg.is_empty());
    }

    /// Each rejection is a *distinct* reason, not just "is_err". Without this the
    /// tests above would pass with a validator that refused everything for one
    /// blanket reason, and the 400 bodies users read would be useless.
    #[test]
    fn each_hazard_reports_its_own_reason() {
        for (source, expected) in [
            ("", CloneRejection::Empty),
            ("   ", CloneRejection::Empty),
            ("--upload-pack=evil", CloneRejection::LooksLikeFlag),
            // Interior only: a *trailing* newline is trimmed, because pasting a
            // URL routinely brings one along.
            ("https://x.com/o\n/r", CloneRejection::ControlCharacter),
            ("https://x.com/o/\0r", CloneRejection::ControlCharacter),
            ("foo:bar", CloneRejection::NotAbsolute),
            ("relative/path", CloneRejection::NotAbsolute),
            (
                "ftp://example.com/o/r",
                CloneRejection::UnsupportedScheme {
                    scheme: "ftp".to_string(),
                },
            ),
            ("https:///o/r", CloneRejection::MissingHost),
            (
                "https://example.com/o/..",
                CloneRejection::UnsafeDirectoryName {
                    name: "..".to_string(),
                },
            ),
            (
                "https://example.com",
                CloneRejection::UnsafeDirectoryName {
                    name: String::new(),
                },
            ),
        ] {
            assert_eq!(
                validate_clone_url(source),
                Err(expected),
                "source: {source:?}"
            );
        }
    }

    /// Every rejection renders a non-empty, distinct message — these strings are
    /// the 400 body, so a variant that fell through to a shared blank would be a
    /// user-visible regression.
    #[test]
    fn every_rejection_has_its_own_message() {
        let all = [
            CloneRejection::Empty,
            CloneRejection::LooksLikeFlag,
            CloneRejection::ControlCharacter,
            CloneRejection::NotAbsolute,
            CloneRejection::UnsupportedScheme {
                scheme: "ftp".to_string(),
            },
            CloneRejection::MissingHost,
            CloneRejection::UnsafeDirectoryName {
                name: String::new(),
            },
            CloneRejection::UnsafeDirectoryName {
                name: "..".to_string(),
            },
            CloneRejection::MalformedSlug {
                slug: "owner".to_string(),
            },
        ];
        let messages: Vec<String> = all.iter().map(|r| r.to_string()).collect();
        assert!(messages.iter().all(|m| !m.is_empty()));
        for (i, a) in messages.iter().enumerate() {
            for b in &messages[i + 1..] {
                assert_ne!(a, b, "two rejections share a message");
            }
        }
        // The unsafe-name arms are worded for their case: an empty derived name
        // must not render as an empty-quoted nonsense string.
        assert_eq!(
            CloneRejection::UnsafeDirectoryName {
                name: String::new()
            }
            .to_string(),
            "cannot work out a directory name to clone into"
        );
        assert!(
            CloneRejection::UnsafeDirectoryName {
                name: "..".to_string()
            }
            .to_string()
            .contains("'..'")
        );
        // The scheme message names what *is* allowed, so the user can fix it.
        let scheme_msg = CloneRejection::UnsupportedScheme {
            scheme: "ftp".to_string(),
        }
        .to_string();
        assert!(scheme_msg.contains("ftp") && scheme_msg.contains("https"));
    }

    /// The directory name is derived from the *last* path segment, across every
    /// source shape, with at most one `.git` stripped.
    #[test]
    fn derives_directory_name_from_every_source_shape() {
        for (source, expected) in [
            ("https://github.com/o/repo.git", "repo"),
            ("https://github.com/o/repo", "repo"),
            ("https://github.com/o/repo/", "repo"),
            ("git@github.com:o/repo.git", "repo"),
            ("ssh://git@github.com:2222/o/repo.git", "repo"),
            ("/srv/mirrors/repo", "repo"),
            ("/srv/mirrors/repo.git", "repo"),
            ("file:///srv/mirrors/repo.git", "repo"),
            // Only one suffix is stripped — `trim_end_matches` would eat both.
            ("https://github.com/o/repo.git.git", "repo.git"),
            // A dotfile-looking repo name is legal on GitHub and stays put.
            ("https://github.com/o/.github", ".github"),
        ] {
            assert_eq!(
                validate_clone_url(source).unwrap().default_dir_name,
                expected,
                "source: {source}"
            );
        }
    }

    /// `source` comes back as typed (bar surrounding whitespace): this crate has
    /// no filesystem, so it must not invent a normalised or resolved form that
    /// the caller would then hand to git.
    #[test]
    fn target_source_is_the_input_unchanged_apart_from_trimming() {
        let target = validate_clone_url("  git@github.com:o/repo.git\n").unwrap();
        assert_eq!(target.source, "git@github.com:o/repo.git");
        assert_eq!(target.default_dir_name, "repo");
    }

    /// The allow-list is an allow-list: every named scheme works and unnamed
    /// ones do not — including `ext::`, which git would otherwise honour by
    /// running an arbitrary command.
    #[test]
    fn scheme_allow_list_is_exhaustive_and_case_insensitive() {
        for scheme in CLONE_SCHEMES {
            let source = if *scheme == "file" {
                "file:///srv/mirrors/repo".to_string()
            } else {
                format!("{scheme}://example.com/o/repo")
            };
            assert!(validate_clone_url(&source).is_ok(), "scheme: {scheme}");
        }
        // Schemes are case-insensitive per RFC 3986.
        assert!(validate_clone_url("HTTPS://example.com/o/repo").is_ok());
        for source in [
            "ext::sh -c 'evil'",
            "ftp://example.com/o/r",
            "javascript://example.com/o/r",
        ] {
            assert!(validate_clone_url(source).is_err(), "source: {source}");
        }
    }

    /// The scp/relative ambiguity, pinned from both sides: git reads
    /// `host:path` as ssh, so we accept it only when the reading is unambiguous
    /// to a human (explicit `user@`, or a dotted hostname).
    #[test]
    fn scp_shorthand_accepted_only_when_unambiguous() {
        assert!(validate_clone_url("git@github.com:o/repo.git").is_ok());
        assert!(validate_clone_url("github.com:o/repo.git").is_ok());
        assert!(validate_clone_url("git@localhost:repo.git").is_ok());
        // No `@`, no dot: overwhelmingly a mistyped relative path.
        assert_eq!(
            validate_clone_url("foo:bar"),
            Err(CloneRejection::NotAbsolute)
        );
        assert_eq!(
            validate_clone_url("localhost:repo"),
            Err(CloneRejection::NotAbsolute)
        );
        // The explicit form is always available as the escape hatch.
        assert!(validate_clone_url("ssh://git@localhost/repo").is_ok());
    }

    /// `git@:o/r` is scp-shaped but has an *empty* host. Accepting it here while
    /// `canonical_repo_slug` returns `None` for it is exactly the drift
    /// `SourceShape` exists to prevent — the clone would be attempted and the
    /// resulting project could never match a picker row. The `Url` arm already
    /// refused a hostless `https:///o/r`; the `Scp` arm must agree.
    #[test]
    fn scp_source_with_an_empty_host_is_rejected_and_has_no_identity() {
        for source in ["git@:o/r", "git@:o/r.git", "git@:repo"] {
            assert_eq!(
                validate_clone_url(source),
                Err(CloneRejection::MissingHost),
                "source: {source}"
            );
            assert_eq!(canonical_repo_slug(source), None, "source: {source}");
        }
    }

    /// `canonical_repo_slug` and `validate_clone_url` classify through the same
    /// helper, so a source that validates as a *remote* also has an identity,
    /// and a local one has none. A drift between the two is precisely how the
    /// "already added" badge misfires.
    #[test]
    fn canonicalisation_agrees_with_validation_about_what_is_remote() {
        for remote in [
            "https://github.com/o/repo.git",
            "git@github.com:o/repo.git",
            "ssh://git@github.com/o/repo",
            "github.com:o/repo",
        ] {
            assert!(validate_clone_url(remote).is_ok(), "source: {remote}");
            assert!(
                canonical_repo_slug(remote).is_some(),
                "no identity for: {remote}"
            );
        }
        for local in ["/srv/mirrors/repo", "file:///srv/mirrors/repo"] {
            assert!(validate_clone_url(local).is_ok(), "source: {local}");
            assert_eq!(canonical_repo_slug(local), None, "source: {local}");
        }
    }

    /// A port, a trailing slash and mixed case must not split one repo into two
    /// identities.
    ///
    /// The `.GIT` cases are the sharp ones: the suffix strip and the lowercase
    /// pass have to happen in the right order, or `repo.GIT` keeps its suffix
    /// through the strip and only loses its case afterwards — canonicalising to
    /// `…/repo.git`, a second identity for one repo, which defeats the badge
    /// this function exists to power.
    #[test]
    fn canonicalisation_ignores_ports_trailing_slashes_and_case() {
        let expect = Some("example.com/o/repo".to_string());
        for form in [
            "https://example.com/o/repo",
            "https://example.com/o/repo/",
            "https://example.com/o/repo.git",
            "https://EXAMPLE.com:443/O/Repo.git",
            "ssh://git@example.com:2222/o/repo.git",
            "git@example.com:o/repo.git",
            "  https://example.com/o/repo  ",
            // Case-varying `.git` suffix.
            "https://example.com/o/repo.GIT",
            "https://example.com/o/repo.Git",
            "https://EXAMPLE.COM/O/REPO.GIT",
            "git@example.com:o/repo.GIT",
            "https://example.com/o/repo.GIT/",
            // `.git` as its own trailing segment must not leave the slash behind.
            "https://example.com/o/repo/.git",
        ] {
            assert_eq!(canonical_repo_slug(form), expect, "form: {form}");
        }
        // Different repos stay different.
        assert_ne!(
            canonical_repo_slug("https://github.com/o/repo"),
            canonical_repo_slug("https://gitlab.com/o/repo")
        );
        // A host with no path has no repo identity.
        assert_eq!(canonical_repo_slug("https://github.com"), None);
        assert_eq!(canonical_repo_slug("https://github.com/"), None);
    }

    /// An IPv6 literal's colons are not port delimiters.
    #[test]
    fn canonicalisation_handles_bracketed_ipv6_hosts() {
        assert_eq!(
            canonical_repo_slug("ssh://git@[2001:db8::1]:2222/o/repo.git"),
            Some("2001:db8::1/o/repo".to_string())
        );
    }

    #[test]
    fn repo_slug_accepts_the_characters_github_allows() {
        for slug in [
            "owner/name",
            "owner-1/name_2",
            "owner/name.js",
            "owner/.github",
            "Owner/Name",
        ] {
            assert!(validate_repo_slug(slug).is_ok(), "slug: {slug}");
        }
        for slug in [
            "owner",            // no separator
            "owner/name/extra", // too many segments
            "owner/na me",      // whitespace
            "-flag/name",       // argv hazard in the owner
            "owner/-flag",      // argv hazard in the name
            "/name",            // empty owner
            "owner/",           // empty name
            "owner/..",         // traversal in the name
            // Traversal in the *owner*. Not a path today, but the rule is
            // stated for the whole slug, so it has to hold for both segments.
            "../evil",
            "./evil",
            "../..",
            "owner/na;me",  // shell-ish punctuation
            "owner/na\nme", // control character
        ] {
            assert_eq!(
                validate_repo_slug(slug),
                Err(CloneRejection::MalformedSlug {
                    slug: slug.to_string()
                }),
                "slug: {slug}"
            );
        }
    }

    /// A clone job id is a URL path segment, so its `Display` must be the whole
    /// UUID — unlike `ProjectId`/`SessionId`, which truncate for the tree view.
    #[test]
    fn clone_job_id_displays_in_full() {
        let uuid = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        let id = CloneJobId::from_uuid(uuid);
        assert_eq!(id.to_string(), uuid.to_string());
        assert_eq!(id.to_string().len(), 36);
        assert_eq!(*id.as_uuid(), uuid);
        assert!(std::ptr::eq(id.as_uuid(), &id.0));
    }

    #[test]
    fn clone_source_wire_forms() {
        assert_eq!(
            serde_json::to_string(&CloneSource::Github {
                full_name: "o/r".to_string()
            })
            .unwrap(),
            r#"{"kind":"github","full_name":"o/r"}"#
        );
        assert_eq!(
            serde_json::from_str::<CloneSource>(r#"{"kind":"url","url":"https://x/o/r"}"#).unwrap(),
            CloneSource::Url {
                url: "https://x/o/r".to_string()
            }
        );
    }

    /// `dest_name` is `#[serde(default)]`, so a minimal body is valid.
    #[test]
    fn clone_request_round_trips_with_and_without_dest_name() {
        let minimal: CloneRequest =
            serde_json::from_str(r#"{"source":{"kind":"github","full_name":"o/r"}}"#).unwrap();
        assert!(minimal.dest_name.is_none());

        let req = CloneRequest {
            source: CloneSource::Url {
                url: "https://example.com/o/r.git".to_string(),
            },
            dest_name: Some("mine".to_string()),
        };
        let back: CloneRequest =
            serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn clone_status_wire_forms_round_trip() {
        let project_id = ProjectId::new();
        for status in [
            CloneStatus::Running,
            CloneStatus::Succeeded { project_id },
            CloneStatus::Failed {
                message: "boom".to_string(),
            },
            CloneStatus::DestinationExists {
                dest: PathBuf::from("/projects/r"),
                is_git_repo: true,
            },
        ] {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(serde_json::from_str::<CloneStatus>(&json).unwrap(), status);
        }
        assert_eq!(
            serde_json::to_string(&CloneStatus::Running).unwrap(),
            r#"{"kind":"running"}"#
        );
        assert_eq!(
            serde_json::to_string(&CloneStatus::DestinationExists {
                dest: PathBuf::from("/projects/r"),
                is_git_repo: false,
            })
            .unwrap(),
            r#"{"kind":"destination_exists","dest":"/projects/r","is_git_repo":false}"#
        );
    }

    #[test]
    fn clone_job_round_trips() {
        let job = CloneJob {
            id: CloneJobId::new(),
            source_label: "sizeak/claude-commander".to_string(),
            dest: PathBuf::from("/projects/claude-commander"),
            status: CloneStatus::Running,
        };
        let back: CloneJob = serde_json::from_str(&serde_json::to_string(&job).unwrap()).unwrap();
        assert_eq!(back, job);
    }

    /// A repo with no commits has `"pushed_at": null`, and `description` is
    /// routinely null too — a picker page must not fail to decode over either.
    #[test]
    fn github_repo_decodes_an_empty_repo_from_the_api() {
        let wire = r#"{
            "full_name": "sizeak/empty",
            "owner": "sizeak",
            "name": "empty",
            "description": null,
            "private": true,
            "fork": false,
            "archived": false,
            "default_branch": "main",
            "clone_url": "https://github.com/sizeak/empty.git",
            "ssh_url": "git@github.com:sizeak/empty.git",
            "pushed_at": null
        }"#;
        let repo: GithubRepo = serde_json::from_str(wire).unwrap();
        assert!(repo.pushed_at.is_none());
        assert!(repo.description.is_none());
        assert!(repo.private);
        // Both URL forms canonicalise to the same identity, which is what lets
        // the picker badge a repo the user already has.
        assert_eq!(
            canonical_repo_slug(&repo.clone_url),
            canonical_repo_slug(&repo.ssh_url)
        );

        // The optional fields may also be absent entirely.
        let minimal = wire.replace(r#""description": null,"#, "").replace(
            r#",
            "pushed_at": null"#,
            "",
        );
        let repo: GithubRepo = serde_json::from_str(&minimal).unwrap();
        assert!(repo.pushed_at.is_none());
        assert_eq!(repo.full_name, "sizeak/empty");
    }

    #[test]
    fn github_repo_round_trips_with_a_timestamp() {
        let repo = GithubRepo {
            full_name: "sizeak/claude-commander".to_string(),
            owner: "sizeak".to_string(),
            name: "claude-commander".to_string(),
            description: Some("a thing".to_string()),
            private: false,
            fork: false,
            archived: true,
            default_branch: "main".to_string(),
            clone_url: "https://github.com/sizeak/claude-commander.git".to_string(),
            ssh_url: "git@github.com:sizeak/claude-commander.git".to_string(),
            pushed_at: Some(
                DateTime::parse_from_rfc3339("2026-08-08T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
        };
        let back: GithubRepo =
            serde_json::from_str(&serde_json::to_string(&repo).unwrap()).unwrap();
        assert_eq!(back, repo);
        // `full_name` is exactly what `gh repo clone` takes as argv.
        assert!(validate_repo_slug(&back.full_name).is_ok());
    }
}
