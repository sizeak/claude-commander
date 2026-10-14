import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/golden.dart';
import '../support/ink.dart';

/// The LCARS footer's centre (create) block, measured in **painted pixels** —
/// see [inkCentre] for what that buys and which defect it pins. The bled twin
/// of this measurement lives in `phone_shell_test.dart`, against the same
/// helper.
///
/// This test is also the receipt for the claim that a Material `Icon` centres
/// its glyph in its box — nothing in this repo can assert that about the icon
/// font except a measurement of what it actually paints.
void main() {
  Future<void> pumpFooter(WidgetTester tester) => pumpGolden(
    tester,
    tokens: lcarsTokens,
    size: const Size(300, 60),
    child: RepaintBoundary(
      key: inkBoundary,
      child: ChromeFooterNav(
        ChromeFooterNavSpec(
          items: [
            ChromeNavItem(
              label: 'FLEET',
              glyph: '▤',
              selected: true,
              onTap: () {},
            ),
            ChromeNavItem(
              label: 'ACTIVITY',
              glyph: '≋',
              selected: false,
              onTap: () {},
            ),
          ],
          centreAction: ChromeButtonAction(
            icon: Icons.add,
            label: 'New session',
            onPressed: () {},
          ),
        ),
      ),
    ),
  );

  testWidgets('the create block draws its glyph centred', (tester) async {
    await pumpFooter(tester);
    // Without the icon face an `Icon` is a notdef square, and a square's ink is
    // centred on its box whatever the padding — the vertical half of this test
    // would then pass on a glyph that was never drawn.
    expect(
      materialIconsLoaded,
      isTrue,
      reason: 'MaterialIcons did not load; this test cannot measure an icon',
    );

    // Three contiguous blocks — FLEET, the create action, ACTIVITY — so the
    // centre one is the middle `ChromeElbow`. It is also the only block of the
    // run with no rounded corner, which is what lets the scan below treat every
    // pixel of its rect as either fill or ink.
    final blocks = find.byType(ChromeElbow);
    expect(blocks, findsNWidgets(3));
    final rect = tester.getRect(blocks.at(1));

    final centre = await inkCentre(
      tester,
      // Inset by a pixel: the block's own edges may land on a fraction and
      // antialias against the seam either side of it.
      rect.deflate(1),
      lcarsTokens.attention,
    );

    final expected = rect.center;
    expect(
      (centre.dx - expected.dx).abs(),
      lessThan(1),
      reason: 'glyph is off-centre horizontally: $centre vs $expected',
    );
    expect(
      (centre.dy - expected.dy).abs(),
      lessThan(1),
      reason: 'glyph is off-centre vertically: $centre vs $expected',
    );
  });

  testWidgets('the create block announces the action, not its glyph', (
    tester,
  ) async {
    // Disposed inline rather than in a tearDown: the binding checks for leaked
    // handles *before* tearDowns run, so an addTearDown here fails the test.
    final semantics = tester.ensureSemantics();
    await pumpFooter(tester);

    // The block itself has no readable label — it never did, and '+' was a
    // worse one than none. The action's own label comes from the wrapper.
    expect(
      tester.getSemantics(find.byType(ChromeElbow).at(1)),
      isSemantics(label: 'New session', isButton: true),
    );
    semantics.dispose();
  });
}
