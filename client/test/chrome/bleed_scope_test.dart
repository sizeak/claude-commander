import 'package:claude_commander_client/chrome/lcars/bleed.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('reads the ambient bleed', (tester) async {
    late EdgeInsets seen;
    await tester.pumpWidget(
      LcarsBleedScope(
        bleed: const EdgeInsets.only(top: 24, bottom: 48),
        child: Builder(
          builder: (context) {
            seen = LcarsBleedScope.of(context);
            return const SizedBox();
          },
        ),
      ),
    );
    expect(seen, const EdgeInsets.only(top: 24, bottom: 48));
  });

  // The default that keeps every existing test and golden where it is: a widget
  // pumped bare has no scope, so no block bleeds.
  testWidgets('is zero with no scope above', (tester) async {
    late EdgeInsets seen;
    await tester.pumpWidget(
      Builder(
        builder: (context) {
          seen = LcarsBleedScope.of(context);
          return const SizedBox();
        },
      ),
    );
    expect(seen, EdgeInsets.zero);
  });
}
