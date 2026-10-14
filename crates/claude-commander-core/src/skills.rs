//! Agent skills bundled with Commander.
//!
//! The skills in `crates/claude-commander-core/skills/` teach an agent how to
//! drive Commander: handwritten workflow guidance (spawning, monitoring,
//! deleting sessions) plus `claude-commander-cli-reference`, generated from the
//! binary's clap tree and kept fresh by `claude-commander`'s `skills_gen` drift
//! test. They are compiled into the binary and written into an agent's working
//! directory each time Commander launches it, so they are always available and
//! always match the running version, with nothing to install.
//!
//! Every agent Commander drives discovers project skills from its working
//! directory by convention: Claude Code reads `.claude/skills/`, and Codex,
//! OpenCode and omp all read `.agents/skills/`. Exactly one of the two is
//! written per launch — omp and OpenCode read *both*, so writing both would
//! show them every skill twice — and our copies are removed from the other, so
//! switching a session's program leaves no stale duplicates behind.
//!
//! Each skill directory carries a `.gitignore` of `*`, which ignores the
//! directory *and itself*: the skills never show in `git status`, the review
//! diff, or an agent's `git add -A`, and no file of the user's (the repo's
//! `.gitignore`, `.git/info/exclude`) is touched. Only directories named
//! [`SKILL_PREFIX`]`*` are ever written or removed.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rust_embed::RustEmbed;
use tracing::warn;

use crate::agent::AgentKind;

#[derive(RustEmbed)]
#[folder = "skills/"]
struct Bundled;

/// Every bundled skill's directory (and frontmatter `name`) starts with this.
/// Project skills are not namespaced, so the prefix keeps ours from colliding
/// with a repository's own — and marks which directories are ours to manage.
pub const SKILL_PREFIX: &str = "claude-commander-";

/// Where Claude Code discovers project skills.
const CLAUDE_SKILLS_DIR: &str = ".claude/skills";
/// Where Codex, OpenCode and omp discover project skills.
const AGENTS_SKILLS_DIR: &str = ".agents/skills";
/// A `.gitignore` that ignores everything beside it, itself included.
const SELF_IGNORE: &[u8] = b"*\n";

/// Disambiguates temp files when two launches install into one directory at
/// once (e.g. the TUI and a server sharing a commander directory).
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The skills root `kind` reads, and the one it doesn't.
fn roots(kind: AgentKind) -> (&'static str, &'static str) {
    if kind.is_claude() {
        (CLAUDE_SKILLS_DIR, AGENTS_SKILLS_DIR)
    } else {
        (AGENTS_SKILLS_DIR, CLAUDE_SKILLS_DIR)
    }
}

/// One bundled file: its `/`-separated path within the skill directory, and its
/// contents.
struct BundledFile {
    rel: String,
    data: Cow<'static, [u8]>,
}

/// The bundled files, grouped by skill directory name.
fn bundled_skills() -> BTreeMap<String, Vec<BundledFile>> {
    let mut skills: BTreeMap<String, Vec<BundledFile>> = BTreeMap::new();
    for path in Bundled::iter() {
        let Some((skill, rel)) = path.split_once('/') else {
            continue;
        };
        let Some(file) = Bundled::get(&path) else {
            continue;
        };
        skills
            .entry(skill.to_string())
            .or_default()
            .push(BundledFile {
                rel: rel.to_string(),
                data: file.data,
            });
    }
    skills
}

/// Write the bundled skills into `dir` for an agent of `kind`, returning how
/// many files were (re)written — zero when everything was already current.
pub fn install(dir: &Path, kind: AgentKind) -> io::Result<usize> {
    let (target, other) = roots(kind);
    let target = dir.join(target);
    let skills = bundled_skills();
    let mut written = 0;
    for (skill, files) in &skills {
        let skill_dir = target.join(skill);
        std::fs::create_dir_all(&skill_dir)?;
        written += usize::from(write_if_changed(
            &skill_dir.join(".gitignore"),
            SELF_IGNORE,
        )?);
        for file in files {
            written += usize::from(write_if_changed(&skill_dir.join(&file.rel), &file.data)?);
        }
        let keep: Vec<&str> = files.iter().map(|file| file.rel.as_str()).collect();
        remove_unlisted_files(&skill_dir, &skill_dir, &keep)?;
    }
    // A skill a newer build dropped or renamed, then every copy in the root
    // this agent doesn't read.
    remove_our_skill_dirs(&target, |name| !skills.contains_key(name))?;
    remove_our_skill_dirs(&dir.join(other), |_| true)?;
    Ok(written)
}

