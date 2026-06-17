//! Locating and tailing the commander's Claude Code transcript.
//!
//! Claude Code writes each session's conversation to
//! `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`, where the cwd is
//! encoded by replacing every non-alphanumeric character with `-`. Each line is
//! a JSON object; assistant turns have `"type":"assistant"` and a `message`
//! whose `content` is an array of blocks (`thinking` / `text` / `tool_use`). We
//! surface only the `text` blocks of newly-appended assistant turns.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Encode an absolute path into Claude Code's project-dir segment: every
/// non-alphanumeric char becomes `-`, char-by-char (no collapsing).
pub fn encode_project_dir(abs_cwd: &Path) -> String {
    abs_cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// `<home>/.claude/projects/<encoded-cwd>`.
pub fn transcript_dir(home: &Path, abs_cwd: &Path) -> PathBuf {
    home.join(".claude")
        .join("projects")
        .join(encode_project_dir(abs_cwd))
}

/// The most-recently-modified `*.jsonl` in `dir` (the active session), or
/// `None` if the directory is missing or empty.
pub fn latest_transcript(dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(best, _)| mtime > *best) {
            newest = Some((mtime, path));
        }
    }
    newest.map(|(_, p)| p)
}

/// One parsed assistant turn (only its natural-language text blocks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantTurn {
    pub uuid: String,
    pub text_blocks: Vec<String>,
}

#[derive(Deserialize, Default)]
struct RawLine {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    message: Option<RawMessage>,
}

#[derive(Deserialize, Default)]
struct RawMessage {
    #[serde(default)]
    content: Vec<RawBlock>,
}

#[derive(Deserialize, Default)]
struct RawBlock {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

/// Parse a single JSONL line into an [`AssistantTurn`], or `None` if it isn't an
/// assistant turn with at least one non-empty text block. Tolerant of unknown
/// fields and malformed/partial lines (returns `None` rather than erroring).
pub fn parse_line(line: &str) -> Option<AssistantTurn> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let raw: RawLine = serde_json::from_str(line).ok()?;
    if raw.kind != "assistant" {
        return None;
    }
    let text_blocks: Vec<String> = raw
        .message?
        .content
        .into_iter()
        .filter(|b| b.kind == "text")
        .filter_map(|b| b.text)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if text_blocks.is_empty() {
        return None;
    }
    // Real transcripts always carry a uuid; fall back to a content hash so dedup
    // still works if one is ever missing.
    let uuid = if raw.uuid.is_empty() {
        format!(
            "h{:016x}",
            xxhash_rust::xxh3::xxh3_64(text_blocks.concat().as_bytes())
        )
    } else {
        raw.uuid
    };
    Some(AssistantTurn { uuid, text_blocks })
}

fn last_assistant_uuid(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    content
        .lines()
        .filter_map(parse_line)
        .next_back()
        .map(|t| t.uuid)
}

/// Stateful tail reader over the commander's transcript directory. Tracks the
/// active file, a byte offset, and the last-emitted uuid so each assistant turn
/// is surfaced exactly once. Follows session rotation (a new `*.jsonl` from
/// `/clear`) and only ever returns the *latest* new turn.
#[derive(Debug, Default)]
pub struct TranscriptTail {
    path: Option<PathBuf>,
    offset: u64,
    last_uuid: Option<String>,
}

impl TranscriptTail {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed to the end of the current transcript so only turns appended *after*
    /// this call are surfaced — avoids replaying the existing backlog when
    /// conversation mode is first enabled.
    pub fn seed_to_end(&mut self, dir: &Path) {
        if let Some(path) = latest_transcript(dir) {
            self.offset = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            self.last_uuid = last_assistant_uuid(&path);
            self.path = Some(path);
        }
    }

