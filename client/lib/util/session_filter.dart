import 'package:flutter/foundation.dart' show visibleForTesting;

import '../src/rust/api/mirrors.dart';
import '../src/rust/api/query.dart' as rust;

/// Session search + recency helpers.
///
/// Scoring is **not** implemented here any more. It used to be a Dart port of
/// the Rust scorer, which drifted: the port was greedy where Skim is optimal, so
/// the app and the terminal ranked the same session list differently. Both now
/// call one implementation in `claude-commander-viewmodel`, bridged
/// synchronously (see `rust/src/api/query.rs` for why sync).
///
/// The ordering helpers below are still Dart — they are generic sorts over
/// whatever key a caller supplies, which is not worth an FFI crossing — and need
/// no widget pump to unit-test.

/// The best fuzzy score for [query] across a session's title, branch and
/// program. Null when none of them match.
///
/// The field set and "best field wins" live with the scorer in Rust, so the
/// app cannot rank on a different set of fields than the TUI does. The project
/// name is not matched.
int? sessionFuzzyScore(SessionInfo session, String query) {
  final override = debugSessionScorer;
  if (override != null) return override(session, query);
  return rust.sessionScore(
    title: session.title,
    branch: session.branch,
    program: session.program,
    query: query,
  );
}

/// Test-only replacement for the scorer, used when it cannot be passed in.
///
/// [matchingSessions] takes a `score` parameter and that is the seam to prefer.
/// This exists for the path that has none: `session_list_page.dart` filters and
/// ranks inside `build`, so a widget test rendering it reaches the scorer with no
/// opportunity to inject one — and `flutter test` has no native library for the
/// real scorer to call into. Production never assigns this.
///
/// Set it in `setUp` and clear it in `tearDown`; leaving it set would silently
/// change scoring for every later test in the same file.
@visibleForTesting
int? Function(SessionInfo session, String query)? debugSessionScorer;

/// Whether a session's tmux process is still alive — every status except
/// `stopped`. Mirrors the Rust `SessionStatus::is_active`. The Recent tab
/// shows only active sessions: a stopped one isn't something you're actively
/// working in, so it shouldn't linger in the MRU list.
extension SessionStatusActive on SessionStatus {
  bool get isActive => this != SessionStatus.stopped;
}

/// [sessions] that match [query], in original order. An empty query keeps them
/// all. Used to filter a project group in place without reordering it.
/// [score] defaults to the shared Rust scorer. It is a parameter so this stays
/// unit-testable: `flutter test` runs without the native library, so a test
/// passes its own scorer rather than reaching the real one (the same reason
/// `test/support/fake_diff_layout.dart` exists for the diff engine).
List<SessionInfo> matchingSessions(
  Iterable<SessionInfo> sessions,
  String query, {
  int? Function(SessionInfo, String) score = sessionFuzzyScore,
}) {
  if (query.isEmpty) return sessions.toList();
  return [
    for (final s in sessions)
      if (score(s, query) != null) s,
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

/// How recent a session is for the Recent view: when it was last attached, or —
/// for one that has never been attached — when it was created.
///
/// The fallback is what keeps a freshly created session in that view. Keyed on
/// `lastAttachedAt` alone its key is null, and [mostRecent] drops null keys, so
/// the session the user had just made was absent from the tab — and absent from
/// the only place they could have attached it from, which is what would have
/// given it a key. Creation is a real recency signal, and the row already
/// falls back to it when rendering the session's age.
DateTime sessionRecency(SessionInfo s) => s.lastAttachedAt ?? s.createdAt;

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
