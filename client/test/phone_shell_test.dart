import 'dart:async';

import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/pages/create_session_page.dart';
import 'package:claude_commander_client/pages/phone_shell.dart';
import 'package:claude_commander_client/pages/session_detail_page.dart';
import 'package:claude_commander_client/pages/settings_page.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:claude_commander_client/widgets/brand_mark.dart';
import 'package:claude_commander_client/window/window_controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;
  late CommanderStore store;
  late WorkspaceStore workspace;

  setUp(() {
    api = FakeCommanderApi();
    store = CommanderStore(api: api, config: testConfig);
    workspace = WorkspaceStore.withStores([store]);
  });

  tearDown(() => workspace.dispose());

  /// The shell under the scopes `main()` gives it. Themeless falls back to
  /// Mission Control tokens, which is how the shell's own tests pump it; passing
  /// [tokens] opts into LCARS. The theme and window scopes are here so the
  /// settings route can actually be pushed and built.
  ///
  /// [textScale] rescales through a `copyWith` on the inherited data rather than
  /// a fresh `MediaQueryData`, which would drop the surface size along with it.
  Widget wrap({CommanderTokens? tokens, double? textScale}) => WorkspaceScope(
    workspace: workspace,
    child: WindowScope(
      controller: null,
      child: ThemeScope(
        // Never the device's real preferences.
        controller: ThemeController(store: InMemoryPrefStore()),
        child: MaterialApp(
          theme: tokens == null ? null : themeDataFor(tokens),
          home: textScale == null
              ? const PhoneShell()
              : Builder(
                  builder: (context) => MediaQuery(
                    data: MediaQuery.of(
                      context,
                    ).copyWith(textScaler: TextScaler.linear(textScale)),
                    child: const PhoneShell(),
                  ),
                ),
        ),
      ),
    ),
  );

  testWidgets('shows the Fleet header, both nav tabs, and the create FAB', (
    tester,
  ) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    // The branded Fleet header (BrandMark + title) is on the Fleet tab.
    expect(find.byType(BrandMark), findsOneWidget);
    expect(find.text('Fleet'), findsOneWidget);
    expect(find.text('Alpha'), findsOneWidget);

    // Both bottom-nav tabs and the centre FAB are present.
    expect(find.text('FLEET'), findsOneWidget);
    expect(find.text('ACTIVITY'), findsOneWidget);
    expect(find.byType(FloatingActionButton), findsOneWidget);
  });

  testWidgets('switching to the Activity tab does not throw', (tester) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.text('ACTIVITY'));
    await tester.pumpAndSettle();

    // Both bodies are kept alive in the IndexedStack, so the Activity header is
    // in the tree; the tap just switches which is shown.
    expect(tester.takeException(), isNull);
    expect(find.text('Activity'), findsOneWidget);
  });

  testWidgets('tapping a session row pushes the detail route', (tester) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    api.getSessionDetailResponse = sessionDetail();
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.text('Alpha'));
    await tester.pumpAndSettle();

    expect(find.byType(SessionDetailPage), findsOneWidget);
  });

  testWidgets('the FAB pushes the create route', (tester) async {
    api.listSessionsResponse = const [];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.byType(FloatingActionButton));
    await tester.pumpAndSettle();

    expect(find.byType(CreateSessionPage), findsOneWidget);
  });

  /// Settings is a *shell* action, not a view one: it used to hang off the Fleet
  /// view's frame, which left it unreachable from the Activity tab and stacked a
  /// second bottom terminator above the footer in LCARS.
  group('settings in the footer', () {
    Future<void> pump(
      WidgetTester tester, {
      CommanderTokens? tokens,
      double? textScale,
    }) async {
      api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
      unawaited(store.connect());
      await tester.pumpWidget(wrap(tokens: tokens, textScale: textScale));
      await tester.pumpAndSettle();
    }

    testWidgets('Mission Control puts it in the bottom bar', (tester) async {
      await pump(tester);

      // Not in the Fleet header, where the view used to carry it — the shell's
      // bar, so it is the same control on either tab.
      expect(
        find.descendant(
          of: find.byType(BottomAppBar),
          matching: find.byIcon(Icons.settings),
        ),
        findsOneWidget,
      );
    });

    testWidgets('Mission Control keeps the tabs symmetric about the FAB', (
      tester,
    ) async {
      await pump(tester);

      // The settings button is a leading widget in the bar, so without its
      // trailing counterweight the notch — and the tabs either side of it —
      // would sit left of the centre-docked FAB.
      final fab = tester.getCenter(find.byType(FloatingActionButton)).dx;
      final fleet = tester.getCenter(find.text('FLEET')).dx;
      final activity = tester.getCenter(find.text('ACTIVITY')).dx;
      expect(fab - fleet, moreOrLessEquals(activity - fab, epsilon: 0.5));
    });

    testWidgets('LCARS makes it the leading block of the footer run', (
      tester,
    ) async {
      await pump(tester, tokens: lcarsTokens);

      final settings = tester.getRect(
        find.widgetWithText(ChromeElbow, 'SETTINGS'),
      );
      final rail = tester.getRect(find.widgetWithText(ChromeElbow, '47-A'));
      final fleet = tester.getRect(find.widgetWithText(ChromeElbow, 'FLEET'));

      // Directly under the rail: same left edge, same width.
      expect(settings.left, rail.left);
      expect(settings.width, lcarsTokens.railWidth);
      // And inline with the run it leads, rather than a block above it.
      expect(settings.center.dy, moreOrLessEquals(fleet.center.dy));
    });

    testWidgets('LCARS keeps the run on the screen edge at 1.3× text', (
      tester,
    ) async {
      // 'SETTINGS' fits its 62px block at 11px by a whisker, so any accessibility
      // scaling wraps it and `ChromeElbow` grows that block to fit two lines. It
      // is the run's only fixed-width block, so it is the only one that can grow
      // — and a centred `Row` would then lift every other block off the bottom of
      // the screen, on the one run whose premise is meeting that edge.
      await pump(tester, tokens: lcarsTokens, textScale: 1.3);

      final settings = tester.getRect(
        find.widgetWithText(ChromeElbow, 'SETTINGS'),
      );
      final fleet = tester.getRect(find.widgetWithText(ChromeElbow, 'FLEET'));
      final activity = tester.getRect(
        find.widgetWithText(ChromeElbow, 'ACTIVITY'),
      );

      expect(settings.height, greaterThan(fleet.height));
      expect(fleet.bottom, settings.bottom);
      expect(activity.bottom, settings.bottom);
    });

    testWidgets('LCARS opens settings from the Activity tab', (tester) async {
      await pump(tester, tokens: lcarsTokens);

      await tester.tap(find.widgetWithText(ChromeElbow, 'ACTIVITY'));
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(ChromeElbow, 'SETTINGS'));
      await tester.pumpAndSettle();

      expect(find.byType(SettingsPage), findsOneWidget);
    });
  });
}