    /// Read newly-appended complete lines and return the latest new assistant
    /// turn, or `None` if there's nothing new. Defers a trailing partial line
    /// (no newline yet) until it completes.
    pub fn poll(&mut self, dir: &Path) -> Option<AssistantTurn> {
        let latest = latest_transcript(dir)?;
        // New session file → start from its beginning.
        if self.path.as_deref() != Some(latest.as_path()) {
            self.path = Some(latest.clone());
            self.offset = 0;
            self.last_uuid = None;
        }
        let path = self.path.clone()?;

        let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            // File was truncated/rewritten in place.
            self.offset = 0;
        }
        if len <= self.offset {
            return None;
        }

        let mut file = fs::File::open(&path).ok()?;
        file.seek(SeekFrom::Start(self.offset)).ok()?;
        let mut buf = String::new();
        file.read_to_string(&mut buf).ok()?;

        // Only consume through the last newline; keep the trailing fragment.
        let nl = buf.rfind('\n')?;
        let complete = &buf[..=nl];
        self.offset += complete.len() as u64;

        // Among the new complete lines, take the most recent assistant turn.
        let latest_turn = complete.lines().filter_map(parse_line).next_back()?;
        if self.last_uuid.as_deref() == Some(latest_turn.uuid.as_str()) {
            return None;
        }
        self.last_uuid = Some(latest_turn.uuid.clone());
        Some(latest_turn)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn encode_matches_claude_code_scheme() {
        // The empirically-verified commander case.
        let p = Path::new("/home/si/.local/share/claude-commander/commander");
        assert_eq!(
            encode_project_dir(p),
            "-home-si--local-share-claude-commander-commander"
        );
    }

    #[test]
    fn encode_each_separator_independently() {
        assert_eq!(encode_project_dir(Path::new("/a.b-c")), "-a-b-c");
        assert_eq!(encode_project_dir(Path::new("/a//b")), "-a--b");
    }

    #[test]
    fn transcript_dir_layout() {
        let dir = transcript_dir(Path::new("/home/u"), Path::new("/x/y"));
        assert_eq!(dir, PathBuf::from("/home/u/.claude/projects/-x-y"));
    }

