//! Tab-completion for filesystem paths in the Add Project modal.

use std::path::{Path, PathBuf};

/// Shell-style tab completion for directory paths.
///
/// Provides:
/// - Tilde (`~`) expansion to the user's home directory
/// - Completion to the longest common prefix on first Tab
/// - Cycling through individual matches on subsequent Tabs
/// - Staleness detection: completions are recomputed when input changes
#[derive(Debug, Clone)]
pub struct PathCompleter {
    /// Matching directory names from the last Tab press
    completions: Vec<String>,
    /// Current cycling index (None = at common prefix stage)
    cycle_index: Option<usize>,
    /// The input value completions were computed from (staleness check)
    completed_from: String,
}

impl Default for PathCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl PathCompleter {
    pub fn new() -> Self {
        Self {
            completions: Vec::new(),
            cycle_index: None,
            completed_from: String::new(),
        }
    }

    /// Handle a Tab press: compute or cycle completions.
    ///
    /// Returns the new input value after completion.
    pub fn complete(&mut self, value: &str) -> String {
        let expanded = expand_tilde(value);

        // If completions are stale, recompute
        if self.completed_from != expanded {
            self.completions = list_matching_dirs(&expanded);
            self.cycle_index = None;
            self.completed_from = expanded.clone();
        }

        if self.completions.is_empty() {
            return value.to_string();
        }

        if self.completions.len() == 1 {
            // Single match — complete it with a trailing slash
            let result = &self.completions[0];
            let full = format!("{}/", result);
            self.completed_from = full.clone();
            return maybe_unexpand_tilde(value, &full);
        }

        // Multiple matches
        match self.cycle_index {
            None => {
                // First Tab: complete to longest common prefix
                let lcp = longest_common_prefix(&self.completions);
                if lcp != expanded {
                    // We made progress — show the common prefix
                    self.completed_from = lcp.clone();
                    maybe_unexpand_tilde(value, &lcp)
                } else {
                    // Already at the common prefix — start cycling
                    self.cycle_index = Some(0);
                    let result = format!("{}/", &self.completions[0]);
                    self.completed_from = result.clone();
                    maybe_unexpand_tilde(value, &result)
                }
            }
            Some(idx) => {
                // Subsequent Tabs: cycle to next match
                let next = (idx + 1) % self.completions.len();
                self.cycle_index = Some(next);
                let result = format!("{}/", &self.completions[next]);
                self.completed_from = result.clone();
                maybe_unexpand_tilde(value, &result)
            }
        }
    }

    /// Returns the current completion candidates and the highlighted index (if cycling).
    pub fn visible_completions(&self) -> (&[String], Option<usize>) {
        (&self.completions, self.cycle_index)
    }

    /// Reset completion state. Call this on any character input or backspace.
    pub fn invalidate(&mut self) {
        self.completions.clear();
        self.cycle_index = None;
        self.completed_from.clear();
    }
}

/// Expand a leading `~` or `~/` to the user's home directory.
pub(super) fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = home_dir() {
            let rest = &path[1..]; // "" or "/..."
            return format!("{}{}", home.display(), rest);
        }
    }
    path.to_string()
}

/// If the original input used `~`, re-collapse the home prefix.
fn maybe_unexpand_tilde(original: &str, expanded: &str) -> String {
    if original.starts_with('~') {
        if let Some(home) = home_dir() {
            let home_str = home.display().to_string();
            if let Some(rest) = expanded.strip_prefix(&home_str) {
                return format!("~{}", rest);
            }
        }
    }
    expanded.to_string()
}

/// List directories inside `parent` whose names start with `partial`.
///
/// `value` is the full expanded path typed so far. We split it into
/// `(parent_dir, partial_name)` at the last `/`.
fn list_matching_dirs(value: &str) -> Vec<String> {
    let (parent, partial) = split_path(value);

    let parent_path = if parent.is_empty() {
        Path::new(".")
    } else {
        Path::new(&parent)
    };

    let Ok(entries) = std::fs::read_dir(parent_path) else {
        return Vec::new();
    };

    let mut matches: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type()
                .map(|ft| ft.is_dir() || ft.is_symlink())
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(partial) {
                // For symlinks, verify the target is a directory
                if e.file_type().map(|ft| ft.is_symlink()).unwrap_or(false) && !e.path().is_dir() {
                    return None;
                }
                let full = if parent.is_empty() {
                    name
                } else if parent.ends_with('/') {
                    format!("{}{}", parent, name)
                } else {
                    format!("{}/{}", parent, name)
                };
                Some(full)
            } else {
                None
            }
        })
        .collect();

    matches.sort();
    matches
}

/// Split a path into (parent_dir, partial_name) at the last `/`.
///
/// Examples:
/// - `/home/user/pro` → (`/home/user`, `pro`)
/// - `/home/user/` → (`/home/user/`, ``)
/// - `pro` → (``, `pro`)
fn split_path(value: &str) -> (&str, &str) {
    match value.rfind('/') {
        Some(pos) => (&value[..=pos], &value[pos + 1..]),
        None => ("", value),
    }
}

/// Longest common prefix of a set of strings.
fn longest_common_prefix(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }

    let first = &strings[0];
    let mut len = first.len();

    for s in &strings[1..] {
        len = len.min(s.len());
        for (i, (a, b)) in first.bytes().zip(s.bytes()).enumerate() {
            if a != b {
                len = len.min(i);
                break;
            }
        }
    }

    first[..len].to_string()
}

