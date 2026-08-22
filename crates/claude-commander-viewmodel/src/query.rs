//! Matching a typed query against sessions.
//!
//! The scorer is shared rather than reimplemented per frontend: the TUI and the
//! Flutter client rank the same session list, so they must score it the same way.

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// A matcher for one scoring call.
///
/// Built per call, deliberately, because a **reused** `SkimMatcherV2` does not
/// score deterministically. It keeps per-thread scratch buffers
/// (`m_cache: CachedThreadLocal<RefCell<Vec<MatrixCell>>>`,
/// fuzzy-matcher-0.3.7 `src/skim.rs:610`) and grows them with
/// `matrix.resize(rows * cols, MatrixCell::default())` (`src/skim.rs:394`).
/// `Vec::resize` initialises only the *new* elements, so cells left over from a
/// longer previous haystack keep that call's DP values, and the matrix build
/// reads cells this call never wrote. The score therefore depends on what the
/// thread scored before.
///
/// That is not academic. With one shared matcher, scoring `"s-e-s-s-i-o-n"`
/// against `"session"` (145) before `"list-session"` (142 alone) makes the
/// second report 145 too — turning a strict ordering into a tie. The whole point
/// of this crate is that the TUI and the Flutter client rank a list identically,
/// and they run on different threads with different histories, so a
/// history-dependent scorer cannot deliver that.
///
/// `use_cache(false)` is NOT the fix: in 0.3.7 that path does
/// `cell.replace(vec![])` on caches whose `RefCell` borrows are still live
/// (`src/skim.rs:929-933`) and panics with "already borrowed".
///
/// Cost, measured: a 180-field keystroke (60 sessions × 3 fields) takes ~114µs
/// with a fresh matcher against ~87µs shared — 27µs against a 16.7ms frame.
/// `ignore_case` matches regardless of needle case, preserving the original
/// lowercase-then-contains behaviour users were used to.
fn matcher() -> SkimMatcherV2 {
    SkimMatcherV2::default().ignore_case()
}

/// Score `needle` against `haystack` using Skim-style fuzzy matching.
///
/// Returns `Some(score)` when every char of `needle` appears in `haystack`
/// in order (case-insensitive), `None` otherwise. Higher score = better
/// match. An empty needle always scores `Some(0)` so callers can filter
/// without special-casing the empty-query path.
pub fn fuzzy_score(haystack: &str, needle: &str) -> Option<i64> {
    if needle.is_empty() {
        return Some(0);
    }
    matcher().fuzzy_match(haystack, needle)
}

