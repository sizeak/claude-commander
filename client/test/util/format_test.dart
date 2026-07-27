import 'package:claude_commander_client/util/format.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('relativeAge', () {
    final now = DateTime.utc(2026, 1, 1, 12, 0, 0);
    String age(Duration ago) => relativeAge(now.subtract(ago), now: now);

    test('sub-5s and future render as now', () {
      expect(age(const Duration(seconds: 2)), 'now');
      expect(relativeAge(now.add(const Duration(minutes: 5)), now: now), 'now');
    });

    test('seconds / minutes / hours / days buckets', () {
      expect(age(const Duration(seconds: 30)), '30s');
      expect(age(const Duration(minutes: 2)), '2m');
      expect(age(const Duration(minutes: 59)), '59m');
      expect(age(const Duration(hours: 1)), '1h');
      expect(age(const Duration(hours: 23)), '23h');
      expect(age(const Duration(days: 3)), '3d');
    });

    test('weeks / months / years buckets', () {
      expect(age(const Duration(days: 10)), '1w');
      expect(age(const Duration(days: 60)), '2mo');
      expect(age(const Duration(days: 800)), '2y');
    });
  });
}
