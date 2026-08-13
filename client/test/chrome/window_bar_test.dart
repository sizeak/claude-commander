import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/chrome/chrome_wide.dart';
import 'package:claude_commander_client/chrome/lcars/bleed.dart';
import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// The borderless window bar, in both chromes.
///
/// Both are exercised by the same tests: the bar replaces the desktop's title
/// bar, so a theme that dropped its close button or stopped reporting drags would
/// leave a window the user cannot move or close.
void main() {
  const tokenSets = {
    'Mission Control': missionControlTokens,
    'LCARS': lcarsTokens,
  };

  Future<void> pumpBar(
    WidgetTester tester,
    CommanderTokens tokens,
    ChromeWindowBarSpec spec,
  ) => tester.pumpWidget(
    MaterialApp(
      theme: themeDataFor(tokens),
      home: Scaffold(body: Column(children: [ChromeWindowBar(spec)])),
    ),
  );

  tokenSets.forEach((name, tokens) {
    group(name, () {
      testWidgets('shows the app name', (tester) async {
        await pumpBar(
          tester,
          tokens,
          ChromeWindowBarSpec(
            title: 'Claude Commander',
            onDragStart: () {},
            onDoubleTap: () {},
            onMinimize: () {},
            onToggleMaximize: () {},
            onClose: () {},
          ),
        );

        // Mission Control captions its bar; LCARS does not. Its leading block
        // is the bracket's corner — a shape, not a caption — and the window's
        // name is already its title in the task switcher.
        expect(
          find.text(tokens.caseLabel('Claude Commander')),
          tokens.chrome == ChromeKind.lcars ? findsNothing : findsOneWidget,
        );
      });

      testWidgets('minimize, maximize and close each fire', (tester) async {
        final fired = <String>[];
        await pumpBar(
          tester,
          tokens,
          ChromeWindowBarSpec(
            title: 'Claude Commander',
            onDragStart: () => fired.add('drag'),
            onDoubleTap: () => fired.add('doubleTap'),
            onMinimize: () => fired.add('minimize'),
            onToggleMaximize: () => fired.add('maximize'),
            onClose: () => fired.add('close'),
          ),
        );

        await tester.tap(find.byTooltip('Minimise'));
        await tester.tap(find.byTooltip('Maximise'));
        await tester.tap(find.byTooltip('Close'));
        await tester.pump();

        expect(fired, ['minimize', 'maximize', 'close']);
      });

      testWidgets('the maximise control reports the restore state', (
        tester,
      ) async {
        await pumpBar(
          tester,
          tokens,
          ChromeWindowBarSpec(
            title: 'Claude Commander',
            isMaximized: true,
            onDragStart: () {},
            onDoubleTap: () {},
            onMinimize: () {},
            onToggleMaximize: () {},
            onClose: () {},
          ),
        );

        expect(find.byTooltip('Restore'), findsOneWidget);
        expect(find.byTooltip('Maximise'), findsNothing);
      });

      testWidgets('dragging the bar starts a window drag', (tester) async {
        final fired = <String>[];
        await pumpBar(
          tester,
          tokens,
          ChromeWindowBarSpec(
            title: 'Claude Commander',
            onDragStart: () => fired.add('drag'),
            onDoubleTap: () => fired.add('doubleTap'),
            onMinimize: () {},
            onToggleMaximize: () {},
            onClose: () {},
          ),
        );

        // The drag surface, not a control: the buttons must stay clickable.
        // Keyed rather than found by what is drawn on it — LCARS draws nothing
        // on its corner block, and the surface is the thing under test anyway.
        await tester.timedDrag(
          find.byKey(windowBarDragSurface),
          const Offset(60, 0),
          const Duration(milliseconds: 200),
        );

        expect(fired, ['drag']);
      });

      testWidgets('double-tapping the bar toggles maximise', (tester) async {
        final fired = <String>[];
        await pumpBar(
          tester,
          tokens,
          ChromeWindowBarSpec(
            title: 'Claude Commander',
            onDragStart: () => fired.add('drag'),
            onDoubleTap: () => fired.add('doubleTap'),
            onMinimize: () {},
            onToggleMaximize: () {},
            onClose: () {},
          ),
        );

        final bar = find.byKey(windowBarDragSurface);
        await tester.tap(bar);
        await tester.pump(const Duration(milliseconds: 50));
        await tester.tap(bar);
        await tester.pumpAndSettle();

        expect(fired, ['doubleTap']);
      });
    });
  });

  testWidgets('the close control is the only danger-tinted one', (
    tester,
  ) async {
    // Mission Control's bar is a row of neutral glyphs; close is the one action
    // that ends the session, so it is the one that reads as destructive.
    await pumpBar(
      tester,
      missionControlTokens,
      ChromeWindowBarSpec(
        title: 'Claude Commander',
        onDragStart: () {},
        onDoubleTap: () {},
        onMinimize: () {},
        onToggleMaximize: () {},
        onClose: () {},
      ),
    );

    Color? colourOf(String tooltip) => tester
        .widget<IconButton>(
          find.ancestor(
            of: find.byTooltip(tooltip),
            matching: find.byType(IconButton),
          ),
        )
        .color;

    expect(colourOf('Close'), missionControlTokens.danger);
    expect(colourOf('Minimise'), isNot(missionControlTokens.danger));
  });

  // The LCARS bar is the top of the frame's bracket, not a separate run above
  // it. Its name cap carries the identity colour and the elbow radius, so the
  // bracket's corner is the *window's* corner — with the bar sitting above a
  // rail that drew its own 32px corner, the bracket appeared to start
  // mid-window. The design deck cannot settle this on its own: its laptop frame
  // is drawn as a bare screen with no title bar at all.
  group('LCARS bracket', () {
    Future<void> pumpLcars(WidgetTester tester) => pumpBar(
      tester,
      lcarsTokens,
      ChromeWindowBarSpec(
        title: 'Claude Commander',
        onDragStart: () {},
        onDoubleTap: () {},
        onMinimize: () {},
        onToggleMaximize: () {},
        onClose: () {},
      ),
    );

    /// The bar's leading block — the corner itself. Found by position rather
    /// than by a label, because it deliberately has none.
    Finder corner() => find.byType(ChromeElbow).first;

    testWidgets('the corner block is the frame identity, in amber', (
      tester,
    ) async {
      await pumpLcars(tester);

      expect(tester.widget<ChromeElbow>(corner()).color, lcarsTokens.primary);
      expect(tester.widget<ChromeElbow>(corner()).label, isNull);
    });

    // Drawn to the nav column's width, so the amber runs straight down into the
    // block below rather than stepping sideways a row in.
    testWidgets('the corner block is the width of the column below it', (
      tester,
    ) async {
      await pumpLcars(tester);

      expect(tester.getSize(corner()).width, kLcarsNavWidth);
    });

    testWidgets('the corner turns the elbow, and only at the top', (
      tester,
    ) async {
      await pumpLcars(tester);

      final clip = tester.widget<ClipRRect>(
        find.ancestor(of: corner(), matching: find.byType(ClipRRect)).first,
      );
      final radius = clip.borderRadius as BorderRadius;
      expect(radius.topLeft, Radius.circular(lcarsTokens.elbowRadius));
      // Square below, so the bracket flows down into the rail rather than
      // closing itself off as a free-standing pill.
      expect(radius.bottomLeft, Radius.zero);
    });

    // The trailing edge is the window's, and the frame runs flush to it
    // everywhere else too — a pill there left the run stopping short of its own
    // corner.
    testWidgets('the closing control is square against the window edge', (
      tester,
    ) async {
      await pumpLcars(tester);

      final clip = tester.widget<ClipRRect>(
        find
            .ancestor(
              of: find.widgetWithText(ChromeElbow, 'CLOSE'),
              matching: find.byType(ClipRRect),
            )
            .first,
      );
      expect(clip.borderRadius, BorderRadius.zero);
    });

    // The corner runs the full height of the bar while every other block
    // leaves the seam under it. The block directly below the corner is the
    // rail's opening block — same colour, same width — so a gap there splits
    // one amber column head in two, where the same gap under the dark drag
    // filler is the run's own rhythm.
    testWidgets('the corner alone carries no seam beneath it', (tester) async {
      await pumpLcars(tester);

      final bar = tester.getRect(find.byType(ChromeWindowBar));
      expect(tester.getRect(corner()).bottom, bar.bottom);
      expect(
        tester.getRect(find.widgetWithText(ChromeElbow, 'CLOSE')).bottom,
        bar.bottom - 4,
        reason: 'the controls keep the seam',
      );
    });

    // The other half of the deal, and the only thing that holds it: the bar
    // turning the corner is pointless if the rail turns it again 4px below.
    testWidgets('the rail beneath does not turn the corner again', (
      tester,
    ) async {
      Future<ElbowCorner> cornerUnder({required bool bar}) async {
        final rail = ChromeViewRail(
          ChromeViewRailSpec(
            code: '47-A',
            title: 'Fleet',
            body: const SizedBox.expand(),
          ),
        );
        await tester.pumpWidget(
          MaterialApp(
            theme: themeDataFor(lcarsTokens),
            home: Scaffold(
              body: bar
                  ? LcarsCornerScope(takenAbove: true, child: rail)
                  : rail,
            ),
          ),
        );
        await tester.pumpAndSettle();
        return tester
            .widget<ChromeElbow>(find.byType(ChromeElbow).first)
            .corner;
      }

      expect(await cornerUnder(bar: false), ElbowCorner.topLeft);
      expect(await cornerUnder(bar: true), ElbowCorner.none);
    });
  });
}
