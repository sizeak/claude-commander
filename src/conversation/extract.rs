//! Pure text-extraction for conversation mode.
//!
//! Turns an assistant turn's raw markdown `text` blocks into spoken-ready prose
//! (per [`SpeakScope`]) and splits that prose into sentence-sized chunks so the
//! worker can synthesize + play incrementally for low latency. Everything here
//! is pure and unit-tested — no IO, no audio.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// How much of each assistant reply to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpeakScope {
    /// Strip code blocks and markdown; speak the natural-language prose only.
    #[default]
    ProseOnly,
    /// Prose, then keep only the final paragraph (a short summary).
    FinalSummary,
    /// Speak the joined text blocks unchanged.
    Verbatim,
}

impl SpeakScope {
    /// All variants, in display order (used by the settings picker).
    pub const ALL: [SpeakScope; 3] = [Self::ProseOnly, Self::FinalSummary, Self::Verbatim];

    /// snake_case config token (matches the serde representation).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProseOnly => "prose_only",
            Self::FinalSummary => "final_summary",
            Self::Verbatim => "verbatim",
        }
    }

    /// Parse from the snake_case config token.
    pub fn from_token(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// Parse from the human label (used by the settings option-picker).
    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.label() == s)
    }

    /// Human-friendly label for the settings UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::ProseOnly => "Prose only",
            Self::FinalSummary => "Final summary",
            Self::Verbatim => "Verbatim",
        }
    }
}

/// Upper bound on characters spoken from a single reply, so a very long reply
/// can't tie up synthesis indefinitely. Truncated on a sentence boundary.
const MAX_SPOKEN_CHARS: usize = 2000;

static IMAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\([^)]*\)").unwrap());
static LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
static INLINE_CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]*)`").unwrap());

/// Turn an assistant turn's text blocks into the text to speak, per `scope`.
/// Returns `None` if nothing speakable remains (e.g. the reply was pure code).
pub fn spoken_text(blocks: &[String], scope: SpeakScope) -> Option<String> {
    let joined = blocks
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if joined.trim().is_empty() {
        return None;
    }
    let text = match scope {
        SpeakScope::Verbatim => joined,
        SpeakScope::ProseOnly => prose_from_markdown(&joined),
        SpeakScope::FinalSummary => last_paragraph(&prose_from_markdown(&joined)),
    };
    let text = normalize_whitespace(&text);
    let capped = cap_length(text.trim(), MAX_SPOKEN_CHARS);
    let capped = capped.trim();
    if capped.is_empty() {
        None
    } else {
        Some(capped.to_string())
    }
}

/// Split text into sentence-sized chunks for streaming synthesis. Heuristic:
/// break after `.`/`?`/`!` when followed by whitespace, unless the preceding
/// token is a known abbreviation or a single-letter initial. Never emits empty
/// chunks. (Decimals like `3.14` aren't split because the `.` isn't followed by
/// whitespace.)
pub fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        cur.push(c);
        if matches!(c, '.' | '?' | '!') {
            // Absorb a run of terminators ("?!", "...").
            while i + 1 < chars.len() && matches!(chars[i + 1], '.' | '?' | '!') {
                i += 1;
                cur.push(chars[i]);
            }
            let at_boundary = chars.get(i + 1).is_none_or(|n| n.is_whitespace());
            if at_boundary && !ends_with_abbrev(&cur) {
                let trimmed = cur.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
                cur.clear();
            }
        }
        i += 1;
    }
    let trimmed = cur.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

/// Byte index just past the first *confirmed* sentence boundary in `s` — a
/// `.?!` run immediately followed by whitespace, and not a known abbreviation.
/// Returns `None` if no boundary is confirmed yet (text ends mid-sentence, or at
/// a terminator with nothing after it). Lets the streaming accumulator emit a
/// sentence the moment it's complete, instead of waiting for the next to begin.
pub fn first_sentence_boundary(s: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if matches!(chars[i].1, '.' | '?' | '!') {
            let mut j = i;
            while j + 1 < n && matches!(chars[j + 1].1, '.' | '?' | '!') {
                j += 1;
            }
            if let Some(&(after_byte, next)) = chars.get(j + 1)
                && next.is_whitespace()
                && !ends_with_abbrev(&s[..after_byte])
            {
                return Some(after_byte);
            }
            i = j + 1;
            continue;
        }
        i += 1;
    }
    None
}

/// Common abbreviations that end in a period but don't end a sentence.
const ABBREVIATIONS: &[&str] = &[
    "eg", "ie", "etc", "mr", "mrs", "ms", "dr", "prof", "vs", "fig", "no", "st", "approx", "inc",
    "ltd", "jr", "sr", "al", "dept", "est",
];

fn ends_with_abbrev(s: &str) -> bool {
    let last = s.split_whitespace().last().unwrap_or("");
    let alpha: String = last
        .chars()
        .filter(|c| c.is_alphabetic())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if alpha.is_empty() {
        return false;
    }
    // A single-letter token before a period is almost always an initial.
    if alpha.chars().count() == 1 {
        return true;
    }
    ABBREVIATIONS.contains(&alpha.as_str())
}

/// Strip fenced code blocks, then markdown markup line-by-line, leaving prose.
fn prose_from_markdown(text: &str) -> String {
    let no_fence = strip_fenced_code(text);
    let lines: Vec<String> = no_fence.lines().map(strip_line_markdown).collect();
    collapse_blank_lines(&lines.join("\n"))
}

fn strip_fenced_code(text: &str) -> String {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push(line);
        }
    }
    out.join("\n")
}

fn strip_line_markdown(raw: &str) -> String {
    let mut t = raw.trim_start();
    // Blockquote markers (possibly nested).
    while let Some(rest) = t.strip_prefix('>') {
        t = rest.trim_start();
    }
    // ATX heading markers.
    let without_hashes = t.trim_start_matches('#');
    if without_hashes.len() != t.len() {
        t = without_hashes.trim_start();
    }
    // List bullets / ordered markers.
    let t = strip_bullet(t);
    // Inline constructs: images → alt, links → text, inline code → contents.
    let s = IMAGE_RE.replace_all(t, "$1");
    let s = LINK_RE.replace_all(&s, "$1");
    let s = INLINE_CODE_RE.replace_all(&s, "$1");
    // Remove any remaining emphasis / stray code markers.
    let s = s.replace(['*', '`'], "");
    s.trim().to_string()
}

