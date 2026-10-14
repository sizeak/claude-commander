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
//! OpenCode and omp all read `.agents/skills/`. A recognised agent gets exactly
//! one of the two — omp and OpenCode read *both*, so writing both would show
//! them every skill twice — and our copies are removed from the other, so
//! switching a session's program leaves no stale duplicates behind. An
//! unrecognised program (possibly Claude behind a wrapper) gets both.
//!
//! Each skill directory carries an [`OWNER_MARKER`] `.gitignore`, which ignores
//! the directory *and itself*: the skills never show in `git status`, the
//! review diff, or an agent's `git add -A`, and no file of the user's (the
//! repo's `.gitignore`, `.git/info/exclude`) is touched. The marker is also how
//! a directory is recognised as ours: only a [`SKILL_PREFIX`]`*` directory
//! carrying it (or one we create) is ever written, pruned or removed, so a
//! user's or repository's own `claude-commander-*` skill is never touched.
//! Symlinks are never followed — not a symlinked skill directory, and not a
//! skills root that resolves outside the working directory (e.g.
//! `.claude -> ~/.claude`).

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
/// The `.gitignore` in each skill directory we write: ignores everything beside
/// it, itself included, and marks the directory as ours to manage.
const OWNER_MARKER: &[u8] =
    b"# Written by Claude Commander on every agent launch; edits are overwritten.\n*\n";

