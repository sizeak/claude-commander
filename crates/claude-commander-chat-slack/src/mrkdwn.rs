//! Standard Markdown → Slack *mrkdwn*.
//!
//! Agents always write ordinary Markdown; Slack speaks its own dialect, so the
//! backend converts on the way out. Covered: bold, italic, strikethrough, links,
//! headings (Slack has none — they become bold lines), bullets, blockquotes,
//! and the `&`/`<`/`>` escaping Slack requires. Inline code and fenced blocks
//! are escaped but otherwise left alone. Anything else passes through as text,
//! which Slack shows literally — readable, if not pretty.

use std::sync::LazyLock;

use regex::Regex;

static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s{0,3}#{1,6}\s+(.*?)\s*#*\s*$").unwrap());
static BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)[-*+]\s+(.*)$").unwrap());
static QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s{0,3}>\s?(.*)$").unwrap());
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]\(([^)\s]+)\)").unwrap());
static BOLD_STARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([^*\n]+?)\*\*").unwrap());
static BOLD_UNDERSCORES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"__([^_\n]+?)__").unwrap());
static ITALIC_STAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*([^*\s](?:[^*\n]*[^*\s])?)\*").unwrap());
static STRIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"~~([^~\n]+?)~~").unwrap());

/// Stands in for a bold `*` while single-`*` italics are rewritten, so the two
/// don't collide. Not a character Markdown text contains.
const BOLD_MARK: char = '\u{1}';

/// Convert `markdown` to Slack mrkdwn.
pub fn render(markdown: &str) -> String {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push(escape(line));
        } else if in_fence {
            out.push(escape(line));
        } else {
            out.push(render_line(line));
        }
    }
    out.join("\n")
}

fn render_line(line: &str) -> String {
    if let Some(caps) = HEADING.captures(line) {
        return format!("*{}*", render_inline(&caps[1]));
    }
    if let Some(caps) = QUOTE.captures(line) {
        return format!("> {}", render_inline(&caps[1]));
    }
    if let Some(caps) = BULLET.captures(line) {
        return format!("{}• {}", &caps[1], render_inline(&caps[2]));
    }
    render_inline(line)
}

/// Render a line's inline formatting, leaving `code spans` alone.
fn render_inline(text: &str) -> String {
    let parts: Vec<&str> = text.split('`').collect();
    // Backticks pair up when there's an odd number of parts; otherwise the
    // last one is unmatched and the text after it is plain.
    let unmatched_tail = parts.len().is_multiple_of(2);
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let after_backtick = !i.is_multiple_of(2);
        let is_tail = i + 1 == parts.len();
        if after_backtick && !(unmatched_tail && is_tail) {
            out.push('`');
            out.push_str(&escape(part));
            out.push('`');
        } else {
            if after_backtick {
                out.push('`');
            }
            out.push_str(&render_span(part));
        }
    }
    out
}

fn render_span(text: &str) -> String {
    let text = escape(text);
    // Only real URLs: inside `<…>` Slack reads `!channel`, `@U…` and
    // `!subteam^…` as pings, so any other target stays literal text.
    let text = LINK.replace_all(&text, |caps: &regex::Captures| {
        let target = &caps[2];
        if ["https://", "http://", "mailto:"]
            .iter()
            .any(|scheme| target.starts_with(scheme))
        {
            format!("<{target}|{}>", &caps[1])
        } else {
            caps[0].to_string()
        }
    });
    let bold = format!("{BOLD_MARK}$1{BOLD_MARK}");
    let text = BOLD_STARS.replace_all(&text, bold.as_str());
    let text = BOLD_UNDERSCORES.replace_all(&text, bold.as_str());
    let text = ITALIC_STAR.replace_all(&text, "_${1}_");
    let text = STRIKE.replace_all(&text, "~$1~");
    text.replace(BOLD_MARK, "*")
}

/// Slack requires `&`, `<` and `>` escaped in message text.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn emphasis_maps_to_mrkdwn() {
        assert_eq!(
            render("**bold**, __also bold__, *italic*, _italic_, ~~gone~~"),
            "*bold*, *also bold*, _italic_, _italic_, ~gone~"
        );
    }

    #[test]
    fn arithmetic_asterisks_are_not_italics() {
        assert_eq!(render("2 * 3 * 4"), "2 * 3 * 4");
    }

    #[test]
    fn links_become_slack_links_and_text_is_escaped() {
        assert_eq!(
            render("see [the PR](https://github.com/o/r/pull/1?a=1&b=2) & a < b > c"),
            "see <https://github.com/o/r/pull/1?a=1&amp;b=2|the PR> &amp; a &lt; b &gt; c"
        );
    }

    /// Slack reads `<!channel>`, `<@U…>` and `<!subteam^…>` inside angle
    /// brackets as pings, so only real URLs may become `<target|text>` links.
    #[test]
    fn only_web_and_mail_links_become_slack_links() {
        assert_eq!(render("[x](mailto:a@b.c)"), "<mailto:a@b.c|x>");
        for target in ["!channel", "!here", "@U123", "!subteam^S1", "javascript:x"] {
            let out = render(&format!("[x]({target})"));
            assert!(!out.contains('<'), "{target} produced {out}");
        }
    }

    #[test]
    fn headings_bullets_and_quotes() {
        assert_eq!(
            render("# Done\n- one\n* two\n  + three\n> quoted *bit*"),
            "*Done*\n• one\n• two\n  • three\n> quoted _bit_"
        );
    }

    #[test]
    fn code_is_escaped_but_not_formatted() {
        assert_eq!(
            render("run `a **b** <c>` now"),
            "run `a **b** &lt;c&gt;` now"
        );
        assert_eq!(
            render("```\n# not a heading\n**x** & y\n```\n**after**"),
            "```\n# not a heading\n**x** &amp; y\n```\n*after*"
        );
    }

    #[test]
    fn an_unmatched_backtick_is_plain_text() {
        assert_eq!(render("it's `odd **bold**"), "it's `odd *bold*");
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(
            render("All tests pass.\n\nPR: https://x/y"),
            "All tests pass.\n\nPR: https://x/y"
        );
    }
}
