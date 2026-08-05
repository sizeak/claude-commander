import 'package:claude_commander_client/chrome/chrome_forms.dart';
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

        // Uppercased under LCARS, so match on the cased form the theme asks for
        // rather than pinning one spelling.
        expect(find.text(tokens.caseLabel('Claude Commander')), findsOneWidget);
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

        // Drag the title area, not a control: the buttons must stay clickable.
        await tester.timedDrag(
          find.text(tokens.caseLabel('Claude Commander')),
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

        final bar = find.text(tokens.caseLabel('Claude Commander'));
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
}
