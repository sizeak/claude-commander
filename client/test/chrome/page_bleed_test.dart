import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/ink.dart';
import '../support/insets.dart';

/// A pushed LCARS route has no footer, so its bottom row is the rail's closing
/// elbow beside the body. The elbow bleeds; the body must not — a scrollable
/// running under the gesture strip is a regression, not a feature.
void main() {
  // Keyed with [inkBoundary] so `pixelAt` (used by the seam-colour test below)
  // can rasterise the tree; a `RepaintBoundary` changes no geometry, so this
  // is a no-op for every other test in the file.
  Widget page(ChromeInsets insets, {bool showBack = false}) => RepaintBoundary(
    key: inkBoundary,
    child: MaterialApp(
      theme: themeDataFor(lcarsTokens),
      home: ChromePage(
        code: '47-B',
        title: 'Detail',
        insets: insets,
        showBack: showBack,
        // Keyed, not found by type: `ColoredBox` and `SizedBox` both occur all
        // over a built page, so a type finder here would silently measure
        // something else.
        body: const SizedBox.expand(key: Key('page-body')),
      ),
    ),
  );

  testWidgets('the rail closes on the physical bottom edge', (tester) async {
    useInsets(tester, bottom: 48);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    expect(
      tester.getRect(find.byType(ChromeElbow).last).bottom,
      surfaceHeight(tester),
    );
    // And it is the elbow that grew, not the inert filler above it. That edge
    // is the only thing the bottom bleed can be read off: 44 is the closing
    // block's unbled height, and without the bleed reaching it the filler would
    // take up the slack instead, leaving the corner's top seam 48 lower than
    // the safe region had it while this rect's bottom stayed put.
    expect(tester.getSize(find.byType(ChromeElbow).last).height, 44 + 48);
  });

  testWidgets('the body is held off the gesture strip', (tester) async {
    useInsets(tester, bottom: 48);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    expect(
      tester.getRect(find.byKey(const Key('page-body'))).bottom,
      surfaceHeight(tester) - 48,
    );
  });

  testWidgets('the rail and the cap meet the physical top edge', (
    tester,
  ) async {
    useInsets(tester, top: 24);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    expect(tester.getRect(find.byType(ChromeElbow).first).top, 0);
    expect(tester.getRect(find.byType(ChromeElbowCap)).top, 0);
    // 74 and 16 are their unbled heights — the rail's identifier block (this
    // route cannot pop, so it carries the code rather than a back affordance)
    // and the content column's cap. Growing by exactly the inset is only true
    // once `_rail`/`_content` pass the bleed down to them.
    expect(tester.getSize(find.byType(ChromeElbow).first).height, 74 + 24);
    expect(tester.getSize(find.byType(ChromeElbowCap)).height, 16 + 24);
  });

  // "Extend the fill, hold the label": the identifier is bottom-aligned in its
  // block, so growing the block by the status-bar inset must leave the text
  // exactly where the safe region had it — one inset lower on the screen than
  // an unbled render, not two.
  testWidgets('the identifier holds the safe region', (tester) async {
    useInsets(tester);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();
    final flat = tester.getRect(find.text('47-B')).center.dy;

    useInsets(tester, top: 24);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    expect(tester.getRect(find.text('47-B')).center.dy, flat + 24);
  });

  // Regression guard: the seam's fill colour tracks the back affordance, not
  // a flat `t.nav`. A pushed page with a back button paints its rail's top
  // block and cap `t.primary` (the same lilac-vs-amber distinction as the
  // '‹ BACK' block itself), so the seam bridging them must match or a lilac
  // block would sit against an amber seam and cap.
  testWidgets('the seam fill matches the back affordance\'s colour', (
    tester,
  ) async {
    useInsets(tester, top: 24);
    await tester.pumpWidget(page(ChromeInsets.standard, showBack: true));
    await tester.pumpAndSettle();

    final rail = tester.getRect(find.widgetWithText(ChromeElbow, '‹ BACK'));
    // Inside the top inset, well clear of the curved emergence the seam
    // draws below its fill (see `phone_shell_test.dart`'s gutter-curve test)
    // — this only needs to land somewhere the fill is flat.
    final seamColour = await pixelAt(tester, Offset(rail.right + 2, 12));

    expect(seamColour, lcarsTokens.primary);
  });

  testWidgets('a cutout is held, not bled', (tester) async {
    useInsets(tester, left: 20, right: 20);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    final rail = tester.getRect(find.byType(ChromeElbow).first);
    expect(rail.left, 20);
    // Held: the block starts inboard of the cutout at its ordinary width, rather
    // than being widened to paint under it.
    expect(rail.width, lcarsTokens.railWidth);
  });

  // The terminal's exemption. `pan` already wraps the whole row in a SafeArea
  // (`chrome.dart:224`), so a block that also bled would be offset twice. Both
  // expectations hold before the bleed exists as well as after — they are the
  // guard on the exemption, not a red-green pair.
  testWidgets('a pan page does not bleed', (tester) async {
    useInsets(tester, bottom: 48);
    await tester.pumpWidget(page(ChromeInsets.pan));
    await tester.pumpAndSettle();

    expect(
      tester.getRect(find.byType(ChromeElbow).last).bottom,
      surfaceHeight(tester) - 48,
    );
    // The elbow above cannot see a double offset on its own — it would simply
    // grow into the slack the filler gives up — but the body can: a page that
    // both sat inside the `SafeArea` and held the inset again would end 96 short.
    expect(
      tester.getRect(find.byKey(const Key('page-body'))).bottom,
      surfaceHeight(tester) - 48,
    );
  });
}
