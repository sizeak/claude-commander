import '../src/rust/api/mirrors.dart';

/// Session search + recency helpers, mirroring the TUI's semantics.
///
/// These are deliberately Flutter-free so they unit-test without a widget pump.
/// The Rust core matches with `SkimMatcherV2`; here we re-implement a
/// case-insensitive subsequence scorer in Dart (chosen over an FFI bridge). It
/// won't be byte-identical to Skim, but preserves the properties the UI relies
/// on: a subsequence matches, a non-subsequence doesn't, and a tighter/earlier
/// hit ranks above a looser/later one.

/// A fuzzy subsequence score for [needle] within [haystack], or null when
/// [needle] is not a subsequence. Higher is a better match. An empty needle
/// scores 0 (matches everything), mirroring `fuzzy::fuzzy_score`.
///
/// Match/no-match is exact (a subsequence always scores), so All-mode filtering
/// is precise. The *score* is greedy — it commits to the leftmost occurrence of
/// each char rather than Skim's DP optimum — so ranking between two haystacks
/// that both contain a tight hit can be slightly off. That only nudges the
/// Recent tab's ordering, never whether a row shows.
int? fuzzyScore(String haystack, String needle) {
  if (needle.isEmpty) return 0;
  final h = haystack.toLowerCase();
  final n = needle.toLowerCase();

  var hi = 0;
  var score = 0;
  var lastMatch = -2; // guarantees the first match is never "contiguous"
  var firstMatch = -1;

  for (var ni = 0; ni < n.length; ni++) {
    final c = n.codeUnitAt(ni);
    var matched = false;
    while (hi < h.length) {
      if (h.codeUnitAt(hi) == c) {
        firstMatch = firstMatch < 0 ? hi : firstMatch;
        // Reward matches that continue a run and those on a word boundary, so
        // "auth" scores higher against "auth-fix" than against "a-u-t-h".
        score += (hi == lastMatch + 1) ? 10 : 1;
        if (hi == 0 || _isBoundary(h.codeUnitAt(hi - 1))) score += 5;
        lastMatch = hi;
        hi++;
        matched = true;
        break;
      }
      hi++;
    }
    if (!matched) return null;
  }

  // An earlier first hit is better; nudge the score down by where matching began.
  return score - firstMatch;
}

/// True when [codeUnit] is a separator that makes the next char a word start.
bool _isBoundary(int codeUnit) {
  final isAlnum =
      (codeUnit >= 0x30 && codeUnit <= 0x39) || // 0-9
      (codeUnit >= 0x61 && codeUnit <= 0x7a); // a-z (haystack is lowercased)
  return !isAlnum;
}

/// The best fuzzy score for [query] across a session's title, branch, and
/// program — the same three fields the TUI's `WorktreeSession::fuzzy_score`
/// scores. `projectName` is shown in the list but, as in the TUI, not matched.
/// Returns null when the query matches none of the fields.
int? sessionFuzzyScore(SessionInfo session, String query) {
  int? best;
  for (final field in [session.title, session.branch, session.program]) {
    final score = fuzzyScore(field, query);
    if (score != null && (best == null || score > best)) best = score;
  }
  return best;
}

/// [sessions] that match [query], in original order. An empty query keeps them
/// all. Used to filter a project group in place without reordering it.
List<SessionInfo> matchingSessions(
  Iterable<SessionInfo> sessions,
  String query,
) {
  if (query.isEmpty) return sessions.toList();
  return [
    for (final s in sessions)
      if (sessionFuzzyScore(s, query) != null) s,
  ];
}

/// [items] whose [score] is non-null, best score first, with input order kept
/// as a stable tie-break — so ranking a recency-ordered input by fuzzy score
/// keeps recency as the secondary sort. Items scoring null are dropped.
List<E> rankByScore<E>(Iterable<E> items, int? Function(E) score) {
  final scored = <(int, int, E)>[];
  var i = 0;
  for (final item in items) {
    final s = score(item);
    if (s != null) scored.add((s, i, item));
    i++;
  }
  scored.sort((a, b) {
    final byScore = b.$1.compareTo(a.$1); // best match first
    return byScore != 0 ? byScore : a.$2.compareTo(b.$2); // then input order
  });
  return [for (final e in scored) e.$3];
}

/// The [items] carrying the most recent [attachedAt] timestamps, newest first.
///
/// A generic port of the TUI's `order_recent`: items whose key is null (never
/// attached) are dropped, the rest are ordered by timestamp descending with a
/// stable tie-break (input order preserved), and the result is capped at
/// [limit] when given. Generic over the element so it works over bare sessions
/// or `(store, session)` pairs across servers.
List<E> mostRecent<E>(
  Iterable<E> items,
  DateTime? Function(E) attachedAt, {
  int? limit,
}) {
  final indexed = <(DateTime, int, E)>[];
  var i = 0;
  for (final item in items) {
    final at = attachedAt(item);
    if (at != null) indexed.add((at, i, item));
    i++;
  }
  indexed.sort((a, b) {
    final byTime = b.$1.compareTo(a.$1); // newest first
    return byTime != 0 ? byTime : a.$2.compareTo(b.$2); // stable
  });
  final ordered = [for (final e in indexed) e.$3];
  if (limit != null && ordered.length > limit) {
    return ordered.sublist(0, limit);
  }
  return ordered;
}
