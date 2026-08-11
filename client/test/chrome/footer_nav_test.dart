import 'dart:ui' as ui;

import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/golden.dart';

/// The LCARS footer's centre (create) block, measured in **painted pixels**.
///
/// Pixels rather than widget boxes, because the defect this pins was invisible
/// to box geometry. The block used to draw a `Text('+')`, and a text glyph
/// centres its *line box*, not its ink: '+' sits above the baseline, so the box
/// could be dead centre while the cross painted ~2px low. The horizontal half of
/// the same bug came from [ChromeElbow]'s inboard padding bias (6 left, 9
/// right), which is right for a rail's right-aligned labels and wrong for a
/// centred one.
///
/// This test is also the receipt for the claim that a Material `Icon` centres
/// its glyph in its box — nothing in this repo can assert that about the icon
/// font except a measurement of what it actually paints.
void main() {
  const boundary = ValueKey('footer-boundary');

  /// The ink centroid of [rect] (in the surface's logical pixels), weighted by
  /// how far each pixel is from [fill].
  ///
  /// A centroid rather than a bounding box: both are exact for a symmetric
  /// glyph, but antialiased edge pixels perturb a bbox asymmetrically and cancel
  /// out in a weighted mean.
  Future<Offset> inkCentre(WidgetTester tester, Rect rect, Color fill) async {
    final image = await tester.runAsync(
      () => (tester.renderObject(find.byKey(boundary)) as RenderRepaintBoundary)
          .toImage(),
    );
    final data = await tester.runAsync(
      () => image!.toByteData(format: ui.ImageByteFormat.rawRgba),
    );
    addTearDown(image!.dispose);

    var weight = 0.0, sx = 0.0, sy = 0.0;
    for (var y = rect.top.ceil(); y < rect.bottom.floor(); y++) {
      for (var x = rect.left.ceil(); x < rect.right.floor(); x++) {
        final i = (y * image.width + x) * 4;
        final w =
            ((data!.getUint8(i) - (fill.r * 255)).abs() +
                (data.getUint8(i + 1) - (fill.g * 255)).abs() +
                (data.getUint8(i + 2) - (fill.b * 255)).abs()) /
            765;
        weight += w;
        sx += w * (x + 0.5);
        sy += w * (y + 0.5);
      }
    }
    expect(weight, greaterThan(1), reason: 'the block painted no glyph at all');
    return Offset(sx / weight, sy / weight);
  }

  Future<void> pumpFooter(WidgetTester tester) => pumpGolden(
    tester,
    tokens: lcarsTokens,
    size: const Size(300, 60),
    child: RepaintBoundary(
      key: boundary,
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
    final origin = tester.getTopLeft(find.byKey(boundary));

    final centre = await inkCentre(
      tester,
      // Inset by a pixel: the block's own edges may land on a fraction and
      // antialias against the seam either side of it.
      rect.shift(-origin).deflate(1),
      lcarsTokens.attention,
    );

    final expected = rect.shift(-origin).center;
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
