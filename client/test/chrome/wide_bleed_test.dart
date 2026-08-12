import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/chrome/chrome_wide.dart';
import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/ink.dart';
import '../support/insets.dart';

/// The wide LCARS frame's safe-area treatment.
///
/// It is a *different* frame from the phone shell — three columns rather than
/// two, and its right-hand workspace opens with a plain text header instead of
/// an elbow cap. So the band runs across the nav and fleet columns only, and
/// the workspace holds its insets like a pushed page's body does.
///
/// The three-column layout needs a surface past `kLcarsThreeColumnWidth`
/// (1180dp), which no phone reaches — it is the desktop shape, where the insets
/// are zero and every assertion here degenerates to today's layout. The folded
/// two-column shape is the one a phone in landscape actually gets.
void main() {
  /// Sizes the surface *before* [useInsets] pins the ratio, which is the only
  /// order that works: `useInsets` derives the logical size from whatever
  /// `physicalSize / devicePixelRatio` is when it runs.
  void useSurface(WidgetTester tester, double width) {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = Size(width, 900);
  }

  Widget shell() => RepaintBoundary(
    key: inkBoundary,
    child: MaterialApp(
      theme: themeDataFor(lcarsTokens),
      home: ChromeWide(
        ChromeWideSpec(
          fleetList: const SizedBox.expand(key: Key('fleet-list')),
          workspace: const SizedBox.expand(key: Key('workspace')),
          modes: [
            ChromeNavItem(
              label: 'Fleet',
              glyph: '▤',
              selected: true,
              onTap: () {},
            ),
            ChromeNavItem(
              label: 'Activity',
              glyph: '≋',
              selected: false,
              onTap: () {},
            ),
          ],
          needsInputCount: 0,
          activeCount: 3,
          totalCount: 9,
          serverCount: 2,
          newSession: ChromeButtonAction(
            label: 'New session',
            icon: Icons.add,
            onPressed: () {},
          ),
          settings: ChromeButtonAction(
            label: 'Settings',
            icon: Icons.settings,
            onPressed: () {},
          ),
        ),
      ),
    ),
  );

  Future<void> pump(
    WidgetTester tester, {
    required double width,
    double top = 0,
    double bottom = 0,
    double left = 0,
    double right = 0,
  }) async {
    useSurface(tester, width);
    useInsets(tester, top: top, bottom: bottom, left: left, right: right);
    await tester.pumpWidget(shell());
    await tester.pumpAndSettle();
  }

  group('three columns', () {
    testWidgets('the nav rail and the fleet cap meet the physical top edge', (
      tester,
    ) async {
      await pump(tester, width: 1400, top: 24);

      expect(tester.getRect(find.byType(ChromeElbow).first).top, 0);
      expect(tester.getRect(find.byType(ChromeElbowCap)).top, 0);
      // 74 is the CMDR block's unbled height; the cap swaps to the fixed bled
      // height rather than growing off its own 16, exactly as the phone frame's
      // do — see `page_bleed_test.dart`.
      expect(tester.getSize(find.byType(ChromeElbow).first).height, 74 + 24);
      expect(
        tester.getSize(find.byType(ChromeElbowCap)).height,
        kElbowCapBledHeight + 24,
      );
    });

    testWidgets('the rail closes on the physical bottom edge', (tester) async {
      await pump(tester, width: 1400, bottom: 48);

      expect(
        tester.getRect(find.byType(ChromeElbow).last).bottom,
        surfaceHeight(tester),
      );
      // The closing block grew, rather than the inert filler above it taking up
      // the slack — 44 is its unbled height.
      expect(tester.getSize(find.byType(ChromeElbow).last).height, 44 + 48);
    });

    // The band has to be *continuous*. The nav and fleet columns both bleed into
    // it with a 5dp gap between them, and leaving that gap open cuts a black
    // slit through the status bar — the same defect the phone frame's rail seam
    // had, measured there on a Pixel 8a as a black column through the clock.
    testWidgets('the band is continuous across the gap between the columns', (
      tester,
    ) async {
      await pump(tester, width: 1400, top: 24);

      final navRight = tester.getRect(find.byType(ChromeElbow).first).right;
      expect(
        await pixelAt(tester, Offset(navRight + 2, 12)),
        lcarsTokens.nav,
        reason: 'the gap between the nav and fleet columns is open',
      );
    });

    testWidgets('the workspace is held off both insets', (tester) async {
      await pump(tester, width: 1400, top: 24, bottom: 48);

      final workspace = tester.getRect(find.byKey(const Key('workspace')));
      // Below the band, not below the raw inset: the band is one bar with a
      // flat bottom edge, and it overhangs the status bar by the cap's own
      // height so the workspace starts where the fleet column's content does.
      expect(workspace.top, 24 + kElbowCapBledHeight);
      expect(workspace.bottom, surfaceHeight(tester) - 48);
    });

    // The band covers the *whole* status bar, not just the columns that have
    // blocks in them. Measured on a Pixel 8a when it did not: the frame asks
    // for dark system icons, correct over a bright band — and the bluetooth,
    // signal and wifi glyphs, which sit at the far right over the workspace
    // column, drew dark on black and vanished. Every pixel sampled from
    // x=2050-2350 came back (0,0,0).
    testWidgets('the band runs the full width of the status bar', (
      tester,
    ) async {
      await pump(tester, width: 1400, top: 24);

      for (final x in [
        4.0,
        surfaceWidth(tester) / 2,
        surfaceWidth(tester) - 4,
      ]) {
        expect(
          await pixelAt(tester, Offset(x, 12)),
          lcarsTokens.nav,
          reason: 'the status bar is not banded at x=$x',
        );
      }
    });

    testWidgets('the workspace runs flush to the right bezel', (tester) async {
      await pump(tester, width: 1400);

      expect(
        tester.getRect(find.byKey(const Key('workspace'))).right,
        surfaceWidth(tester),
      );
    });

    testWidgets('a cutout is held, not bled', (tester) async {
      await pump(tester, width: 1400, left: 20, right: 20);

      expect(tester.getRect(find.byType(ChromeElbow).first).left, 20);
      expect(
        tester.getRect(find.byKey(const Key('workspace'))).right,
        surfaceWidth(tester) - 20,
      );
    });
  });

  // The shape a phone in landscape gets: no nav column, the fleet column's cap
  // carrying the band on its own and the folded nav run closing the frame.
  group('folded', () {
    testWidgets('the fleet cap meets the physical top edge', (tester) async {
      await pump(tester, width: 800, top: 24);

      expect(tester.getRect(find.byType(ChromeElbowCap)).top, 0);
      expect(
        tester.getSize(find.byType(ChromeElbowCap)).height,
        kElbowCapBledHeight + 24,
      );
    });

    testWidgets('the folded nav run reaches the physical bottom edge', (
      tester,
    ) async {
      final settings = find.widgetWithText(ChromeElbow, 'SETTINGS');

      await pump(tester, width: 800);
      final flat = tester.getSize(settings).height;
      final flatLabel = tester.getRect(find.text('SETTINGS')).center.dy;

      await pump(tester, width: 800, bottom: 48);

      expect(tester.getRect(settings).bottom, surfaceHeight(tester));
      // Grown by exactly the inset, rather than merely pushed down. Measured
      // against the unbled render rather than against the block's declared 32,
      // because that height is a *minimum*: the test fonts wrap 'NEW SESSION'
      // onto a second line and every block in the run grows to match.
      expect(tester.getSize(settings).height, flat + 48);
      // And the label held *the safe region*, which against a zero-inset
      // control means moving up by exactly the inset: with nothing inset, the
      // safe region is the whole screen and the label sits 18 above 900; with
      // 48 inset it must sit 18 above 852. Without the matching inner padding
      // it would not move at all, and the label would be in the gesture strip.
      expect(tester.getRect(find.text('SETTINGS')).center.dy, flatLabel - 48);
    });

    testWidgets('the fleet list is held off the gesture strip', (tester) async {
      await pump(tester, width: 800, bottom: 48);

      // The list is a scrollable, not an LCARS block. A folded frame closes on
      // the nav run, so the list's own bottom stays where the safe region put
      // it and only the run below it grows.
      expect(
        tester.getRect(find.byKey(const Key('fleet-list'))).bottom,
        lessThanOrEqualTo(surfaceHeight(tester) - 48),
      );
    });
  });
}
