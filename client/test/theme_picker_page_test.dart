import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/pages/theme_picker_page.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/fleet_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/theme_prefs.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:claude_commander_client/util/colour_hex.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

const _red = Color(0xFFFF0000);
const _blue = Color(0xFF0000FF);

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
  Set<Color> previewColors(ThemeId id) {
    final preview = find.byWidgetPredicate(
      (w) => w is ThemePreview && w.id == id,
    );
    expect(preview, findsOneWidget);
    final colors = <Color>{};
    for (final element
        in find
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
    final lcars = previewColors(ThemeId.lcars);
    expect(lcars, contains(lcarsTokens.primary));
    expect(lcars, contains(lcarsTokens.nav));
    expect(lcars, contains(lcarsTokens.canvas));
    expect(lcars, isNot(contains(missionControlTokens.primary)));

    final mc = previewColors(ThemeId.missionControl);
    expect(mc, contains(missionControlTokens.canvas));
    expect(mc, contains(missionControlTokens.surface));
    expect(mc, isNot(contains(lcarsTokens.primary)));

    // And it stays that way after switching: the previews describe the themes,
    // so neither follows the selection.
    await tester.tap(find.text('LCARS'));
    await tester.pumpAndSettle();
    expect(
      previewColors(ThemeId.missionControl),
      contains(missionControlTokens.canvas),
    );
  });

  group('colour rows', () {
    Finder row(ThemeRole role) => find.byKey(themeRoleRowKey(role));

    Future<void> pumpTall(WidgetTester tester, Widget app) async {
      tester.view.physicalSize = const Size(430, 2400);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(app);
      await tester.pumpAndSettle();
    }

    Future<void> pickHex(
      WidgetTester tester,
      ThemeRole role,
      String hex,
    ) async {
      await tester.tap(row(role));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('colour-hex-field')),
        hex,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(FilledButton, 'Save'));
      await tester.pumpAndSettle();
    }

    testWidgets('one row per overridable role, showing the preset value', (
      tester,
    ) async {
      await pumpTall(tester, wrap());
      for (final role in ThemeRole.values) {
        expect(row(role), findsOneWidget, reason: role.wire);
      }
      expect(
        find.descendant(
          of: row(ThemeRole.primary),
          matching: find.text(
            '${hexOf(missionControlTokens.primary)} · preset',
          ),
        ),
        findsOneWidget,
      );
      // With no fleet (or one workspace) there is nothing to scope to.
      expect(find.byKey(themeScopeSelectorKey), findsNothing);
    });

    testWidgets('a picked colour overrides the usual theme', (tester) async {
      await pumpTall(tester, wrap());
      await pickHex(tester, ThemeRole.primary, '#ff0000');
      expect(theme.usual.overrides, {ThemeRole.primary: _red});
      expect(theme.tokens.primary, _red);
      expect(
        find.descendant(
          of: row(ThemeRole.primary),
          matching: find.text('#ff0000 · custom'),
        ),
        findsOneWidget,
      );

      // Reset in the dialog goes back to the preset.
      await tester.tap(row(ThemeRole.primary));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Reset'));
      await tester.pumpAndSettle();
      expect(theme.usual.overrides, isEmpty);
    });

    group('with workspaces', () {
      late FakeCommanderApi api;
      late FleetStore fleet;

      setUp(() async {
        api = FakeCommanderApi()
          ..workspacesResponse = const [
            WorkspaceDef(name: 'Work'),
            WorkspaceDef(name: 'Personal'),
          ];
        final store = CommanderStore(api: api, config: testConfig);
        fleet = FleetStore.withStores([store], prefs: InMemoryPrefStore());
        await store.connect();
        await fleet.selectWorkspace('Work');
      });

      tearDown(() => fleet.dispose());

      Widget wrapFleet({ThemeEditScope? initial}) => FleetScope(
        fleet: fleet,
        child: ThemeScope(
          controller: theme,
          child: ListenableBuilder(
            listenable: theme,
            builder: (context, _) => MaterialApp(
              theme: themeDataFor(theme.tokens),
              home: ThemePickerPage(initialScope: initial),
            ),
          ),
        ),
      );

      /// The scope selector's segments, by label.
      Map<String, bool> scopes(WidgetTester tester) => {
        for (final seg
            in tester
                .widget<ChromeSegmented>(find.byKey(themeScopeSelectorKey))
                .spec
                .segments)
          seg.label: seg.selected,
      };

      Future<void> tapScope(WidgetTester tester, String label) async {
        await tester.tap(
          find.descendant(
            of: find.byKey(themeScopeSelectorKey),
            matching: find.text(label),
          ),
        );
        await tester.pumpAndSettle();
      }

      testWidgets('defaults to the active workspace; a card sets its theme '
          'only', (tester) async {
        await pumpTall(tester, wrapFleet());
        expect(scopes(tester), {'Work': true, usualScopeLabel: false});

        await tester.tap(find.text('LCARS'));
        await tester.pumpAndSettle();
        expect(theme.workspaceTheme('Work')!.themeId, ThemeId.lcars);
        expect(theme.usual.themeId, ThemeId.missionControl);
      });

      testWidgets('the usual scope edits the usual theme', (tester) async {
        await pumpTall(tester, wrapFleet());
        await tapScope(tester, usualScopeLabel);
        expect(scopes(tester), {'Work': false, usualScopeLabel: true});
        await tester.tap(find.text('LCARS'));
        await tester.pumpAndSettle();
        expect(theme.usual.themeId, ThemeId.lcars);
        expect(theme.workspaceTheme('Work'), isNull);
      });

      testWidgets('inherited colours read as usual; overriding one reads '
          'custom', (tester) async {
        await theme.setOverride(null, ThemeRole.primary, _red);
        await pumpTall(tester, wrapFleet());
        expect(
          find.descendant(
            of: row(ThemeRole.primary),
            matching: find.text('#ff0000 · usual'),
          ),
          findsOneWidget,
        );

        await pickHex(tester, ThemeRole.primary, '#0000ff');
        expect(theme.workspaceTheme('Work')!.overrides, {
          ThemeRole.primary: _blue,
        });
        expect(theme.usual.overrides, {ThemeRole.primary: _red});
        expect(
          find.descendant(
            of: row(ThemeRole.primary),
            matching: find.text('#0000ff · custom'),
          ),
          findsOneWidget,
        );
      });

      testWidgets('Reset to usual theme drops the workspace theme', (
        tester,
      ) async {
        await theme.selectFor('Work', ThemeId.lcars);
        await pumpTall(tester, wrapFleet());
        await tester.tap(find.text('Reset to usual theme'));
        await tester.pumpAndSettle();
        expect(theme.workspaceTheme('Work'), isNull);
        // Nothing left to reset.
        expect(find.text('Reset to usual theme'), findsNothing);
      });

      testWidgets('opens on the scope it was given', (tester) async {
        await pumpTall(
          tester,
          wrapFleet(initial: const ThemeEditScope.workspace('Personal')),
        );
        expect(scopes(tester), {'Personal': true, usualScopeLabel: false});
        await tester.tap(find.text('LCARS'));
        await tester.pumpAndSettle();
        expect(theme.workspaceTheme('Personal')!.themeId, ThemeId.lcars);
        expect(theme.workspaceTheme('Work'), isNull);
      });

      testWidgets('Main is scoped under its reserved key', (tester) async {
        await fleet.selectWorkspace(null);
        await pumpTall(tester, wrapFleet());
        expect(scopes(tester), {'Main': true, usualScopeLabel: false});
        await tester.tap(find.text('LCARS'));
        await tester.pumpAndSettle();
        expect(
          theme.workspaceTheme(mainWorkspaceThemeKey)!.themeId,
          ThemeId.lcars,
        );
      });
    });
  });
}