    fn write(dir: &Path, name: &str, body: &str, mtime: SystemTime) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        let f = fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(mtime).unwrap();
        path
    }

    fn assistant_line(uuid: &str, text: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","message":{{"role":"assistant","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    #[test]
    fn latest_transcript_picks_newest_mtime() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        write(tmp.path(), "old.jsonl", "{}\n", base);
        let newer = write(
            tmp.path(),
            "new.jsonl",
            "{}\n",
            base + Duration::from_secs(60),
        );
        write(
            tmp.path(),
            "ignore.txt",
            "x",
            base + Duration::from_secs(120),
        );
        assert_eq!(latest_transcript(tmp.path()), Some(newer));
    }

    #[test]
    fn latest_transcript_none_when_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(latest_transcript(tmp.path()), None);
    }

    #[test]
    fn parse_assistant_with_text() {
        let line = assistant_line("u1", "Hello there.");
        let turn = parse_line(&line).unwrap();
        assert_eq!(turn.uuid, "u1");
        assert_eq!(turn.text_blocks, vec!["Hello there."]);
    }

    #[test]
    fn parse_skips_thinking_and_tool_use_blocks() {
        let line = r#"{"type":"assistant","uuid":"u2","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Visible."},{"type":"tool_use","name":"x"}]}}"#;
        let turn = parse_line(line).unwrap();
        assert_eq!(turn.text_blocks, vec!["Visible."]);
    }

    #[test]
    fn parse_thinking_only_is_none() {
        let line = r#"{"type":"assistant","uuid":"u3","message":{"content":[{"type":"thinking","thinking":"just thinking"}]}}"#;
        assert_eq!(parse_line(line), None);
    }

    #[test]
    fn parse_non_assistant_and_garbage_is_none() {
        assert_eq!(parse_line(r#"{"type":"user","message":{}}"#), None);
        assert_eq!(parse_line(r#"{"type":"summary"}"#), None);
        assert_eq!(parse_line("not json at all"), None);
        assert_eq!(parse_line(""), None);
    }

    #[test]
    fn parse_tolerates_unknown_fields() {
        let line = r#"{"type":"assistant","uuid":"u4","cwd":"/x","extra":42,"message":{"role":"assistant","model":"claude","content":[{"type":"text","text":"Hi.","sig":"z"}]}}"#;
        let turn = parse_line(line).unwrap();
        assert_eq!(turn.text_blocks, vec!["Hi."]);
    }

    #[test]
    fn tail_emits_latest_then_dedups() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);
        let body = format!(
            "{}\n{}\n",
            assistant_line("a", "First."),
            assistant_line("b", "Second.")
        );
        write(tmp.path(), "s.jsonl", &body, base);

        let mut tail = TranscriptTail::new();
        // First poll returns the latest of the two existing turns.
        let turn = tail.poll(tmp.path()).unwrap();
        assert_eq!(turn.uuid, "b");
        // Nothing new → None.
        assert_eq!(tail.poll(tmp.path()), None);
    }

    #[test]
    fn tail_returns_new_appended_turn() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(3_000_000);
        let path = write(
            tmp.path(),
            "s.jsonl",
            &format!("{}\n", assistant_line("a", "One.")),
            base,
        );

        let mut tail = TranscriptTail::new();
        assert_eq!(tail.poll(tmp.path()).unwrap().uuid, "a");

        // Append a new complete line.
        let mut body = fs::read_to_string(&path).unwrap();
        body.push_str(&format!("{}\n", assistant_line("c", "Three.")));
        write(tmp.path(), "s.jsonl", &body, base + Duration::from_secs(1));
        assert_eq!(tail.poll(tmp.path()).unwrap().uuid, "c");
    }

    #[test]
    fn tail_defers_partial_line() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(4_000_000);
        write(
            tmp.path(),
            "s.jsonl",
            &format!("{}\n", assistant_line("a", "One.")),
            base,
        );
        let mut tail = TranscriptTail::new();
        let _ = tail.poll(tmp.path());

        // Append a line WITHOUT a trailing newline → deferred.
        let partial = format!(
            "{}\n{}",
            assistant_line("a", "One."),
            assistant_line("d", "Partial.")
        );
        write(
            tmp.path(),
            "s.jsonl",
            &partial,
            base + Duration::from_secs(1),
        );
        assert_eq!(tail.poll(tmp.path()), None);

        // Complete the line → now emitted.
        let completed = format!("{partial}\n");
        write(
            tmp.path(),
            "s.jsonl",
            &completed,
            base + Duration::from_secs(2),
        );
        assert_eq!(tail.poll(tmp.path()).unwrap().uuid, "d");
    }

    #[test]
    fn tail_follows_rotation_to_new_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(5_000_000);
        write(
            tmp.path(),
            "old.jsonl",
            &format!("{}\n", assistant_line("a", "Old.")),
            base,
        );
        let mut tail = TranscriptTail::new();
        assert_eq!(tail.poll(tmp.path()).unwrap().uuid, "a");

        // A new session file (newer mtime) → tail switches and reads from start.
        write(
            tmp.path(),
            "new.jsonl",
            &format!("{}\n", assistant_line("z", "New session.")),
            base + Duration::from_secs(10),
        );
        assert_eq!(tail.poll(tmp.path()).unwrap().uuid, "z");
    }

    #[test]
    fn seed_to_end_skips_backlog() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(6_000_000);
        let path = write(
            tmp.path(),
            "s.jsonl",
            &format!("{}\n", assistant_line("a", "Backlog.")),
            base,
        );

        let mut tail = TranscriptTail::new();
        tail.seed_to_end(tmp.path());
        // Existing backlog is not replayed.
        assert_eq!(tail.poll(tmp.path()), None);

        // Only a newly-appended turn is surfaced.
        let body = format!(
            "{}{}\n",
            fs::read_to_string(&path).unwrap(),
            assistant_line("e", "Fresh.")
        );
        write(tmp.path(), "s.jsonl", &body, base + Duration::from_secs(1));
        assert_eq!(tail.poll(tmp.path()).unwrap().uuid, "e");
    }
}