/// Get the user's home directory.
fn home_dir() -> Option<PathBuf> {
    // Use directories crate (already a dependency)
    directories::BaseDirs::new().map(|bd| bd.home_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temp dir with the given subdirectories.
    fn setup_dirs(names: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for name in names {
            fs::create_dir_all(tmp.path().join(name)).unwrap();
        }
        tmp
    }

    #[test]
    fn single_match_completes_with_trailing_slash() {
        let tmp = setup_dirs(&["projects"]);
        let mut c = PathCompleter::new();

        let input = format!("{}/pro", tmp.path().display());
        let result = c.complete(&input);
        assert_eq!(result, format!("{}/projects/", tmp.path().display()));
    }

    #[test]
    fn no_match_returns_unchanged() {
        let tmp = setup_dirs(&["alpha"]);
        let mut c = PathCompleter::new();

        let input = format!("{}/zzz", tmp.path().display());
        let result = c.complete(&input);
        assert_eq!(result, input);
    }

    #[test]
    fn multiple_matches_completes_to_common_prefix() {
        let tmp = setup_dirs(&["project-a", "project-b"]);
        let mut c = PathCompleter::new();

        let input = format!("{}/pro", tmp.path().display());
        let result = c.complete(&input);
        assert_eq!(result, format!("{}/project-", tmp.path().display()));
    }

    #[test]
    fn cycling_through_matches() {
        let tmp = setup_dirs(&["project-a", "project-b"]);
        let mut c = PathCompleter::new();

        let input = format!("{}/project-", tmp.path().display());

        // First Tab: already at common prefix, starts cycling (index 0)
        let r1 = c.complete(&input);
        assert_eq!(r1, format!("{}/project-a/", tmp.path().display()));

        // Second Tab: cycle to next
        let r2 = c.complete(&r1);
        assert_eq!(r2, format!("{}/project-b/", tmp.path().display()));

        // Third Tab: wrap around
        let r3 = c.complete(&r2);
        assert_eq!(r3, format!("{}/project-a/", tmp.path().display()));
    }

    #[test]
    fn invalidation_resets_state() {
        let tmp = setup_dirs(&["alpha", "beta"]);
        let mut c = PathCompleter::new();

        let input = format!("{}/", tmp.path().display());
        c.complete(&input);
        assert!(!c.completions.is_empty());

        c.invalidate();
        assert!(c.completions.is_empty());
        assert!(c.cycle_index.is_none());
    }

    #[test]
    fn trailing_slash_lists_children() {
        let tmp = setup_dirs(&["aaa", "bbb"]);
        let mut c = PathCompleter::new();

        let input = format!("{}/", tmp.path().display());
        let result = c.complete(&input);
        // Should complete to common prefix of aaa and bbb — which is just the parent dir + /
        // Actually the LCP of "<tmp>/aaa" and "<tmp>/bbb" is "<tmp>/" since they diverge at the name
        // So the first Tab does nothing new (already at LCP), starts cycling
        assert!(
            result.ends_with("aaa/") || result.ends_with("bbb/"),
            "Expected cycling to start, got: {}",
            result
        );
    }

    #[test]
    fn nonexistent_parent_returns_unchanged() {
        let mut c = PathCompleter::new();
        let input = "/nonexistent_surely_xyz_123/foo";
        let result = c.complete(input);
        assert_eq!(result, input);
    }

    #[test]
    fn hidden_dirs_are_included() {
        let tmp = setup_dirs(&[".hidden", "visible"]);
        let mut c = PathCompleter::new();

        let input = format!("{}/.h", tmp.path().display());
        let result = c.complete(&input);
        assert_eq!(result, format!("{}/.hidden/", tmp.path().display()));
    }

    #[test]
    fn tilde_expansion() {
        // We can only test the expand/unexpand helpers since home dir varies
        let home = home_dir().unwrap();

        assert_eq!(expand_tilde("~"), home.display().to_string());
        assert_eq!(expand_tilde("~/foo"), format!("{}/foo", home.display()));
        assert_eq!(expand_tilde("/absolute"), "/absolute");
        assert_eq!(expand_tilde("relative"), "relative");
    }

    #[test]
    fn tilde_is_preserved_in_output() {
        let home = home_dir().unwrap();
        let expanded = format!("{}/Documents", home.display());
        let result = maybe_unexpand_tilde("~/Doc", &expanded);
        assert_eq!(result, "~/Documents");
    }

    #[test]
    fn longest_common_prefix_works() {
        assert_eq!(
            longest_common_prefix(&["abc".into(), "abd".into(), "abx".into()]),
            "ab"
        );
        assert_eq!(longest_common_prefix(&["hello".into()]), "hello");
        assert_eq!(longest_common_prefix(&["a".into(), "b".into()]), "");
    }

    #[test]
    fn files_are_excluded() {
        let tmp = setup_dirs(&["dir_a"]);
        // Create a regular file
        fs::write(tmp.path().join("file_a"), "content").unwrap();

        let mut c = PathCompleter::new();
        let input = format!("{}/", tmp.path().display());
        let result = c.complete(&input);
        // Only dir_a should match, not file_a
        assert_eq!(result, format!("{}/dir_a/", tmp.path().display()));
    }
}
