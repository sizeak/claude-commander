import 'package:claude_commander_client/pages/workspaces_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/fleet_store.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

const _workProject = 'aaaaaaaa-0000-0000-0000-000000000001';
const _mainProject = 'aaaaaaaa-0000-0000-0000-000000000002';
const _sideProject = 'aaaaaaaa-0000-0000-0000-000000000003';

const _laptop = ServerConfig(
  id: 'id-a',
  name: 'laptop',
  baseUrl: 'http://laptop:7878',
  token: 't-a',
);
const _codespace = ServerConfig(
  id: 'id-b',
  name: 'codespace',
  baseUrl: 'http://codespace:7878',
  token: 't-b',
);

/// Settings → PROJECTS → Workspaces: every edit to the workspace list goes to
/// every server at once, and a server that refuses one is named in a snackbar
/// rather than silently left behind.
void main() {
  late FakeCommanderApi laptopApi;
  late FakeCommanderApi codespaceApi;
  late FleetStore fleet;

  setUp(() async {
    laptopApi = FakeCommanderApi()
      ..workspacesResponse = const [
        WorkspaceDef(name: 'Work', color: '#ff8800'),
        WorkspaceDef(name: 'Personal'),
      ]
      ..projectsResponse = [
        projectInfo(id: _workProject, name: 'work-repo', workspace: 'Work'),
        projectInfo(id: _mainProject, name: 'main-repo'),
      ];
    codespaceApi = FakeCommanderApi()
      ..workspacesResponse = const [WorkspaceDef(name: 'Personal')]
      ..projectsResponse = [
        projectInfo(id: _sideProject, name: 'side-repo', workspace: 'Personal'),
      ];
    final laptop = CommanderStore(api: laptopApi, config: _laptop);
    final codespace = CommanderStore(api: codespaceApi, config: _codespace);
    fleet = FleetStore.withStores([
      laptop,
      codespace,
    ], prefs: InMemoryPrefStore());
    await laptop.connect();
    await codespace.connect();
  });

  tearDown(() => fleet.dispose());

  Future<void> pumpPage(WidgetTester tester) async {
    tester.view.physicalSize = const Size(430, 1400);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(MaterialApp(home: WorkspacesPage(fleet: fleet)));
    await tester.pumpAndSettle();
  }

  SetWorkspacesRequestDto lastPut(FakeCommanderApi api) =>
      api.lastCall('setWorkspaces')!.args['request']!
          as SetWorkspacesRequestDto;

  Finder rowOf(String? name) => find.byKey(workspaceRowKey(name));

  Future<void> openRowMenu(WidgetTester tester, String? name) async {
    await tester.tap(
      find.descendant(
        of: rowOf(name),
        matching: find.byTooltip('Workspace actions'),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('lists Main first, then the merged workspaces, and every '
      "project with the workspace it's in", (tester) async {
    await pumpPage(tester);

    expect(rowOf(null), findsOneWidget);
    expect(rowOf('Work'), findsOneWidget);
    expect(rowOf('Personal'), findsOneWidget);
    final mainY = tester.getTopLeft(rowOf(null)).dy;
    final workY = tester.getTopLeft(rowOf('Work')).dy;
    final personalY = tester.getTopLeft(rowOf('Personal')).dy;
    expect(mainY < workY && workY < personalY, isTrue);

    // Projects from both servers, each captioned with its server and workspace.
    expect(find.text('work-repo'), findsOneWidget);
    expect(find.text('laptop · Work'), findsOneWidget);
    expect(find.text('laptop · Main'), findsOneWidget);
    expect(find.text('codespace · Personal'), findsOneWidget);
  });

  testWidgets('a new workspace is sent to every server', (tester) async {
    await pumpPage(tester);

    await tester.tap(find.byTooltip('New workspace'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Side');
    await tester.tap(find.text('OK'));
    await tester.pumpAndSettle();

    for (final api in [laptopApi, codespaceApi]) {
      expect(lastPut(api).workspaces.map((w) => w.name), [
        'Work',
        'Personal',
        'Side',
      ]);
    }
    expect(rowOf('Side'), findsOneWidget);
  });

  testWidgets('servers that disagree on a spelling each keep their own', (
    tester,
  ) async {
    // Created independently while each server was unreachable: "Personal"
    // here, "personal" there. The merge keeps both (exact names), but no
    // server accepts both — so each is sent only what it can take.
    codespaceApi.workspacesResponse = const [WorkspaceDef(name: 'personal')];
    await fleet.servers.last.refresh();
    await pumpPage(tester);

    await tester.tap(find.byTooltip('New workspace'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Side');
    await tester.tap(find.text('OK'));
    await tester.pumpAndSettle();

    expect(lastPut(laptopApi).workspaces.map((w) => w.name), [
      'Work',
      'Personal',
      'Side',
    ]);
    expect(lastPut(codespaceApi).workspaces.map((w) => w.name), [
      'Work',
      'personal',
      'Side',
    ]);
  });

  testWidgets('a reserved name is refused before anything is sent', (
    tester,
  ) async {
    await pumpPage(tester);

    await tester.tap(find.byTooltip('New workspace'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'main');
    await tester.pump();
    expect(find.textContaining('reserved'), findsOneWidget);
    await tester.tap(find.text('OK'));
    await tester.pumpAndSettle();

    expect(laptopApi.countOf('setWorkspaces'), 0);
    // Still on the dialog: a refused name does not dismiss it.
    expect(find.byType(AlertDialog), findsOneWidget);
  });

  testWidgets('a name that is already taken is refused too', (tester) async {
    await pumpPage(tester);

    await tester.tap(find.byTooltip('New workspace'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'work');
    await tester.pump();
    expect(find.textContaining('already'), findsOneWidget);
  });

  testWidgets('rename goes to every server', (tester) async {
    await pumpPage(tester);

    await openRowMenu(tester, 'Work');
    await tester.tap(find.text('Rename'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Job');
    await tester.tap(find.text('OK'));
    await tester.pumpAndSettle();

    for (final api in [laptopApi, codespaceApi]) {
      expect(
        api.lastCall('renameWorkspace')!.args,
        allOf(containsPair('from', 'Work'), containsPair('to', 'Job')),
      );
    }
    expect(rowOf('Job'), findsOneWidget);
    expect(find.text('laptop · Job'), findsOneWidget);
  });

  testWidgets('renaming Main relabels it without a rename call', (
    tester,
  ) async {
    await pumpPage(tester);

    await openRowMenu(tester, null);
    // Main cannot be deleted or reordered.
    expect(find.text('Delete'), findsNothing);
    expect(find.text('Move up'), findsNothing);
    await tester.tap(find.text('Rename'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Home');
    await tester.tap(find.text('OK'));
    await tester.pumpAndSettle();

    expect(laptopApi.countOf('renameWorkspace'), 0);
    expect(lastPut(codespaceApi).main?.name, 'Home');
    expect(find.text('laptop · Home'), findsOneWidget);
  });

  testWidgets('delete confirms, then moves the projects to Main', (
    tester,
  ) async {
    await pumpPage(tester);

    await openRowMenu(tester, 'Work');
    await tester.tap(find.text('Delete'));
    await tester.pumpAndSettle();
    expect(find.textContaining('moves its projects to Main'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilledButton, 'Delete'));
    await tester.pumpAndSettle();

    expect(laptopApi.countOf('deleteWorkspace'), 1);
    expect(codespaceApi.countOf('deleteWorkspace'), 1);
    expect(rowOf('Work'), findsNothing);
    expect(find.text('laptop · Main'), findsNWidgets(2));
  });

  testWidgets('move down reorders the list on every server', (tester) async {
    await pumpPage(tester);

    await openRowMenu(tester, 'Work');
    await tester.tap(find.text('Move down'));
    await tester.pumpAndSettle();

    expect(lastPut(codespaceApi).workspaces.map((w) => w.name), [
      'Personal',
      'Work',
    ]);
  });

  testWidgets('colour picks from the palette, or clears', (tester) async {
    await pumpPage(tester);

    await openRowMenu(tester, 'Personal');
    await tester.tap(find.text('Colour'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('swatch-#6f9fd8')));
    await tester.pumpAndSettle();
    expect(
      lastPut(
        laptopApi,
      ).workspaces.firstWhere((w) => w.name == 'Personal').color,
      '#6f9fd8',
    );

    await openRowMenu(tester, 'Work');
    await tester.tap(find.text('Colour'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('No colour'));
    await tester.pumpAndSettle();
    expect(
      lastPut(laptopApi).workspaces.firstWhere((w) => w.name == 'Work').color,
      isNull,
    );
  });

  testWidgets('the startup workspace can be pinned', (tester) async {
    await pumpPage(tester);

    expect(find.text('Last used'), findsOneWidget);
    await tester.tap(find.text('Startup workspace'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Personal').last);
    await tester.pumpAndSettle();

    expect(lastPut(laptopApi).startupWorkspace, 'Personal');
    expect(lastPut(codespaceApi).startupWorkspace, 'Personal');
  });

  testWidgets('moving a project writes only to the server that owns it', (
    tester,
  ) async {
    await pumpPage(tester);

    await tester.tap(find.text('main-repo'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Personal').last);
    await tester.pumpAndSettle();

    expect(
      laptopApi.lastCall('setProjectWorkspace')!.args,
      allOf(
        containsPair('projectId', _mainProject),
        containsPair('workspace', 'Personal'),
      ),
    );
    expect(codespaceApi.countOf('setProjectWorkspace'), 0);
    expect(find.text('laptop · Personal'), findsOneWidget);
  });

  testWidgets('a server that refuses an edit is named in a snackbar', (
    tester,
  ) async {
    codespaceApi.workspaceMutationError = Exception('boom');
    await pumpPage(tester);

    await tester.tap(find.byTooltip('New workspace'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Side');
    await tester.tap(find.text('OK'));
    await tester.pumpAndSettle();

    expect(find.textContaining("Couldn't update codespace"), findsOneWidget);
    // The server that took it still did.
    expect(laptopApi.countOf('setWorkspaces'), 1);
  });
}
