//! Syntax highlighting for the review diff body, via `syntect`.
//!
//! The bundled syntax/theme sets are loaded once (lazily) and shared. Each diff
//! line is highlighted independently — diffs are fragments, not whole files, so
//! per-line state is the pragmatic choice (multi-line constructs like block
//! comments aren't carried across hunk gaps). Only the foreground colour is
//! used; the review view supplies its own add/remove backgrounds.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::SyntaxSet;
use two_face::theme::EmbeddedThemeName;

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
        // Monokai Extended has vivid, near-white foregrounds that stay legible
        // on the coloured add/remove fills (base16 themes are tuned for a
        // near-black background and wash out over the fills).
        let theme = two_face::theme::extra()
            .get(EmbeddedThemeName::MonokaiExtended)
            .clone();
        Assets { syntaxes, theme }
    })
}

/// Process-global memo of highlight results, keyed by `(ext, content)`.
///
/// The review body is rebuilt on every render frame (every tick and keystroke),
/// and `highlight_line` constructs a fresh syntect `HighlightLines` per call —
/// the dominant per-frame cost. Diff content is immutable, so memoizing makes
/// scrolling and file-switching O(unique fragments) instead of re-highlighting
/// the whole file each frame.
///
/// The cache is a shared `Mutex` rather than a thread-local so the open-review
/// background task can warm it from a worker thread (see
/// `precompute_review_caches`) and have the render thread hit those entries.
/// After warming, every render is a cache read, so lock contention is a brief
/// lock/clone with no real waiting.
type HlKey = (String, String);
type HlCache = HashMap<HlKey, Vec<(String, Color)>>;
fn hl_cache() -> &'static Mutex<HlCache> {
    static HL_CACHE: OnceLock<Mutex<HlCache>> = OnceLock::new();
    HL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Soft cap on cached fragments; cleared wholesale if exceeded so a marathon
/// review session can't grow the map without bound. Far above any real diff.
const HL_CACHE_CAP: usize = 100_000;

/// Syntax-highlight one line of code into `(text, foreground)` runs.
///
/// `ext` is the file extension (no dot). When the extension isn't recognised,
/// or highlighting fails, the whole line is returned as a single `fallback` run
/// so callers always get usable spans. Results are memoized per `(ext, content)`
/// (see [`HL_CACHE`]); `fallback` is the stable palette text colour at all call
/// sites, so it is not part of the key.
pub(crate) fn highlight_line(content: &str, ext: &str, fallback: Color) -> Vec<(String, Color)> {
    let key = (ext.to_string(), content.to_string());
    if let Some(hit) = hl_cache().lock().unwrap().get(&key) {
        return hit.clone();
    }
    let runs = highlight_line_uncached(content, ext, fallback);
    let mut cache = hl_cache().lock().unwrap();
    if cache.len() >= HL_CACHE_CAP {
        cache.clear();
    }
    cache.insert(key, runs.clone());
    runs
}

/// The actual syntect highlight, without memoization (see [`highlight_line`]).
fn highlight_line_uncached(content: &str, ext: &str, fallback: Color) -> Vec<(String, Color)> {
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

    #[test]
    fn memoized_result_matches_uncached() {
        // The cache must be transparent: a (cold then warm) memoized call has to
        // equal a fresh syntect highlight for the same input.
        for (content, ext) in [
            ("let x = 1;", "rs"),
            ("const y: number = 2;", "ts"),
            ("plain text", "no-such-ext"),
        ] {
            let want = highlight_line_uncached(content, ext, Color::Reset);
            let cold = highlight_line(content, ext, Color::Reset);
            let warm = highlight_line(content, ext, Color::Reset);
            assert_eq!(cold, want, "cold cache must match uncached for .{ext}");
            assert_eq!(warm, want, "warm cache must match uncached for .{ext}");
        }
    }

    #[test]
    fn cache_does_not_cross_contaminate_extensions() {
        // Identical text under two languages must not collide in the cache: the
        // key includes the extension, so each keeps its own highlight.
        let content = "class Foo {}";
        let rust = highlight_line(content, "rs", Color::Reset);
        let ts = highlight_line(content, "ts", Color::Reset);
        // Reconstructed text is the same...
        let rust_text: String = rust.iter().map(|(t, _)| t.as_str()).collect();
        let ts_text: String = ts.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(rust_text, content);
        assert_eq!(ts_text, content);
        // ...and each equals its own uncached highlight (no key collision).
        assert_eq!(rust, highlight_line_uncached(content, "rs", Color::Reset));
        assert_eq!(ts, highlight_line_uncached(content, "ts", Color::Reset));
    }
}
