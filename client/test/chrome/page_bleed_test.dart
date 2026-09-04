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
  // Keyed with [inkBoundary] so `pixelAt` (used by the pixel tests below)
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

  // The gutter runs open for the frame's whole height, the status-bar band
  // included. It was briefly filled across the band — the fill kept the black
  // out of the system clock, which sat right on the seam on a Pixel 8a — but
  // a filled gutter is what forced the cap to give up its bottom-left radius,
  // and the resulting bare 90° junction between band and rail is worse than
  // the notch. The notch is the accepted cost of the elbow; this pins that the
  // fill does not come back without the trade being re-argued.
  testWidgets('the rail/content gutter runs open through the band', (
    tester,
  ) async {
    useInsets(tester, top: 24);
    await tester.pumpWidget(page(ChromeInsets.standard, showBack: true));
    await tester.pumpAndSettle();

    // Two pixels inboard of the rail is inside the 5dp gutter, and y=12 is
    // inside the status-bar inset — the one place a fill would show.
    final rail = tester.getRect(find.widgetWithText(ChromeElbow, '‹ BACK'));

    expect(
      await pixelAt(tester, Offset(rail.right + 2, 12)),
      lcarsTokens.canvas,
    );
  });

  // Both insets at once, which is what no other test in this file does. Note
  // what this can and cannot see: it pins that *the frame* hands its cap a
  // top-only bleed, not that the cap ignores a bottom one — `_content` filters
  // the bleed before the cap ever sees it, so mutating the cap's own
  // `bleed.top` to `bleed.vertical` leaves this green. That rule is pinned
  // where it is visible, on the widget itself, by 'ignores a bottom inset
  // entirely' in `elbow_bleed_test.dart`.
  testWidgets('the frame hands its cap a top-only bleed', (tester) async {
    useInsets(tester, top: 24, bottom: 48);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    expect(
      tester.getSize(find.byType(ChromeElbowCap)).height,
      kElbowCapBledHeight + 24,
    );
  });

  // The elbow the whole un-merge exists for: the cap's bottom-left curves away
  // from the rail, so the bracket turns its corner on an arc rather than a bare
  // right angle. Sampled a pixel in from the cap's own bottom-left, which the
  // radius has carved back to canvas, against one the same distance in from its
  // *bottom-right*, which it has not.
  testWidgets('a bled cap curves out of the rail', (tester) async {
    useInsets(tester, top: 24);
    await tester.pumpWidget(page(ChromeInsets.standard));
    await tester.pumpAndSettle();

    final cap = tester.getRect(find.byType(ChromeElbowCap));

    expect(
      await pixelAt(tester, Offset(cap.left + 1, cap.bottom - 1)),
      lcarsTokens.canvas,
      reason: 'the bled cap squared its bottom-left corner',
    );
    expect(
      await pixelAt(tester, Offset(cap.right - 1, cap.bottom - 1)),
      lcarsTokens.nav,
      reason: 'only the bottom-left corner is rounded',
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
