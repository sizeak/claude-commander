//! Syntax highlighting for the review diff body, via `syntect`.
//!
//! The bundled syntax/theme sets are loaded once (lazily) and shared. Each diff
//! line is highlighted independently — diffs are fragments, not whole files, so
//! per-line state is the pragmatic choice (multi-line constructs like block
//! comments aren't carried across hunk gaps). Only the foreground colour is
//! used; the review view supplies its own add/remove backgrounds.
//!
//! This is the review view's [`diffgrid::style::Highlighter`]. `diffgrid` takes a
//! trait rather than a highlighter of its own precisely so that each host can
//! bring the one its platform wants — `syntect` here, Prism in a browser,
//! `flutter_highlight` in the client — and it deals in [`Rgb`], not
//! `ratatui::style::Color`, so nothing below the TUI has an opinion about how a
//! colour is drawn.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use diffgrid::style::{Highlighter, Rgb};
use rayon::prelude::*;
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::SyntaxSet;
use two_face::theme::EmbeddedThemeName;

struct Assets {
    syntaxes: SyntaxSet,
    theme: Theme,
}

static ASSETS: OnceLock<Assets> = OnceLock::new();

/// Force the one-time load of the syntax/theme assets. Loading the extended
/// two-face syntax set is tens of milliseconds the first highlight would
/// otherwise pay; calling this at startup moves it off the first review-open's
/// critical path. Idempotent (the `OnceLock` only initialises once).
pub(crate) fn warm_assets() {
    let _ = assets();
}

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

/// Coloured runs of one line: `(byte range into the line, colour)`, ascending
/// and non-overlapping, as [`Highlighter::highlight`] requires. An empty list
/// means "no highlighting" — the palette's default foreground covers the whole
/// line — which is also how an unrecognised extension is reported.
type HlRuns = Vec<(Range<usize>, Rgb)>;

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
type HlCache = HashMap<HlKey, HlRuns>;
fn hl_cache() -> &'static Mutex<HlCache> {
    static HL_CACHE: OnceLock<Mutex<HlCache>> = OnceLock::new();
    HL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Soft cap on cached fragments; cleared wholesale if exceeded so a marathon
/// review session can't grow the map without bound. Far above any real diff.
const HL_CACHE_CAP: usize = 100_000;

/// The review view's syntax highlighter: `syntect` behind `diffgrid`'s trait.
///
/// Zero-sized — the syntax set, the theme and the memo are all process-global —
/// so a caller constructs one wherever it needs to pass `&dyn Highlighter`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SyntectHighlighter;

impl Highlighter for SyntectHighlighter {
    fn highlight(&self, language: Option<&str>, text: &str) -> HlRuns {
        highlight_line(text, language.unwrap_or_default())
    }
}

/// Syntax-highlight one line of code into `(byte range, foreground)` runs.
///
/// `ext` is the file extension (no dot). When the extension isn't recognised,
/// or highlighting fails, the result is empty and the caller's default
/// foreground covers the line. Results are memoized per `(ext, content)`.
pub(crate) fn highlight_line(content: &str, ext: &str) -> HlRuns {
    let key = (ext.to_string(), content.to_string());
    if let Some(hit) = hl_cache().lock().unwrap().get(&key) {
        return hit.clone();
    }
    let runs = highlight_line_uncached(content, ext);
    let mut cache = hl_cache().lock().unwrap();
    if cache.len() >= HL_CACHE_CAP {
        cache.clear();
    }
    cache.insert(key, runs.clone());
    runs
}

/// Warm the cache for many `(ext, content)` lines at once, computing the
/// uncached highlights **in parallel with no lock held** and inserting them
/// under a single lock at the end.
///
/// Calling [`highlight_line`] per line from N threads would serialise on the
/// cache mutex twice per line; the review precompute warms a whole diff's worth
/// of lines, so doing the heavy syntect work lock-free and batching the write is
/// what actually buys the parallel speedup. Lines already cached, and duplicate
/// lines within the batch, are computed at most once. After this returns, a
/// later `highlight_line` for any warmed line is a pure cache read.
pub(crate) fn warm_highlight_cache(lines: &[(&str, &str)]) {
    // De-dup, and skip lines already cached, so each unique fragment is
    // highlighted at most once. One short lock to read membership.
    let unique: Vec<(&str, &str)> = {
        let cache = hl_cache().lock().unwrap();
        let mut seen = std::collections::HashSet::new();
        lines
            .iter()
            .copied()
            .filter(|(ext, content)| seen.insert((*ext, *content)))
            .filter(|(ext, content)| {
                !cache.contains_key(&((*ext).to_string(), (*content).to_string()))
            })
            .collect()
    };
    if unique.is_empty() {
        return;
    }
    let computed: Vec<(HlKey, HlRuns)> = unique
        .par_iter()
        .map(|(ext, content)| {
            (
                ((*ext).to_string(), (*content).to_string()),
                highlight_line_uncached(content, ext),
            )
        })
        .collect();
    let mut cache = hl_cache().lock().unwrap();
    if cache.len() + computed.len() >= HL_CACHE_CAP {
        cache.clear();
    }
    cache.extend(computed);
}

