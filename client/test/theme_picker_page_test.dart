import 'package:claude_commander_client/pages/theme_picker_page.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late InMemoryPrefStore prefs;
  late ThemeController theme;

  setUp(() {
    // In-memory only: a picker test must never read or write the device's real
    // theme preference.
    prefs = InMemoryPrefStore();
    theme = ThemeController(store: prefs);
  });

  /// Hosts the picker as `main()` does — [ThemeScope] above the `MaterialApp`,
  /// which rebuilds on a selection so a tap really rethemes the app.
  Widget wrap() => ThemeScope(
    controller: theme,
    child: ListenableBuilder(
      listenable: theme,
      builder: (context, _) => MaterialApp(
        theme: themeDataFor(theme.tokens),
        home: const ThemePickerPage(),
      ),
    ),
  );

  /// Every `Color` painted anywhere inside [id]'s preview.
  Set<Color> previewColors(WidgetTester tester, ThemeId id) {
    final preview = find.byWidgetPredicate(
      (w) => w is ThemePreview && w.id == id,
    );
    expect(preview, findsOneWidget);
    final colors = <Color>{};
    for (final element in find
        .descendant(of: preview, matching: find.byType(Container))
        .evaluate()) {
      final container = element.widget as Container;
      final color = container.color;
      if (color != null) colors.add(color);
      final decoration = container.decoration;
      if (decoration is BoxDecoration) {
        if (decoration.color case final c?) colors.add(c);
        if (decoration.border?.top.color case final c?) colors.add(c);
      }
    }
    return colors;
  }

  testWidgets('offers a card per theme with its description and badge', (
    tester,
  ) async {
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    for (final id in ThemeId.values) {
      expect(find.text(id.label), findsOneWidget, reason: id.wire);
      expect(
        find.byWidgetPredicate((w) => w is ThemePreview && w.id == id),
        findsOneWidget,
        reason: id.wire,
      );
    }
    expect(find.text('dark · indigo/cyan · Space Grotesk'), findsOneWidget);
    expect(find.text('black · amber/lilac · Antonio'), findsOneWidget);
    expect(find.text('DEFAULT'), findsOneWidget);
    expect(find.text('NEW'), findsOneWidget);
    expect(
      find.textContaining('Applies on this device'),
      findsOneWidget,
      reason: 'the picker says the choice is device-local',
    );
  });

  testWidgets('marks exactly the active theme', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.byIcon(Icons.check_circle), findsOneWidget);

    await tester.tap(find.text('LCARS'));
    await tester.pumpAndSettle();

    // The page stays open on the new selection rather than popping, so the mark
    // has moved but is still the only one.
    expect(find.byType(ThemePickerPage), findsOneWidget);
    expect(find.byIcon(Icons.check_circle), findsOneWidget);
    expect(theme.id, ThemeId.lcars);
  });

  testWidgets('tapping a card applies and persists that theme', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.text('LCARS'));
    await tester.pumpAndSettle();

    expect(theme.tokens.chrome, ChromeKind.lcars);
    expect(await prefs.read(ThemeController.prefKey), ThemeId.lcars.wire);
  });

  testWidgets('each preview paints its own theme, not the active one', (
    tester,
  ) async {
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    // Mission Control is active, so this is the deliberate exception to the
    // "every colour from CommanderTokens.of(context)" rule: the LCARS preview
    // must be painted in LCARS' colours while the app around it is not.
    final lcars = previewColors(tester, ThemeId.lcars);
    expect(lcars, contains(lcarsTokens.primary));
    expect(lcars, contains(lcarsTokens.nav));
    expect(lcars, contains(lcarsTokens.canvas));
    expect(lcars, isNot(contains(missionControlTokens.primary)));

    final mc = previewColors(tester, ThemeId.missionControl);
    expect(mc, contains(missionControlTokens.canvas));
    expect(mc, contains(missionControlTokens.surface));
    expect(mc, isNot(contains(lcarsTokens.primary)));

    // And it stays that way after switching: the previews describe the themes,
    // so neither follows the selection.
    await tester.tap(find.text('LCARS'));
    await tester.pumpAndSettle();
    expect(
      previewColors(tester, ThemeId.missionControl),
      contains(missionControlTokens.canvas),
    );
  });
}
