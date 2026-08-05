import 'package:claude_commander_client/pages/projects_page.dart';
import 'package:claude_commander_client/pages/servers_page.dart';
import 'package:claude_commander_client/pages/settings_page.dart';
import 'package:claude_commander_client/pages/theme_picker_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
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
  late WorkspaceStore workspace;
  late ThemeController theme;

  setUp(() {
    api = FakeCommanderApi();
    store = CommanderStore(api: api, config: testConfig);
    workspace = WorkspaceStore.withStores([store]);
    // Never the device's real preferences: the theme row and picker must not
    // touch (or read) whatever the developer has selected.
    theme = ThemeController(store: InMemoryPrefStore());
  });

  tearDown(() => workspace.dispose());

  /// Hosts the page the way `main()` does: both scopes above the `MaterialApp`,
  /// with the app rebuilt on a theme change so a selection actually rethemes.
  Widget wrap(WorkspaceStore workspace) => WorkspaceScope(
    workspace: workspace,
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
    await tester.pumpWidget(wrap(workspace));
    await tester.pumpAndSettle();

    expect(find.text('SERVERS'), findsOneWidget);
    expect(find.text('WORKSPACE'), findsOneWidget);
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
    // setUp: `WorkspaceStore.dispose()` disposes its children, so a store held
    // by two workspaces would be disposed twice — once here and once by the
    // shared tearDown.
    final local = CommanderStore(api: api, config: testConfig);
    final remoteApi = FakeCommanderApi();
    final remote = CommanderStore(api: remoteApi, config: remoteConfig);
    final both = WorkspaceStore.withStores([local, remote]);
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
    await tester.pumpWidget(wrap(workspace));
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
    await tester.pumpWidget(wrap(workspace));
    await tester.pumpAndSettle();

    await tapAndPush(tester, find.text('test'));

    expect(find.byType(ServersPage), findsOneWidget);
  });

  testWidgets('with no servers configured, offers to add one', (tester) async {
    final empty = WorkspaceStore(
      api: api,
      listStore: InMemoryServerListStore(),
    );
    addTearDown(empty.dispose);
    await tester.pumpWidget(wrap(empty));
    await tester.pumpAndSettle();

    expect(find.text('No servers configured'), findsOneWidget);
    await tapAndPush(tester, find.text('No servers configured'));
    expect(find.byType(ServersPage), findsOneWidget);
  });

  testWidgets('the workspace rows stay shut while no server is connected', (
    tester,
  ) async {
    // Never connected, so no live handle — the row must not open a manager that
    // has no server to talk to.
    await tester.pumpWidget(wrap(workspace));
    await tester.pumpAndSettle();

    expect(find.text('Needs a connected server'), findsNWidgets(2));
    await tapAndPush(tester, find.text('Projects'));
    expect(find.byType(ProjectsPage), findsNothing);
    expect(find.byType(SettingsPage), findsOneWidget);
  });

  testWidgets('the workspace rows open once a server is connected', (
    tester,
  ) async {
    await store.connect();
    await tester.pumpWidget(wrap(workspace));
    await tester.pumpAndSettle();

    expect(find.text('Needs a connected server'), findsNothing);
    await tapAndPush(tester, find.text('Projects'));
    expect(find.byType(ProjectsPage), findsOneWidget);
  });

  testWidgets('the theme row names the active theme and opens the picker', (
    tester,
  ) async {
    await store.connect();
    await tester.pumpWidget(wrap(workspace));
    await tester.pumpAndSettle();

    expect(find.text('Mission Control'), findsOneWidget);

    await tapAndPush(tester, find.text('Theme'));
    expect(find.byType(ThemePickerPage), findsOneWidget);
  });

  testWidgets('the theme row follows a selection made elsewhere', (
    tester,
  ) async {
    await store.connect();
    await tester.pumpWidget(wrap(workspace));
    await tester.pumpAndSettle();

    await theme.select(ThemeId.lcars);
    await tester.pumpAndSettle();

    // LCARS uppercases labels, so the row's label cases with the theme while
    // the theme's name — a proper noun from ThemeId.label — does not.
    expect(find.text('LCARS'), findsOneWidget);
    expect(find.text('THEME'), findsOneWidget);
  });
}
