//! Syntax highlighting for the review diff body, via `syntect`.
//!
//! The bundled syntax/theme sets are loaded once (lazily) and shared. Each diff
//! line is highlighted independently — diffs are fragments, not whole files, so
//! per-line state is the pragmatic choice (multi-line constructs like block
//! comments aren't carried across hunk gaps). Only the foreground colour is
//! used; the review view supplies its own add/remove backgrounds.

use std::sync::OnceLock;

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

struct Assets {
    syntaxes: SyntaxSet,
    theme: Theme,
}

static ASSETS: OnceLock<Assets> = OnceLock::new();

fn assets() -> &'static Assets {
    ASSETS.get_or_init(|| {
        // The extended (bat) syntax set covers far more languages than
        // syntect's bundled defaults — notably TypeScript/TSX/TOML.
        let syntaxes = two_face::syntax::extra_newlines();
        let themes = ThemeSet::load_defaults();
        // A dark theme whose foregrounds read well on the diff fills. Fall back
        // to any bundled theme if the named one is ever absent.
        let theme = themes
            .themes
            .get("base16-mocha.dark")
            .or_else(|| themes.themes.values().next())
            .cloned()
            .expect("syntect ships default themes");
        Assets { syntaxes, theme }
    })
}

/// Syntax-highlight one line of code into `(text, foreground)` runs.
///
/// `ext` is the file extension (no dot). When the extension isn't recognised,
/// or highlighting fails, the whole line is returned as a single `fallback` run
/// so callers always get usable spans.
pub(crate) fn highlight_line(content: &str, ext: &str, fallback: Color) -> Vec<(String, Color)> {
    let assets = assets();
    let Some(syntax) = assets.syntaxes.find_syntax_by_extension(ext) else {
        return vec![(content.to_string(), fallback)];
    };
    let mut highlighter = HighlightLines::new(syntax, &assets.theme);
    match highlighter.highlight_line(content, &assets.syntaxes) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| {
                let fg = style.foreground;
                (text.to_string(), Color::Rgb(fg.r, fg.g, fg.b))
            })
            .collect(),
        Err(_) => vec![(content.to_string(), fallback)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_known_extension_into_runs() {
        let runs = highlight_line("let x = 1;", "rs", Color::Reset);
        // Rust is a bundled syntax, so we get multiple coloured runs, none of
        // which fall back to Reset.
        assert!(runs.len() > 1, "expected tokenised runs, got {runs:?}");
        assert!(runs.iter().all(|(_, c)| *c != Color::Reset));
        let text: String = runs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "let x = 1;");
    }

    #[test]
    fn extended_syntax_set_covers_typescript_and_friends() {
        // These extensions are absent from syntect's bundled defaults but are
        // provided by the extended (two-face) set.
        for ext in ["ts", "tsx", "toml"] {
            assert!(
                assets().syntaxes.find_syntax_by_extension(ext).is_some(),
                "expected syntax for .{ext}"
            );
        }
        // And a multi-run highlight actually happens for TypeScript.
        let runs = highlight_line("const x: number = 1;", "ts", Color::Reset);
        assert!(runs.len() > 1, "expected tokenised TS runs, got {runs:?}");
    }

    #[test]
    fn unknown_extension_falls_back_to_single_run() {
        let runs = highlight_line("some text", "no-such-ext", Color::Reset);
        assert_eq!(runs, vec![("some text".to_string(), Color::Reset)]);
    }
}