/// Disambiguates temp files when two launches install into one directory at
/// once (e.g. the TUI and a server sharing a commander directory).
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The skills roots to write for `kind`, and the roots to clear our copies
/// from.
fn roots(kind: AgentKind) -> (&'static [&'static str], &'static [&'static str]) {
    match kind {
        AgentKind::Claude => (&[CLAUDE_SKILLS_DIR], &[AGENTS_SKILLS_DIR]),
        AgentKind::Codex | AgentKind::OpenCode | AgentKind::Omp => {
            (&[AGENTS_SKILLS_DIR], &[CLAUDE_SKILLS_DIR])
        }
        AgentKind::Unknown => (&[CLAUDE_SKILLS_DIR, AGENTS_SKILLS_DIR], &[]),
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
    let dir_canon = dir.canonicalize()?;
    let (write, sweep) = roots(kind);
    let skills = bundled_skills();
    let mut written = 0;
    let mut written_roots = Vec::new();
    for root in write {
        let root = dir.join(root);
        if !resolves_inside(&root, &dir_canon)? {
            warn!(
                "not installing agent skills into {}: it leads outside {}",
                root.display(),
                dir.display()
            );
            continue;
        }
        std::fs::create_dir_all(&root)?;
        for (skill, files) in &skills {
            written += install_skill(&root.join(skill), files)?;
        }
        // A skill a newer build dropped or renamed.
        remove_owned_skill_dirs(&root, |name| !skills.contains_key(name))?;
        written_roots.push(root.canonicalize()?);
    }
    for root in sweep {
        let root = dir.join(root);
        let Ok(canon) = root.canonicalize() else {
            continue;
        };
        // A root symlinked onto one we just wrote is the same directory.
        if canon.starts_with(&dir_canon) && !written_roots.contains(&canon) {
            remove_owned_skill_dirs(&root, |_| true)?;
        }
    }
    Ok(written)
}

/// Whether `path` — which may not exist yet — stays inside `dir_canon` once
/// symlinks are resolved. Its deepest existing ancestor decides: anything
/// below that is created fresh, as a real directory.
fn resolves_inside(path: &Path, dir_canon: &Path) -> io::Result<bool> {
    for ancestor in path.ancestors() {
        match ancestor.canonicalize() {
            Ok(canon) => return Ok(canon.starts_with(dir_canon)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

/// Write one skill into `skill_dir` if the directory is ours (or absent),
/// returning how many files were written. A directory we didn't write — the
/// user's or the repository's own skill of the same name, or a symlink — is
/// left exactly as it is.
fn install_skill(skill_dir: &Path, files: &[BundledFile]) -> io::Result<usize> {
    match std::fs::symlink_metadata(skill_dir) {
        Ok(meta) if !meta.is_dir() || !is_owned(skill_dir)? => return Ok(0),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => std::fs::create_dir(skill_dir)?,
        Err(e) => return Err(e),
    }
    // The marker first: it is what makes a half-written directory ours.
    let mut written = usize::from(write_if_changed(
        &skill_dir.join(".gitignore"),
        OWNER_MARKER,
    )?);
    for file in files {
        written += usize::from(write_if_changed(&skill_dir.join(&file.rel), &file.data)?);
    }
    let keep: Vec<&str> = files.iter().map(|file| file.rel.as_str()).collect();
    remove_unlisted_files(skill_dir, skill_dir, &keep)?;
    Ok(written)
}

/// Whether we wrote `skill_dir`: it carries our marker, or it's empty (we
/// created it and were interrupted before the marker landed).
fn is_owned(skill_dir: &Path) -> io::Result<bool> {
    if std::fs::read(skill_dir.join(".gitignore")).is_ok_and(|m| m == OWNER_MARKER) {
        return Ok(true);
    }
    Ok(std::fs::read_dir(skill_dir)?.next().is_none())
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
/// relative to `root`, `/`-separated) and aren't the marker. Another
/// installer's in-flight temp file is left for it to rename, and a file that
/// vanishes first (that installer finished) is fine.
fn remove_unlisted_files(root: &Path, dir: &Path, keep: &[&str]) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            remove_unlisted_files(root, &path, keep)?;
            continue;
        }
        let rel = relative_slash_path(root, &path);
        let name = entry.file_name().to_string_lossy().into_owned();
        let in_flight = name.starts_with('.') && name.contains(".tmp.");
        if rel != ".gitignore" && !in_flight && !keep.contains(&rel.as_str()) {
            ignore_not_found(std::fs::remove_file(&path))?;
        }
    }
    Ok(())
}

fn ignore_not_found(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn relative_slash_path(root: &Path, path: &Path) -> String {
    let rel: PathBuf = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Remove the directories directly under `root` that are ours — prefixed,
/// real (not symlinked) and marked — and whose name satisfies `remove`.
/// Anything else there is the user's and left alone; a missing `root` is fine.
fn remove_owned_skill_dirs(root: &Path, remove: impl Fn(&str) -> bool) -> io::Result<()> {
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
        if entry.file_type()?.is_dir()
            && name.starts_with(SKILL_PREFIX)
            && remove(&name)
            && is_owned(&entry.path())?
        {
            ignore_not_found(std::fs::remove_dir_all(entry.path()))?;
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
                OWNER_MARKER
            );
        }
    }

    #[test]
    fn other_agents_get_the_skills_under_dot_agents() {
        for kind in [AgentKind::Codex, AgentKind::OpenCode, AgentKind::Omp] {
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

    fn write_skill(dir: &Path, root: &str, name: &str, body: &str) -> PathBuf {
        let skill = dir.join(root).join(name);
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), body).unwrap();
        skill
    }

    #[test]
    fn unknown_programs_get_both_roots() {
        // An unrecognised program might be Claude behind a wrapper
        // (`env X=1 claude`, `npx …`): give it both, and sweep neither.
        let dir = TempDir::new().unwrap();
        install(dir.path(), AgentKind::Unknown).unwrap();
        assert_eq!(installed(dir.path(), CLAUDE_SKILLS_DIR), skill_names());
        assert_eq!(installed(dir.path(), AGENTS_SKILLS_DIR), skill_names());
    }

    #[test]
    fn a_prefixed_dir_we_did_not_write_is_never_touched() {
        let dir = TempDir::new().unwrap();
        // The user's own skill that happens to share our prefix, in both roots,
        // plus one that collides with a bundled skill's exact name.
        let mine = write_skill(
            dir.path(),
            CLAUDE_SKILLS_DIR,
            "claude-commander-mytool",
            "mine",
        );
        let theirs = write_skill(
            dir.path(),
            AGENTS_SKILLS_DIR,
            "claude-commander-x",
            "theirs",
        );
        let clash = write_skill(
            dir.path(),
            CLAUDE_SKILLS_DIR,
            "claude-commander-new-session",
            "repo's own",
        );
        std::fs::write(clash.join("notes.md"), "extra").unwrap();

        install(dir.path(), AgentKind::Claude).unwrap();
        install(dir.path(), AgentKind::Codex).unwrap();
        install(dir.path(), AgentKind::Claude).unwrap();

        assert_eq!(
            std::fs::read_to_string(mine.join("SKILL.md")).unwrap(),
            "mine"
        );
        assert_eq!(
            std::fs::read_to_string(theirs.join("SKILL.md")).unwrap(),
            "theirs"
        );
        assert_eq!(
            std::fs::read_to_string(clash.join("SKILL.md")).unwrap(),
            "repo's own"
        );
        assert!(clash.join("notes.md").exists());
        assert!(!clash.join(".gitignore").exists());
    }

    #[cfg(unix)]
    #[test]
    fn roots_symlinked_together_keep_the_skills() {
        // `.agents/skills -> ../.claude/skills` shares one directory between
        // agents; sweeping "the other root" must not delete what we just wrote.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(CLAUDE_SKILLS_DIR)).unwrap();
        std::fs::create_dir_all(dir.path().join(".agents")).unwrap();
        std::os::unix::fs::symlink("../.claude/skills", dir.path().join(AGENTS_SKILLS_DIR))
            .unwrap();

        install(dir.path(), AgentKind::Claude).unwrap();
        assert_eq!(installed(dir.path(), CLAUDE_SKILLS_DIR), skill_names());
        install(dir.path(), AgentKind::Codex).unwrap();
        assert_eq!(installed(dir.path(), AGENTS_SKILLS_DIR), skill_names());
    }

    #[cfg(unix)]
    #[test]
    fn a_root_that_leads_outside_the_directory_is_left_alone() {
        // `.claude -> ~/.claude` would otherwise put our skills in the user's
        // global config, where every Claude session would see a stale copy.
        let dir = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join(".claude")).unwrap();
        let outside_mine = write_skill(elsewhere.path(), "skills", "claude-commander-old", "x");
        std::fs::write(outside_mine.join(".gitignore"), OWNER_MARKER).unwrap();

        install(dir.path(), AgentKind::Claude).unwrap();
        install(dir.path(), AgentKind::Codex).unwrap();

        assert!(
            installed(elsewhere.path(), "skills").len() == 1,
            "nothing written outside"
        );
        assert!(outside_mine.exists(), "nothing removed outside");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_skill_dir_is_not_followed() {
        let dir = TempDir::new().unwrap();
        let dotfiles = TempDir::new().unwrap();
        std::fs::write(dotfiles.path().join("precious.md"), "keep me").unwrap();
        std::fs::create_dir_all(dir.path().join(CLAUDE_SKILLS_DIR)).unwrap();
        std::os::unix::fs::symlink(
            dotfiles.path(),
            dir.path()
                .join(CLAUDE_SKILLS_DIR)
                .join("claude-commander-new-session"),
        )
        .unwrap();

        install(dir.path(), AgentKind::Claude).unwrap();

        assert_eq!(
            std::fs::read_to_string(dotfiles.path().join("precious.md")).unwrap(),
            "keep me"
        );
        assert!(!dotfiles.path().join("SKILL.md").exists());
    }

    #[test]
    fn another_installers_temp_file_is_not_pruned() {
        let dir = TempDir::new().unwrap();
        install(dir.path(), AgentKind::Claude).unwrap();
        let skill = dir
            .path()
            .join(CLAUDE_SKILLS_DIR)
            .join("claude-commander-new-session");
        let in_flight = skill.join(".SKILL.md.tmp.99999.0");
        std::fs::write(&in_flight, "half written by another process").unwrap();
        std::fs::write(skill.join("SKILL.md"), "stale").unwrap();

        install(dir.path(), AgentKind::Claude).unwrap();
        assert!(in_flight.exists());
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
        // Written by an older build, so it carries our marker.
        std::fs::write(retired.join(".gitignore"), OWNER_MARKER).unwrap();

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
