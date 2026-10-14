import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/util/session_filter.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fixtures.dart';

void main() {
  // Scoring itself is no longer implemented in Dart: `fuzzyScore` and
  // `sessionFuzzyScore` delegate to the shared Rust scorer in
  // `claude-commander-viewmodel`, which is what stops the app and the terminal
  // ranking a session list differently. `flutter test` runs without the native
  // library, so their behaviour cannot be asserted here — it is covered by that
  // crate's own tests (`query.rs`: subsequence/case/empty-needle rules,
  // contiguous-beats-gappy, earlier-beats-later, best-field-wins, and the two
  // orderings the old Dart port inverted). Same split as
  // `test/support/fake_diff_layout.dart` documents for the diff engine.
  //
  // What remains testable here is the Dart that is still Dart: the ordering and
  // recency helpers, which take whatever scorer or key a caller supplies.

  /// Stand-in scorer. Deliberately trivial — these tests exercise filtering and
  /// ordering, not scoring, so a real scorer here would only obscure which is
  /// under test (and would need the native library).
  int? fakeSessionScore(SessionInfo s, String query) =>
      s.title.contains(query) ? s.title.length : null;

  group('sessionRecency', () {
    test('uses the attach time when the session has been attached', () {
      final s = sessionInfo(
        createdAt: DateTime.utc(2026, 1, 1),
        lastAttachedAt: DateTime.utc(2026, 1, 9),
      );
      expect(sessionRecency(s), DateTime.utc(2026, 1, 9));
    });

    test('falls back to the creation time when never attached', () {
      final s = sessionInfo(
        createdAt: DateTime.utc(2026, 1, 4),
        lastAttachedAt: null,
      );
      expect(sessionRecency(s), DateTime.utc(2026, 1, 4));
    });

    test('keeps a never-attached session in a mostRecent ordering', () {
      // The whole point of the fallback: keyed on lastAttachedAt alone, a
      // freshly created session has a null key and mostRecent drops it.
      final attached = sessionInfo(
        id: '11111111-1111-1111-1111-111111111111',
        lastAttachedAt: DateTime.utc(2026, 1, 2),
      );
      final justCreated = sessionInfo(
        id: '22222222-2222-2222-2222-222222222222',
        createdAt: DateTime.utc(2026, 1, 8),
        lastAttachedAt: null,
      );
      expect(mostRecent([attached, justCreated], sessionRecency), [
        justCreated,
        attached,
      ]);
    });
  });

  group('mostRecent', () {
    DateTime? at(SessionInfo s) => s.lastAttachedAt;

    test('drops sessions never attached (null timestamp)', () {
      final attached = sessionInfo(
        id: '11111111-1111-1111-1111-111111111111',
        lastAttachedAt: DateTime.utc(2026, 1, 2),
      );
      final never = sessionInfo(
        id: '22222222-2222-2222-2222-222222222222',
        lastAttachedAt: null,
      );
      final result = mostRecent([attached, never], at);
      expect(result, [attached]);
    });

    test('orders newest first', () {
      final older = sessionInfo(
        id: '11111111-1111-1111-1111-111111111111',
        lastAttachedAt: DateTime.utc(2026, 1, 1),
      );
      final newer = sessionInfo(
        id: '22222222-2222-2222-2222-222222222222',
        lastAttachedAt: DateTime.utc(2026, 1, 5),
      );
      expect(mostRecent([older, newer], at), [newer, older]);
    });

    test('respects the limit', () {
      final sessions = [
        for (var d = 1; d <= 5; d++)
          sessionInfo(
            id: '1111111$d-1111-1111-1111-111111111111',
            lastAttachedAt: DateTime.utc(2026, 1, d),
          ),
      ];
      expect(mostRecent(sessions, at, limit: 2).length, 2);
    });

    test('stable tie-break keeps input order for equal timestamps', () {
      final t = DateTime.utc(2026, 1, 1);
      final a = sessionInfo(
        id: '11111111-1111-1111-1111-111111111111',
        lastAttachedAt: t,
      );
      final b = sessionInfo(
        id: '22222222-2222-2222-2222-222222222222',
        lastAttachedAt: t,
      );
      expect(mostRecent([a, b], at), [a, b]);
    });
  });

  group('SessionStatus.isActive', () {
    test('stopped is inactive', () {
      expect(SessionStatus.stopped.isActive, isFalse);
    });

    test('every non-stopped status is active', () {
      for (final status in SessionStatus.values) {
        if (status == SessionStatus.stopped) continue;
        expect(status.isActive, isTrue, reason: '$status should be active');
      }
    });
  });

  group('matchingSessions', () {
    test('empty query keeps every session in order', () {
      final a = sessionInfo(title: 'a');
      final b = sessionInfo(
        id: '22222222-2222-2222-2222-555555555555',
        title: 'b',
      );
      expect(matchingSessions([a, b], '', score: fakeSessionScore), [a, b]);
    });

    test('filters to matches, preserving input order', () {
      final alpha = sessionInfo(title: 'alpha');
      final beta = sessionInfo(
        id: '22222222-2222-2222-2222-555555555555',
        title: 'beta',
      );
      final gamma = sessionInfo(
        id: '33333333-2222-2222-2222-555555555555',
        title: 'alphabet',
      );
      expect(
        matchingSessions([alpha, beta, gamma], 'alph', score: fakeSessionScore),
        [alpha, gamma],
      );
    });
  });

  group('rankByScore', () {
    test('orders best score first and drops null scores', () {
      // Scores supplied directly: this asserts rankByScore's ordering and
      // null-dropping, not how a scorer arrives at the numbers.
      const scores = {'axbxc': 5, 'abcxyz': 10, 'zzz': null};
      final ranked = rankByScore<String>([
        'axbxc',
        'abcxyz',
        'zzz',
      ], (s) => scores[s]);
      expect(ranked, ['abcxyz', 'axbxc']);
    });

    test('keeps input order as a stable tie-break for equal scores', () {
      // Same score (empty query → 0 for all); input order must survive.
      final ranked = rankByScore<String>(['x', 'y', 'z'], (_) => 0);
      expect(ranked, ['x', 'y', 'z']);
    });
  });
}
