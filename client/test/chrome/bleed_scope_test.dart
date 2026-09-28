import 'package:claude_commander_client/chrome/lcars/bleed.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// What [_Probe] last read. A file-level variable because the probe has to be a
/// `const` widget — see [_Probe].
late EdgeInsets probed;

/// A dependent the enclosing scope **cannot** rebuild.
///
/// Pumping `const _Probe()` twice hands the element the identical widget, so
/// nothing above it can mark it dirty; a rebuild can then only have come from
/// the inherited dependency. A `Builder` would not do — its closure is a new
/// object on every pump, which rebuilds the child whatever the scope decided.
class _Probe extends StatelessWidget {
  const _Probe();

  @override
  Widget build(BuildContext context) {
    probed = LcarsBleedScope.of(context);
    return const SizedBox();
  }
}

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

  // `updateShouldNotify` is the whole reason this is an inherited widget rather
  // than a one-shot read: the app republishes the scope when the device rotates
  // or the keyboard collapses the bottom padding, and every block below it has
  // to be rebuilt with the new number. A scope that never notified would still
  // satisfy the first test, which only reads the value once.
  testWidgets('rebuilds its dependents when the bleed changes', (tester) async {
    Future<void> pump(EdgeInsets bleed) =>
        tester.pumpWidget(LcarsBleedScope(bleed: bleed, child: const _Probe()));

    await pump(const EdgeInsets.only(bottom: 48));
    await pump(const EdgeInsets.only(bottom: 24));

    expect(probed, const EdgeInsets.only(bottom: 24));
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