/// Best fuzzy score for `query` across a session's title, branch and program —
/// or `None` when no field matches. The fields, and "best field wins", are the
/// ranking rule the palette and the client's session search both apply.
///
/// Takes the three fields as `&str` rather than a session struct on purpose:
/// both a core `WorktreeSession` and a protocol `SessionInfo` can call it, and
/// it crosses the Flutter client's FFI boundary without marshalling a struct.
/// The project name is deliberately *not* matched.
pub fn session_score(title: &str, branch: &str, program: &str, query: &str) -> Option<i64> {
    [title, branch, program]
        .iter()
        .filter_map(|s| fuzzy_score(s, query))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matches() {
        assert!(fuzzy_score("android-record-2", "andr2").is_some());
        assert!(fuzzy_score("android-record-2", "rec2").is_some());
        assert!(fuzzy_score("android-record-2", "and-2").is_some());
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert!(fuzzy_score("android-record-2", "xyz").is_none());
        // Out-of-order chars: needle order must be preserved.
        assert!(fuzzy_score("android-record-2", "2andr").is_none());
    }

    #[test]
    fn case_insensitive() {
        assert!(fuzzy_score("Android-Record-2", "andr2").is_some());
        assert!(fuzzy_score("android-record-2", "ANDR2").is_some());
    }

    #[test]
    fn empty_needle_matches_everything() {
        assert_eq!(fuzzy_score("anything", ""), Some(0));
        assert_eq!(fuzzy_score("", ""), Some(0));
    }

    #[test]
    fn contiguous_outranks_scattered() {
        let tight = fuzzy_score("android", "andr").unwrap();
        let loose = fuzzy_score("a-n-d-r-oid", "andr").unwrap();
        assert!(tight > loose, "tight={tight} loose={loose}");
    }

    #[test]
    fn session_score_takes_the_best_field() {
        // "payments" matches the title exactly and the branch loosely; the
        // better field must win, so the pair is not merely "some match".
        let title_only = fuzzy_score("payments", "payments").unwrap();
        let best = session_score("payments", "feat/pay-ments-api", "claude", "payments").unwrap();
        assert_eq!(best, title_only);
    }

    #[test]
    fn session_score_matches_on_branch_or_program_too() {
        assert!(session_score("unrelated", "feature-auth", "claude", "auth").is_some());
        assert!(session_score("unrelated", "main", "opencode", "opencode").is_some());
    }

    #[test]
    fn session_score_is_none_when_no_field_matches() {
        assert!(session_score("alpha", "beta", "claude", "zzz").is_none());
    }

    #[test]
    fn session_score_empty_query_matches() {
        assert_eq!(session_score("a", "b", "c", ""), Some(0));
    }

    // ---- coverage moved here from the Dart port -------------------------
    //
    // `client/test/util/session_filter_test.dart` asserted these against a
    // second, Dart implementation of the scorer. That implementation is gone —
    // the Flutter client now calls this one over FFI — and `flutter test` runs
    // without the native library, so the assertions live here instead. This is
    // the same split `client/test/support/fake_diff_layout.dart` documents for
    // the diff engine: the behaviour is covered by the Rust tests, and a Dart
    // test that wanted it would only be testing its own stand-in.

    #[test]
    fn earlier_match_outranks_later() {
        // Moved from Dart's "earlier match scores higher than a later one".
        let early = fuzzy_score("auth-service", "auth").unwrap();
        let late = fuzzy_score("service-auth", "auth").unwrap();
        assert!(early > late, "early={early} late={late}");
    }

    #[test]
    fn session_score_matches_each_field_individually() {
        // Moved from Dart's "matches against title, branch, and program".
        let s = |q| session_score("Refactor login", "auth-refactor", "claude", q);
        assert!(s("login").is_some(), "title");
        assert!(s("auth").is_some(), "branch");
        assert!(s("claude").is_some(), "program");
    }

    /// Dart also asserted that a session's *project name* never matches. That is
    /// structural here rather than testable: [`session_score`] takes three
    /// fields and no project, so there is nothing to exclude. Recorded so the
    /// guarantee is not silently lost with the test that used to state it.
    #[test]
    fn session_score_has_no_project_parameter() {
        // Compiles only while the signature stays (title, branch, program, query).
        let f: fn(&str, &str, &str, &str) -> Option<i64> = session_score;
        assert!(f("t", "b", "p", "t").is_some());
    }

    /// Why this scorer is shared rather than reimplemented per frontend.
    ///
    /// The Flutter client used to carry a greedy Dart port that committed to the
    /// leftmost occurrence of each character, and said so: *"It won't be
    /// byte-identical to Skim… ranking between two haystacks that both contain a
    /// tight hit can be slightly off."* These are two real cases where that
    /// greedy choice **inverts** the order this scorer produces, so the terminal
    /// and the app ranked the same session list differently.
    ///
    /// Asserted as orderings, not absolute scores: ordering is what the UI
    /// depends on, and pinning Skim's exact numbers would break on a
    /// `fuzzy-matcher` bump for no user-visible reason.
    #[test]
    fn ordering_the_greedy_dart_port_used_to_invert() {
        // A haystack whose every character sits on a word boundary beats one
        // with a contiguous run mid-word. The greedy port preferred the run.
        let boundaries = fuzzy_score("a-u-t-h", "auth").unwrap();
        let mid_word_run = fuzzy_score("fix-auth-bug", "auth").unwrap();
        assert!(
            boundaries > mid_word_run,
            "all-boundary should outrank a mid-word run: {boundaries} vs {mid_word_run}"
        );

        // Skipping a decoy leading "a" to start on a boundary beats taking it.
        // The greedy port took the leftmost "a" and could not recover.
        let decoy = fuzzy_score("a-auth", "auth").unwrap();
        assert!(
            decoy < boundaries,
            "a leading decoy should score below a clean boundary run: {decoy} vs {boundaries}"
        );
    }

    /// The scorer must be a *function* of its arguments.
    ///
    /// It was not: with the shared `SkimMatcherV2` this crate first shipped,
    /// scoring `"s-e-s-s-i-o-n"` first left stale DP cells that pushed
    /// `"list-session"` from 142 to 145 — collapsing a strict ordering into a
    /// tie. Two frontends on different threads would then rank the same list
    /// differently, which is precisely what this crate exists to prevent.
    ///
    /// Asserts the actual property (same inputs, same answer, whatever ran
    /// before) rather than the mechanism, so it still guards if the fix changes.
    #[test]
    fn scoring_is_independent_of_call_history() {
        let alone = fuzzy_score("list-session", "session");

        // Contaminating call: a longer haystack for the same needle, which is
        // what grew the shared scratch buffer.
        let _ = fuzzy_score("s-e-s-s-i-o-n", "session");
        assert_eq!(
            fuzzy_score("list-session", "session"),
            alone,
            "score changed after scoring a longer haystack first"
        );

        // And a spread of unrelated work, in case some other call pattern leaks.
        for (h, n) in [
            ("authentication-service", "auth"),
            ("a-very-long-branch-name-indeed", "brnch"),
            ("payments", "pay"),
        ] {
            let _ = fuzzy_score(h, n);
        }
        assert_eq!(
            fuzzy_score("list-session", "session"),
            alone,
            "score changed after unrelated scoring"
        );
    }

    /// The ordering the shared-matcher bug used to destroy: a haystack whose
    /// every character sits on a word boundary outranks one where the match is
    /// contiguous but buried. Restored as a regression test now that the scorer
    /// is deterministic enough to assert it.
    #[test]
    fn boundary_run_outranks_a_buried_contiguous_run() {
        let boundaries = fuzzy_score("s-e-s-s-i-o-n", "session").unwrap();
        let buried = fuzzy_score("list-session", "session").unwrap();
        assert!(
            boundaries > buried,
            "all-boundary should outrank a buried run: {boundaries} vs {buried}"
        );
    }
}
