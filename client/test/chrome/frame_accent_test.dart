import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/chrome/chrome_wide.dart';
import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// The colour the LCARS frame's top run takes.
///
/// The design deck paints it **per view**, not per theme: the identifier block
/// that opens the rail and the cap that closes it across the content column are
/// `#f7a01d` (amber, [CommanderTokens.primary]) on Fleet and `#cc99cc` (lilac,
/// [CommanderTokens.nav]) on Activity. Five frames say amber — 4a's portrait
/// Fleet (h74 elbow and h16 cap, both `#f7a01d`) and 4b's L1, L2 and L3 (h92/h80
/// elbows and h18/h16 fleet caps, all `#f7a01d`) — against two that say lilac,
/// 4a's portrait Activity and 4b's L4, both `#cc99cc`.
///
/// [ChromeViewRailStyle] already draws that exact line: `branded` is Fleet,
/// `plain` is Activity. So the accent follows the style rather than needing a
/// field of its own.
void main() {
  Widget viewRail(ChromeViewRailStyle style) => MaterialApp(
    theme: themeDataFor(lcarsTokens),
    home: Scaffold(
      body: ChromeViewRail(
        ChromeViewRailSpec(
          code: '47-A',
          title: 'Fleet',
          style: style,
          body: const SizedBox.expand(),
        ),
      ),
    ),
  );

  /// Past `kLcarsThreeColumnWidth`, so the nav column exists to be measured —
  /// below it the frame folds and there is no identifier block at all.
  void useThreeColumns(WidgetTester tester) {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1400, 900);
    addTearDown(tester.view.reset);
  }

  Widget wide(ChromeViewRailStyle style) => MaterialApp(
    theme: themeDataFor(lcarsTokens),
    home: ChromeWide(
      ChromeWideSpec(
        style: style,
        fleetList: const SizedBox.expand(),
        workspace: const SizedBox.expand(),
        modes: [
          ChromeNavItem(
            label: 'Fleet',
            glyph: '▤',
            selected: true,
            onTap: () {},
          ),
        ],
        needsInputCount: 0,
        activeCount: 0,
        totalCount: 0,
        serverCount: 1,
      ),
    ),
  );

  Color capColour(WidgetTester tester, [int at = 0]) => tester
      .widgetList<ChromeElbowCap>(find.byType(ChromeElbowCap))
      .elementAt(at)
      .color;

  Color firstBlockColour(WidgetTester tester) =>
      tester.widget<ChromeElbow>(find.byType(ChromeElbow).first).color;

  group('the view rail', () {
    testWidgets('Fleet opens the frame in amber', (tester) async {
      await tester.pumpWidget(viewRail(ChromeViewRailStyle.branded));
      await tester.pumpAndSettle();

      expect(firstBlockColour(tester), lcarsTokens.primary);
      expect(capColour(tester), lcarsTokens.primary);
    });

    testWidgets('Activity opens it in lilac', (tester) async {
      await tester.pumpWidget(viewRail(ChromeViewRailStyle.plain));
      await tester.pumpAndSettle();

      expect(firstBlockColour(tester), lcarsTokens.nav);
      expect(capColour(tester), lcarsTokens.nav);
    });
  });

  group('the wide frame', () {
    testWidgets('Fleet opens every column in amber', (tester) async {
      useThreeColumns(tester);
      await tester.pumpWidget(wide(ChromeViewRailStyle.branded));
      await tester.pumpAndSettle();

      // The nav column's identifier block, then the fleet and workspace caps.
      expect(firstBlockColour(tester), lcarsTokens.primary);
      expect(capColour(tester, 0), lcarsTokens.primary);
      expect(capColour(tester, 1), lcarsTokens.primary);
    });

    testWidgets('Activity opens them in lilac', (tester) async {
      useThreeColumns(tester);
      await tester.pumpWidget(wide(ChromeViewRailStyle.plain));
      await tester.pumpAndSettle();

      expect(firstBlockColour(tester), lcarsTokens.nav);
      expect(capColour(tester, 0), lcarsTokens.nav);
      expect(capColour(tester, 1), lcarsTokens.nav);
    });
  });

  // The other half of the rail/run split. A rail marks its selected block lilac
  // and keeps amber for the identifier block that opens the bracket — 4b L1's
  // FLEET (`#cc99cc`) against LOG (`#3a2f45`), and the portrait rail's h30/h22
  // pair with ALL selected. A *run* inverts it: the phone footer's selected
  // block is `#f7a01d`, which is why `buildFooterNav` is untouched.
  testWidgets('a rail marks its selected block lilac, not amber', (
    tester,
  ) async {
    useThreeColumns(tester);
    await tester.pumpWidget(wide(ChromeViewRailStyle.branded));
    await tester.pumpAndSettle();

    final fleet = tester.widget<ChromeElbow>(
      find.widgetWithText(ChromeElbow, 'FLEET'),
    );
    expect(fleet.color, lcarsTokens.nav);
  });

  // The deck's rails step from a thin bright band into a large dark one, and the
  // bright one is `#5c4a6b` — [CommanderTokens.border], the brightest of the
  // three inert fills. Both the portrait rail (h16) and the landscape nav column
  // (h22) use it. The implementation reached one step too far down the ramp for
  // [CommanderTokens.borderSubtle] (`#3a2f45`), which is the *unselected block*
  // colour and reads as a nav block rather than as filler.
  testWidgets('the rails step down through the brightest inert fill', (
    tester,
  ) async {
    await tester.pumpWidget(viewRail(ChromeViewRailStyle.branded));
    await tester.pumpAndSettle();

    final fills = tester
        .widgetList<ChromeElbow>(find.byType(ChromeElbow))
        .map((e) => e.color)
        .toList();
    expect(fills, contains(lcarsTokens.border));
  });
}
