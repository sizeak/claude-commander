import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:claude_commander_client/util/colour_hex.dart';
import 'package:claude_commander_client/widgets/colour_dialog.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// The swatch + hex + paste colour dialog the theme picker's per-role rows
/// open. These cases moved here with the dialog when per-workspace colours gave
/// way to per-workspace themes.
void main() {
  late ColourChoice? result;
  late bool popped;

  setUp(() {
    result = null;
    popped = false;
  });

  Future<void> open(
    WidgetTester tester,
    CommanderTokens tokens, {
    Color? current,
  }) async {
    tester.view.physicalSize = const Size(420, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      MaterialApp(
        theme: themeDataFor(tokens),
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await showColourDialog(
                context,
                title: 'Primary',
                current: current,
              );
              popped = true;
            },
            child: const Text('open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
  }

  Finder hexField() => find.byKey(const ValueKey('colour-hex-field'));

  bool saveEnabled(WidgetTester tester) =>
      tester
          .widget<FilledButton>(find.widgetWithText(FilledButton, 'Save'))
          .onPressed !=
      null;

  Future<void> save(WidgetTester tester) async {
    await tester.tap(find.widgetWithText(FilledButton, 'Save'));
    await tester.pumpAndSettle();
  }

  // Each theme reuses its primary for one other role; that role's swatch must
  // be dropped and the shared hex offered once, as Primary.
  for (final (label, tokens, duplicate, duplicateColor) in [
    ('Mission Control', missionControlTokens, 'Nav', missionControlTokens.nav),
    ('LCARS', lcarsTokens, 'Working', lcarsTokens.working),
  ]) {
    testWidgets('offers the $label theme colours, deduplicated', (
      tester,
    ) async {
      final shared = hexOf(tokens.primary);
      expect(
        hexOf(duplicateColor),
        shared,
        reason: '$label must reuse its primary as $duplicate for this test',
      );

      await open(tester, tokens);

      Finder tooltipStarting(String prefix) => find.byWidgetPredicate(
        (w) => w is Tooltip && (w.message ?? '').startsWith(prefix),
      );
      expect(tooltipStarting('$duplicate · '), findsNothing);
      expect(find.byKey(ValueKey('swatch-$shared')), findsOneWidget);
      expect(find.byTooltip('Primary · $shared'), findsOneWidget);
      for (final s in themeSwatches(tokens)) {
        expect(find.byTooltip('${s.name} · ${s.hex}'), findsOneWidget);
      }
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('swatches can come from a token set other than the app\'s', (
    tester,
  ) async {
    // The picker edits a scope that may not be the one the app is in, so it
    // offers that scope's palette.
    tester.view.physicalSize = const Size(420, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      MaterialApp(
        theme: themeDataFor(missionControlTokens),
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () => showColourDialog(
              context,
              title: 'Primary',
              swatchesFrom: lcarsTokens,
            ),
            child: const Text('open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
    expect(
      find.byKey(ValueKey('swatch-${hexOf(lcarsTokens.primary)}')),
      findsOneWidget,
    );
    expect(
      find.byKey(ValueKey('swatch-${hexOf(missionControlTokens.primary)}')),
      findsNothing,
    );
  });

  testWidgets('picking a swatch names it, then Save returns its colour', (
    tester,
  ) async {
    await open(tester, missionControlTokens);
    final success = hexOf(missionControlTokens.success);
    await tester.tap(find.byKey(ValueKey('swatch-$success')));
    await tester.pumpAndSettle();
    expect(find.text('Success · $success'), findsOneWidget);
    expect(tester.widget<TextField>(hexField()).controller!.text, success);
    await save(tester);
    expect(result!.color, missionControlTokens.success);
  });

  testWidgets('Reset returns a choice with no colour', (tester) async {
    await open(tester, missionControlTokens, current: const Color(0xFF123456));
    await tester.tap(find.text('Reset'));
    await tester.pumpAndSettle();
    expect(result, isNotNull);
    expect(result!.color, isNull);
  });

  testWidgets('Cancel returns nothing', (tester) async {
    await open(tester, missionControlTokens);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(popped, isTrue);
    expect(result, isNull);
  });

  testWidgets('a typed hex, with or without #, is accepted in any case', (
    tester,
  ) async {
    await open(tester, missionControlTokens);
    await tester.enterText(hexField(), '  12AB9F ');
    await tester.pumpAndSettle();
    expect(find.text('Custom · #12ab9f'), findsOneWidget);
    expect(saveEnabled(tester), isTrue);
    await save(tester);
    expect(result!.color, const Color(0xFF12AB9F));
  });

  testWidgets('the paste button fills the field from the clipboard', (
    tester,
  ) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async => call.method == 'Clipboard.getData'
          ? <String, dynamic>{'text': ' #C0FFEE\n'}
          : null,
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await open(tester, missionControlTokens);
    await tester.tap(find.byTooltip('Paste'));
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(hexField()).controller!.text, '#C0FFEE');
    await save(tester);
    expect(result!.color, const Color(0xFFC0FFEE));
  });

  testWidgets('an invalid hex shows an error and disables Save', (
    tester,
  ) async {
    await open(tester, missionControlTokens);
    for (final bad in ['#ff88', '#gg8800', 'red', '#ff88001']) {
      await tester.enterText(hexField(), bad);
      await tester.pumpAndSettle();
      expect(
        find.textContaining('is not a #rrggbb colour'),
        findsOneWidget,
        reason: bad,
      );
      expect(saveEnabled(tester), isFalse, reason: bad);
    }
    // An empty field is not an error yet, but there is nothing to save.
    await tester.enterText(hexField(), '');
    await tester.pumpAndSettle();
    expect(find.textContaining('is not a #rrggbb colour'), findsNothing);
    expect(saveEnabled(tester), isFalse);
  });

  testWidgets('the current colour is prefilled and its swatch selected', (
    tester,
  ) async {
    await open(tester, lcarsTokens, current: lcarsTokens.nav);
    final nav = hexOf(lcarsTokens.nav);
    expect(tester.widget<TextField>(hexField()).controller!.text, nav);
    expect(find.text('Nav · $nav'), findsOneWidget);
    expect(saveEnabled(tester), isTrue);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();

    // A colour that is no theme token is still prefilled, as Custom.
    await open(tester, lcarsTokens, current: const Color(0xFF123456));
    expect(tester.widget<TextField>(hexField()).controller!.text, '#123456');
    expect(find.text('Custom · #123456'), findsOneWidget);
  });
}