fn strip_bullet(t: &str) -> &str {
    for p in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(p) {
            return rest.trim_start();
        }
    }
    // Ordered list: digits followed by ". " or ") ".
    let digit_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    if digit_end > 0 {
        let rest = &t[digit_end..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return r.trim_start();
        }
    }
    t
}

fn collapse_blank_lines(s: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut prev_blank = false;
    for line in s.lines() {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        out.push(line);
        prev_blank = blank;
    }
    out.join("\n").trim().to_string()
}

fn last_paragraph(prose: &str) -> String {
    prose
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .last()
        .unwrap_or("")
        .to_string()
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cap_length(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    if let Some(pos) = truncated.rfind(['.', '?', '!']) {
        return truncated[..=pos].to_string();
    }
    if let Some(pos) = truncated.rfind(' ') {
        return truncated[..pos].to_string();
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_token_roundtrip() {
        for scope in SpeakScope::ALL {
            assert_eq!(SpeakScope::from_token(scope.as_str()), Some(scope));
            assert!(!scope.label().is_empty());
        }
        assert_eq!(SpeakScope::from_token("bogus"), None);
        assert_eq!(SpeakScope::default(), SpeakScope::ProseOnly);
    }

    #[test]
    fn scope_serde_is_snake_case() {
        let json = serde_json::to_string(&SpeakScope::FinalSummary).unwrap();
        assert_eq!(json, "\"final_summary\"");
        let parsed: SpeakScope = serde_json::from_str("\"verbatim\"").unwrap();
        assert_eq!(parsed, SpeakScope::Verbatim);
    }

    #[test]
    fn prose_strips_fenced_code() {
        let blocks = vec![
            "Here is the fix:\n\n```rust\nlet x = 1;\npanic!(\"no\");\n```\n\nThat should work."
                .to_string(),
        ];
        let out = spoken_text(&blocks, SpeakScope::ProseOnly).unwrap();
        assert!(out.contains("Here is the fix"));
        assert!(out.contains("That should work"));
        assert!(!out.contains("let x"));
        assert!(!out.contains('`'));
    }

    #[test]
    fn prose_strips_inline_markup() {
        let blocks = vec![
            "Use `cargo build` and see [the docs](https://x.y) for **bold** and *italic* text."
                .to_string(),
        ];
        let out = spoken_text(&blocks, SpeakScope::ProseOnly).unwrap();
        assert!(!out.contains('`'));
        assert!(!out.contains('*'));
        assert!(!out.contains("https://"));
        assert!(out.contains("cargo build"));
        assert!(out.contains("the docs"));
        assert!(out.contains("bold"));
        assert!(out.contains("italic"));
    }

    #[test]
    fn prose_strips_headings_bullets_and_quotes() {
        let blocks = vec![
            "# Title\n\n- first item\n- second item\n\n1. one\n2. two\n\n> a quote".to_string(),
        ];
        let out = spoken_text(&blocks, SpeakScope::ProseOnly).unwrap();
        assert!(!out.contains('#'));
        assert!(!out.contains("- "));
        assert!(out.contains("first item"));
        assert!(out.contains("one"));
        assert!(out.contains("a quote"));
    }

    #[test]
    fn pure_code_reply_yields_none() {
        let blocks = vec!["```\nfn main() {}\n```".to_string()];
        assert_eq!(spoken_text(&blocks, SpeakScope::ProseOnly), None);
    }

    #[test]
    fn empty_blocks_yield_none() {
        assert_eq!(spoken_text(&[], SpeakScope::ProseOnly), None);
        assert_eq!(
            spoken_text(&["   ".to_string(), "".to_string()], SpeakScope::Verbatim),
            None
        );
    }

    #[test]
    fn final_summary_keeps_last_paragraph_only() {
        let blocks =
            vec!["First paragraph explaining things.\n\nThe final summary line.".to_string()];
        let out = spoken_text(&blocks, SpeakScope::FinalSummary).unwrap();
        assert_eq!(out, "The final summary line.");
    }

    #[test]
    fn verbatim_keeps_markup() {
        let blocks = vec!["Run `ls` now.".to_string()];
        let out = spoken_text(&blocks, SpeakScope::Verbatim).unwrap();
        assert!(out.contains('`'));
    }

    #[test]
    fn multiple_blocks_are_joined() {
        let blocks = vec!["First block.".to_string(), "Second block.".to_string()];
        let out = spoken_text(&blocks, SpeakScope::ProseOnly).unwrap();
        assert!(out.contains("First block"));
        assert!(out.contains("Second block"));
    }

    #[test]
    fn length_is_capped_on_sentence_boundary() {
        let long = "Word ".repeat(1000); // ~5000 chars, no sentence punctuation until end
        let blocks = vec![format!("{long}. Tail.")];
        let out = spoken_text(&blocks, SpeakScope::Verbatim).unwrap();
        assert!(out.chars().count() <= MAX_SPOKEN_CHARS);
    }

    #[test]
    fn split_sentences_basic() {
        let s = split_sentences("Hello there. How are you? I am fine!");
        assert_eq!(s, vec!["Hello there.", "How are you?", "I am fine!"]);
    }

    #[test]
    fn split_sentences_no_empty_chunks() {
        let s = split_sentences("One.   Two.    \n\n  Three.");
        assert_eq!(s, vec!["One.", "Two.", "Three."]);
        assert!(s.iter().all(|c| !c.trim().is_empty()));
    }

    #[test]
    fn split_sentences_keeps_decimals_and_abbreviations() {
        let s = split_sentences("It costs 3.14 dollars, e.g. cheap. Done.");
        // The decimal and "e.g." must not create spurious sentence breaks.
        assert_eq!(s, vec!["It costs 3.14 dollars, e.g. cheap.", "Done."]);
    }

    #[test]
    fn first_boundary_confirms_on_trailing_whitespace_only() {
        // No whitespace after the terminator yet → not confirmed.
        assert_eq!(first_sentence_boundary("Hello there."), None);
        // Whitespace confirms it; index points just past the terminator.
        assert_eq!(first_sentence_boundary("Hello there. "), Some(12));
        // Decimals and abbreviations are not boundaries.
        assert_eq!(first_sentence_boundary("It is 3.14 today"), None);
        assert_eq!(first_sentence_boundary("e.g. this"), None);
        // No terminator at all.
        assert_eq!(first_sentence_boundary("just words"), None);
    }

    #[test]
    fn split_sentences_single_chunk_when_no_terminator() {
        let s = split_sentences("no terminator here");
        assert_eq!(s, vec!["no terminator here"]);
    }

    proptest::proptest! {
        #[test]
        fn prose_never_contains_backticks_or_fences(input in ".{0,400}") {
            let blocks = vec![input];
            if let Some(out) = spoken_text(&blocks, SpeakScope::ProseOnly) {
                proptest::prop_assert!(!out.contains('`'));
                proptest::prop_assert!(!out.contains("```"));
            }
        }
    }
}
