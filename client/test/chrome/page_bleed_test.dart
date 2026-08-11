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
    // 74 is the rail's identifier block's unbled height (this route cannot
    // pop, so it carries the code rather than a back affordance); growing by
    // exactly the inset on top of it is only true once `_rail` passes the
    // bleed down to it. The cap does not grow off its own unbled height (16)
    // the same way — bled, it swaps to the fixed `kElbowCapBledHeight` instead
    // — so its expected height is that plus the inset, not `16 + 24`.
    expect(tester.getSize(find.byType(ChromeElbow).first).height, 74 + 24);
    expect(
      tester.getSize(find.byType(ChromeElbowCap)).height,
      kElbowCapBledHeight + 24,
    );
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
    // Inside the top inset, well clear of where the fill ends — this only
    // needs to land somewhere the seam is painted at all.
    final seamColour = await pixelAt(tester, Offset(rail.right + 2, 12));

    expect(seamColour, lcarsTokens.primary);
  });

  // Both insets at once, which is what no other test in this file does and is
  // exactly how the defect hid: `buildPage` hands `_railGutter` the frame's
  // whole bleed, so a seam sized off `bleed.vertical` ran the bottom inset
  // past the cap it is supposed to end level with. On a gesture-nav Pixel 8a
  // that painted a 24dp amber tab hanging out of the band's underside.
  testWidgets('the seam fill ends level with the cap', (tester) async {
    useInsets(tester, top: 24, bottom: 48);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    // Two pixels inboard of the rail is inside the 5px gutter the seam fills.
    final seamX = tester.getRect(find.byType(ChromeElbow).first).right + 2;
    final capBottom = tester.getRect(find.byType(ChromeElbowCap)).bottom;

    expect(
      await pixelAt(tester, Offset(seamX, capBottom - 1)),
      lcarsTokens.nav,
    );
    expect(
      await pixelAt(tester, Offset(seamX, capBottom + 1)),
      lcarsTokens.canvas,
      reason: 'the seam must not outlive the cap it continues into',
    );
  });

  // A rounded corner exists to curve the bracket into the canvas. Once a block
  // grows to the bezel that corner faces the screen edge instead, and the
  // radius bites a quarter-circle out of the screen's own corner — measured on
  // a Pixel 8a, the band only reached x=0 at y=84, a 32dp black wedge above
  // the rail and another below it.
  testWidgets('the bracket squares the corners it bleeds into', (tester) async {
    useInsets(tester, top: 24, bottom: 48);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    expect(await pixelAt(tester, const Offset(0.5, 0.5)), lcarsTokens.nav);
    expect(
      await pixelAt(tester, Offset(0.5, surfaceHeight(tester) - 0.5)),
      lcarsTokens.nav,
    );
  });

  // The other half of that rule, and the reason it is keyed to the bleed rather
  // than applied outright: with nothing to bleed into, the elbow keeps the
  // radius that makes the column read as a bracket — the shape every desktop
  // and tablet golden pins.
  testWidgets('an unbled bracket keeps its rounded corners', (tester) async {
    useInsets(tester);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    expect(await pixelAt(tester, const Offset(0.5, 0.5)), lcarsTokens.canvas);
    expect(
      await pixelAt(tester, Offset(0.5, surfaceHeight(tester) - 0.5)),
      lcarsTokens.canvas,
    );
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
