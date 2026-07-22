import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/util/session_filter.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fixtures.dart';

void main() {
  group('fuzzyScore', () {
    test('empty needle matches everything with score 0', () {
      expect(fuzzyScore('anything', ''), 0);
    });

    test('non-subsequence returns null', () {
      expect(fuzzyScore('auth-fix', 'zzz'), isNull);
      // Right chars, wrong order: not a subsequence.
      expect(fuzzyScore('abc', 'cba'), isNull);
    });

    test('is case-insensitive', () {
      expect(fuzzyScore('Auth-Fix', 'auth'), isNotNull);
      expect(fuzzyScore('auth-fix', 'AUTH'), isNotNull);
    });

    test('contiguous match scores higher than a gappy one', () {
      final tight = fuzzyScore('abcxyz', 'abc')!;
      final gappy = fuzzyScore('axbxc', 'abc')!;
      expect(tight, greaterThan(gappy));
    });

    test('earlier match scores higher than a later one', () {
      final early = fuzzyScore('auth-service', 'auth')!;
      final late = fuzzyScore('service-auth', 'auth')!;
      expect(early, greaterThan(late));
    });
  });

  group('sessionFuzzyScore', () {
    test('matches against title, branch, and program', () {
      final s = sessionInfo(
        title: 'Refactor login',
        branch: 'auth-refactor',
        program: 'claude',
      );
      expect(sessionFuzzyScore(s, 'login'), isNotNull); // title
      expect(sessionFuzzyScore(s, 'auth'), isNotNull); // branch
      expect(sessionFuzzyScore(s, 'claude'), isNotNull); // program
    });

    test('does not match against projectName', () {
      final s = sessionInfo(
        title: 'unrelated',
        branch: 'b',
        program: 'p',
        projectName: 'my-special-repo',
      );
      expect(sessionFuzzyScore(s, 'special'), isNull);
    });

    test('returns the best score across fields', () {
      final s = sessionInfo(title: 'auth', branch: 'a-u-t-h', program: 'x');
      // Contiguous hit in the title should beat the gappy branch hit.
      expect(sessionFuzzyScore(s, 'auth'), fuzzyScore('auth', 'auth'));
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

  group('matchingSessions', () {
    test('empty query keeps every session in order', () {
      final a = sessionInfo(title: 'a');
      final b = sessionInfo(
        id: '22222222-2222-2222-2222-555555555555',
        title: 'b',
      );
      expect(matchingSessions([a, b], ''), [a, b]);
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
      expect(matchingSessions([alpha, beta, gamma], 'alph'), [alpha, gamma]);
    });
  });

  group('rankByScore', () {
    test('orders best score first and drops null scores', () {
      final ranked = rankByScore<String>([
        'axbxc',
        'abcxyz',
        'zzz',
      ], (s) => fuzzyScore(s, 'abc'));
      // Both 'axbxc' and 'abcxyz' match; the tighter (contiguous) hit ranks
      // first, and the non-match 'zzz' is dropped.
      expect(ranked, ['abcxyz', 'axbxc']);
    });

    test('keeps input order as a stable tie-break for equal scores', () {
      // Same score (empty query → 0 for all); input order must survive.
      final ranked = rankByScore<String>(['x', 'y', 'z'], (_) => 0);
      expect(ranked, ['x', 'y', 'z']);
    });
  });
}