/// The actual syntect highlight, without memoization (see [`highlight_line`]).
///
/// Returns byte ranges rather than owned text: `syntect` hands back subslices of
/// `content` whose lengths sum to it, so the offsets fall out of a running
/// total — and the cache then stores offsets instead of a second copy of every
/// diff line.
fn highlight_line_uncached(content: &str, ext: &str) -> HlRuns {
    let assets = assets();
    let Some(syntax) = assets.syntaxes.find_syntax_by_extension(ext) else {
        return Vec::new();
    };
    let mut highlighter = HighlightLines::new(syntax, &assets.theme);
    let Ok(ranges) = highlighter.highlight_line(content, &assets.syntaxes) else {
        return Vec::new();
    };
    let mut pos = 0usize;
    ranges
        .into_iter()
        .map(|(style, text)| {
            let start = pos;
            pos += text.len();
            let fg = style.foreground;
            (start..pos, Rgb::new(fg.r, fg.g, fg.b))
        })
        .filter(|(range, _)| !range.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reassemble the highlighted text from the runs' byte ranges, which is the
    /// contract `diffgrid` slices `content` by.
    fn covered(content: &str, runs: &HlRuns) -> String {
        runs.iter().map(|(r, _)| &content[r.clone()]).collect()
    }

    #[test]
    fn highlights_known_extension_into_runs() {
        let content = "let x = 1;";
        let runs = highlight_line(content, "rs");
        // Rust is a bundled syntax, so we get multiple coloured runs...
        assert!(runs.len() > 1, "expected tokenised runs, got {runs:?}");
        // ...that tile the line exactly, leaving nothing uncoloured.
        assert_eq!(covered(content, &runs), content);
    }

    /// Ascending, non-overlapping and on char boundaries — what the trait
    /// requires, and what a badly-behaved highlighter would silently corrupt.
    #[test]
    fn runs_are_ascending_non_overlapping_and_char_aligned() {
        let content = "let café = \"日本\"; // ok";
        let runs = SyntectHighlighter.highlight(Some("rs"), content);
        let mut prev_end = 0;
        for (range, _) in &runs {
            assert!(range.start >= prev_end, "runs must not overlap: {runs:?}");
            assert!(content.is_char_boundary(range.start));
            assert!(content.is_char_boundary(range.end));
            prev_end = range.end;
        }
        assert_eq!(covered(content, &runs), content);
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
        let runs = highlight_line("const x: number = 1;", "ts");
        assert!(runs.len() > 1, "expected tokenised TS runs, got {runs:?}");
    }

    /// An unrecognised extension yields no runs at all, which `diffgrid` renders
    /// as the palette's default foreground over the whole line. Guessing a
    /// syntax would be worse than not highlighting.
    #[test]
    fn unknown_extension_yields_no_runs() {
        assert!(highlight_line("some text", "no-such-ext").is_empty());
        assert!(SyntectHighlighter.highlight(None, "some text").is_empty());
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
            let want = highlight_line_uncached(content, ext);
            let cold = highlight_line(content, ext);
            let warm = highlight_line(content, ext);
            assert_eq!(cold, want, "cold cache must match uncached for .{ext}");
            assert_eq!(warm, want, "warm cache must match uncached for .{ext}");
        }
    }

    #[test]
    fn warm_cache_makes_highlight_line_match_uncached() {
        // After warming, a `highlight_line` for each warmed line must equal a
        // fresh uncached highlight. Includes a duplicate (de-dup path) and an
        // unknown extension (fallback path). Use extensions unlikely to be
        // pre-warmed by other tests in this process so the warm path is taken.
        let lines = [
            ("rs", "fn warmed() {}"),
            ("rs", "fn warmed() {}"), // duplicate → highlighted at most once
            ("toml", "warmed = true"),
            ("no-such-ext", "warm plain"),
        ];
        warm_highlight_cache(&lines);
        for (ext, content) in lines {
            assert_eq!(
                highlight_line(content, ext),
                highlight_line_uncached(content, ext),
                "warmed entry must match uncached for .{ext}"
            );
        }
    }

    #[test]
    fn warm_cache_empty_input_is_a_noop() {
        // Must not panic or clear anything for an empty batch.
        warm_highlight_cache(&[]);
    }

    #[test]
    fn cache_does_not_cross_contaminate_extensions() {
        // Identical text under two languages must not collide in the cache: the
        // key includes the extension, so each keeps its own highlight.
        let content = "class Foo {}";
        let rust = highlight_line(content, "rs");
        let ts = highlight_line(content, "ts");
        // Both still tile the same text...
        assert_eq!(covered(content, &rust), content);
        assert_eq!(covered(content, &ts), content);
        // ...and each equals its own uncached highlight (no key collision).
        assert_eq!(rust, highlight_line_uncached(content, "rs"));
        assert_eq!(ts, highlight_line_uncached(content, "ts"));
    }
}
