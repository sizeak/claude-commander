import 'package:claude_commander_client/pages/projects_page.dart';
import 'package:claude_commander_client/pages/servers_page.dart';
import 'package:claude_commander_client/pages/settings_page.dart';
import 'package:claude_commander_client/pages/theme_picker_page.dart';
import 'package:claude_commander_client/pages/workspaces_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/fleet_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/window/window_controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fake_window_service.dart';
import 'support/fixtures.dart';

/// A second server, deliberately not on the loopback host so the `local` /
/// `remote` tag can be exercised both ways.
const remoteConfig = ServerConfig(
  id: 'remote-server',
  name: 'workstation',
  baseUrl: 'https://box.example:7878',
  token: 'remote-token',
);

void main() {
  late FakeCommanderApi api;
  late CommanderStore store;
  late FleetStore fleet;
  late ThemeController theme;

  setUp(() {
    api = FakeCommanderApi();
    store = CommanderStore(api: api, config: testConfig);
    fleet = FleetStore.withStores([store]);
    // Never the device's real preferences: the theme row and picker must not
    // touch (or read) whatever the developer has selected.
    theme = ThemeController(store: InMemoryPrefStore());
  });

  tearDown(() => fleet.dispose());

  /// Hosts the page the way `main()` does: the scopes above the `MaterialApp`,
  /// with the app rebuilt on a theme change so a selection actually rethemes.
  ///
  /// [window] defaults to null, which is the phone case — and the case every test
  /// that predates the window section is asserting.
  Widget wrap(FleetStore fleet, {WindowController? window}) => FleetScope(
    fleet: fleet,
    child: WindowScope(
      controller: window,
      child: ThemeScope(
        controller: theme,
        child: ListenableBuilder(
          listenable: theme,
          builder: (context, _) => MaterialApp(
            theme: themeDataFor(theme.tokens),
            home: const SettingsPage(),
          ),
        ),
      ),
    ),
  );

  /// Pushes a route and lets its transition finish, without `pumpAndSettle` —
  /// the pages pushed here may hold a progress indicator, which never settles.
  Future<void> tapAndPush(WidgetTester tester, Finder target) async {
    await tester.tap(target);
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
  }

  testWidgets('renders the three sections', (tester) async {
    await store.connect();
    await tester.pumpWidget(wrap(fleet));
    await tester.pumpAndSettle();

    expect(find.text('SERVERS'), findsOneWidget);
    expect(find.text('PROJECTS'), findsOneWidget);
    expect(find.text('APPEARANCE'), findsOneWidget);
  });

  testWidgets('a server row carries its session count and locality', (
    tester,
  ) async {
    api.listSessionsResponse = [
      sessionInfo(title: 'Alpha'),
      sessionInfo(id: '99999999-2222-3333-4444-555555555555', title: 'Beta'),
    ];
    // Both stores are local to this test rather than reusing the one from
    // setUp: `FleetStore.dispose()` disposes its children, so a store held
    // by two fleet stores would be disposed twice — once here and once by the
    // shared tearDown.
    final local = CommanderStore(api: api, config: testConfig);
    final remoteApi = FakeCommanderApi();
    final remote = CommanderStore(api: remoteApi, config: remoteConfig);
    final both = FleetStore.withStores([local, remote]);
    addTearDown(both.dispose);

    await local.connect();
    await remote.connect();
    await tester.pumpWidget(wrap(both));
    await tester.pumpAndSettle();

    expect(find.text('test'), findsOneWidget);
    expect(find.text('workstation'), findsOneWidget);
    // The loopback server reads local; the named host reads remote — the check
    // is on the parsed host, so `box.example` is never mistaken for localhost.
    expect(find.text('2 · local'), findsOneWidget);
    expect(find.text('0 · remote'), findsOneWidget);
  });

  testWidgets('a degraded server reports why instead of a stale count', (
    tester,
  ) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    await store.connect();
    await tester.pumpWidget(wrap(fleet));
    await tester.pumpAndSettle();
    expect(find.text('1 · local'), findsOneWidget);

    api.emitConnection(
      const ConnectionStateDto(
        kind: ConnectionStateKind.degraded,
        reason: 'flaky',
      ),
    );
    await tester.pumpAndSettle();

    // The count came from the last good snapshot, so showing it would read as
    // live status the server can no longer back up.
    expect(find.text('1 · local'), findsNothing);
    expect(find.text('flaky'), findsOneWidget);
  });

  testWidgets('tapping a server row opens the servers manager', (tester) async {
    await store.connect();
    await tester.pumpWidget(wrap(fleet));
    await tester.pumpAndSettle();

    await tapAndPush(tester, find.text('test'));

    expect(find.byType(ServersPage), findsOneWidget);
  });

  testWidgets('with no servers configured, offers to add one', (tester) async {
    final empty = FleetStore(api: api, listStore: InMemoryServerListStore());
    addTearDown(empty.dispose);
    await tester.pumpWidget(wrap(empty));
    await tester.pumpAndSettle();

    expect(find.text('No servers configured'), findsOneWidget);
    await tapAndPush(tester, find.text('No servers configured'));
    expect(find.byType(ServersPage), findsOneWidget);
  });

  testWidgets('the project rows stay shut while no server is connected', (
    tester,
  ) async {
    // Never connected, so no live handle — the row must not open a manager that
    // has no server to talk to.
    await tester.pumpWidget(wrap(fleet));
    await tester.pumpAndSettle();

    expect(find.text('Needs a connected server'), findsNWidgets(3));
    await tapAndPush(tester, find.text('Projects'));
    expect(find.byType(ProjectsPage), findsNothing);
    expect(find.byType(SettingsPage), findsOneWidget);
  });

  testWidgets('the project rows open once a server is connected', (
    tester,
  ) async {
    await store.connect();
    await tester.pumpWidget(wrap(fleet));
    await tester.pumpAndSettle();

    expect(find.text('Needs a connected server'), findsNothing);
    await tapAndPush(tester, find.text('Projects'));
    expect(find.byType(ProjectsPage), findsOneWidget);
  });

  testWidgets('the Workspaces row opens the workspaces editor', (tester) async {
    await store.connect();
    await tester.pumpWidget(wrap(fleet));
    await tester.pumpAndSettle();

    // With only Main, the caption invites the first one rather than counting.
    expect(find.text('Group projects to switch between'), findsOneWidget);
    await tapAndPush(tester, find.text('Workspaces'));
    expect(find.byType(WorkspacesPage), findsOneWidget);
  });

  testWidgets('the Workspaces row counts the workspaces once there are two', (
    tester,
  ) async {
    api.workspacesResponse = const [WorkspaceDef(name: 'Work')];
    await store.connect();
    await tester.pumpWidget(wrap(fleet));
    await tester.pumpAndSettle();

    expect(find.text('2 workspaces'), findsOneWidget);
  });

  testWidgets('the theme row names the active theme and opens the picker', (
    tester,
  ) async {
    await store.connect();
    await tester.pumpWidget(wrap(fleet));
    await tester.pumpAndSettle();

    expect(find.text('Mission Control'), findsOneWidget);

    await tapAndPush(tester, find.text('Theme'));
    expect(find.byType(ThemePickerPage), findsOneWidget);
  });

  group('the window section', () {
    WindowController newWindow() => WindowController(
      store: InMemoryPrefStore(),
      service: FakeWindowService(),
    );

    testWidgets('is absent where there is no window to manage', (tester) async {
      // The phone: no controller in scope, so the section has nothing to say and
      // does not appear. No platform check anywhere in the page.
      await tester.pumpWidget(wrap(fleet));
      await tester.pumpAndSettle();

      expect(find.text('WINDOW'), findsNothing);
      expect(find.text('Full screen'), findsNothing);
    });

    testWidgets('reports both states and their shortcuts', (tester) async {
      await tester.pumpWidget(wrap(fleet, window: newWindow()));
      await tester.pumpAndSettle();

      expect(find.text('WINDOW'), findsOneWidget);
      expect(find.text('OFF'), findsOneWidget);
      expect(find.text('HIDDEN'), findsOneWidget);
      // The rows are where the shortcuts are documented in the app itself.
      expect(find.text('F11'), findsOneWidget);
      expect(find.text('Shift+F11'), findsOneWidget);
    });

    testWidgets('tapping the fullscreen row drives the window', (tester) async {
      final window = newWindow();
      await tester.pumpWidget(wrap(fleet, window: window));
      await tester.pumpAndSettle();

      await tester.tap(find.text('Full screen'));
      await tester.pumpAndSettle();

      expect(window.fullscreen, isTrue);
      expect(find.text('ON'), findsOneWidget);
    });

    testWidgets('tapping the frame row restores the native title bar', (
      tester,
    ) async {
      final window = newWindow();
      await tester.pumpWidget(wrap(fleet, window: window));
      await tester.pumpAndSettle();

      await tester.tap(find.text('Window frame'));
      await tester.pumpAndSettle();

      expect(window.titleBar, TitleBarMode.native);
      expect(find.text('NATIVE'), findsOneWidget);
    });

    testWidgets('follows a change made by the F11 shortcut', (tester) async {
      final window = newWindow();
      await tester.pumpWidget(wrap(fleet, window: window));
      await tester.pumpAndSettle();

      await window.setFullscreen(true);
      await tester.pumpAndSettle();

      expect(find.text('ON'), findsOneWidget);
    });
  });

  testWidgets('the theme row follows a selection made elsewhere', (
    tester,
  ) async {
    await store.connect();
    await tester.pumpWidget(wrap(fleet));
    await tester.pumpAndSettle();

    await theme.select(ThemeId.lcars);
    await tester.pumpAndSettle();

    // LCARS uppercases labels, so the row's label cases with the theme while
    // the theme's name — a proper noun from ThemeId.label — does not.
    expect(find.text('LCARS'), findsOneWidget);
    expect(find.text('THEME'), findsOneWidget);
  });
}