/// [`install`] for a launch path: runs off the async executor, and a failure
/// is logged rather than returned — missing skills must never stop an agent
/// from starting.
pub async fn install_for_launch(dir: &Path, kind: AgentKind) {
    let dir = dir.to_path_buf();
    let result = tokio::task::spawn_blocking({
        let dir = dir.clone();
        move || install(&dir, kind)
    })
    .await;
    match result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!("could not install agent skills into {}: {e}", dir.display()),
        Err(e) => warn!("agent skills install task failed: {e}"),
    }
}

/// Write `data` to `path` unless it already holds exactly that, via a sibling
/// temp file + rename so an agent starting mid-write never reads a torn file.
/// Returns whether it wrote.
fn write_if_changed(path: &Path, data: &[u8]) -> io::Result<bool> {
    if std::fs::read(path).is_ok_and(|current| current == data) {
        return Ok(false);
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{name}.tmp.{}.{seq}", std::process::id()));
    let result = std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|()| true)
}

/// Remove files under `dir` (recursively) that are not in `keep` (paths
/// relative to `root`, `/`-separated) and aren't the `.gitignore`.
fn remove_unlisted_files(root: &Path, dir: &Path, keep: &[&str]) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            remove_unlisted_files(root, &path, keep)?;
            continue;
        }
        let rel = relative_slash_path(root, &path);
        if rel != ".gitignore" && !keep.contains(&rel.as_str()) {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn relative_slash_path(root: &Path, path: &Path) -> String {
    let rel: PathBuf = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Remove the [`SKILL_PREFIX`] directories directly under `root` whose name
/// satisfies `remove`. Anything else there is the user's and left alone; a
/// missing `root` is fine.
fn remove_our_skill_dirs(root: &Path, remove: impl Fn(&str) -> bool) -> io::Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // `file_type` doesn't follow symlinks, so a link named like ours is
        // never traversed into.
        if entry.file_type()?.is_dir() && name.starts_with(SKILL_PREFIX) && remove(&name) {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn skill_names() -> Vec<String> {
        bundled_skills().into_keys().collect()
    }

    fn installed(dir: &Path, root: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir.join(root)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(SKILL_PREFIX))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn bundle_holds_the_prefixed_skills_each_naming_itself() {
        let skills = bundled_skills();
        for expected in [
            "claude-commander-cli-reference",
            "claude-commander-new-session",
            "claude-commander-monitor-sessions",
            "claude-commander-delete-session",
            "claude-commander-slack-notify",
        ] {
            assert!(skills.contains_key(expected), "missing {expected}");
        }
        for (skill, files) in &skills {
            assert!(skill.starts_with(SKILL_PREFIX), "{skill} lacks the prefix");
            let md = files
                .iter()
                .find(|file| file.rel == "SKILL.md")
                .unwrap_or_else(|| panic!("{skill} has no SKILL.md"));
            let md = std::str::from_utf8(&md.data).unwrap();
            // Agents match a skill by its frontmatter name, so it must agree
            // with the directory the prefix logic manages.
            assert!(
                md.starts_with(&format!("---\nname: {skill}\n")),
                "{skill}/SKILL.md frontmatter must start with `name: {skill}`"
            );
            assert!(
                md.contains("\ndescription: "),
                "{skill} needs a description"
            );
            for file in files {
                // The nix src-filter keeps only `.md` under crates/.
                assert!(
                    file.rel.ends_with(".md"),
                    "{skill}/{} would be dropped by the nix build",
                    file.rel
                );
            }
        }
    }

    #[test]
    fn claude_gets_the_skills_under_dot_claude() {
        let dir = TempDir::new().unwrap();
        install(dir.path(), AgentKind::Claude).unwrap();
        assert_eq!(installed(dir.path(), CLAUDE_SKILLS_DIR), skill_names());
        assert!(installed(dir.path(), AGENTS_SKILLS_DIR).is_empty());
        for skill in skill_names() {
            let skill_dir = dir.path().join(CLAUDE_SKILLS_DIR).join(&skill);
            assert!(skill_dir.join("SKILL.md").is_file());
            assert_eq!(
                std::fs::read(skill_dir.join(".gitignore")).unwrap(),
                SELF_IGNORE
            );
        }
    }

    #[test]
    fn other_agents_get_the_skills_under_dot_agents() {
        for kind in [
            AgentKind::Codex,
            AgentKind::OpenCode,
            AgentKind::Omp,
            AgentKind::Unknown,
        ] {
            let dir = TempDir::new().unwrap();
            install(dir.path(), kind).unwrap();
            assert_eq!(
                installed(dir.path(), AGENTS_SKILLS_DIR),
                skill_names(),
                "{kind:?}"
            );
            assert!(
                installed(dir.path(), CLAUDE_SKILLS_DIR).is_empty(),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn switching_agent_moves_our_skills_and_leaves_the_users_alone() {
        let dir = TempDir::new().unwrap();
        let users = dir.path().join(CLAUDE_SKILLS_DIR).join("my-own-skill");
        std::fs::create_dir_all(&users).unwrap();
        std::fs::write(users.join("SKILL.md"), "mine").unwrap();

        install(dir.path(), AgentKind::Claude).unwrap();
        install(dir.path(), AgentKind::Codex).unwrap();

        assert!(installed(dir.path(), CLAUDE_SKILLS_DIR).is_empty());
        assert_eq!(installed(dir.path(), AGENTS_SKILLS_DIR), skill_names());
        assert_eq!(
            std::fs::read_to_string(users.join("SKILL.md")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn reinstalling_unchanged_skills_writes_nothing() {
        let dir = TempDir::new().unwrap();
        assert!(install(dir.path(), AgentKind::Claude).unwrap() > 0);
        assert_eq!(install(dir.path(), AgentKind::Claude).unwrap(), 0);
    }

    #[test]
    fn stale_and_unknown_content_in_our_dirs_is_repaired() {
        let dir = TempDir::new().unwrap();
        install(dir.path(), AgentKind::Claude).unwrap();
        let root = dir.path().join(CLAUDE_SKILLS_DIR);
        let skill = root.join("claude-commander-new-session");
        std::fs::write(skill.join("SKILL.md"), "edited by an agent").unwrap();
        std::fs::write(skill.join("leftover.md"), "from an older build").unwrap();
        let retired = root.join("claude-commander-retired-skill");
        std::fs::create_dir_all(&retired).unwrap();
        std::fs::write(retired.join("SKILL.md"), "gone").unwrap();

        install(dir.path(), AgentKind::Claude).unwrap();

        let restored = std::fs::read_to_string(skill.join("SKILL.md")).unwrap();
        assert!(restored.starts_with("---\nname: claude-commander-new-session\n"));
        assert!(!skill.join("leftover.md").exists());
        assert!(!retired.exists());
        assert_eq!(installed(dir.path(), CLAUDE_SKILLS_DIR), skill_names());
    }

    #[test]
    fn installed_skills_are_invisible_to_git() {
        let dir = TempDir::new().unwrap();
        let git = |args: &[&str]| {
            let out = crate::git::fixture::fixture_git_std()
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8(out.stdout).unwrap()
        };
        git(&["init", "-q"]);

        install(dir.path(), AgentKind::Claude).unwrap();
        install(dir.path(), AgentKind::Codex).unwrap();
        install(dir.path(), AgentKind::Claude).unwrap();
        assert_eq!(installed(dir.path(), CLAUDE_SKILLS_DIR), skill_names());

        assert_eq!(git(&["status", "--porcelain", "--untracked-files=all"]), "");
        git(&["add", "-A"]);
        assert_eq!(git(&["diff", "--cached", "--name-only"]), "");
    }

    #[tokio::test]
    async fn launch_install_swallows_failure() {
        let dir = TempDir::new().unwrap();
        // A file where the skills root's parent should be: the install fails,
        // and the launch path must shrug it off rather than panic or error.
        std::fs::write(dir.path().join(".claude"), "not a directory").unwrap();
        install_for_launch(dir.path(), AgentKind::Claude).await;
        assert!(dir.path().join(".claude").is_file());
    }
}
