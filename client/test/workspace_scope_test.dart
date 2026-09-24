import 'dart:async';

import 'package:claude_commander_client/chrome/title_menu.dart';
import 'package:claude_commander_client/pages/activity_page.dart';
import 'package:claude_commander_client/pages/adaptive_shell.dart';
import 'package:claude_commander_client/pages/clone_repo_page.dart';
import 'package:claude_commander_client/pages/create_session_page.dart';
import 'package:claude_commander_client/pages/phone_shell.dart';
import 'package:claude_commander_client/pages/projects_page.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/fleet_store.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

const _workProject = 'aaaaaaaa-0000-0000-0000-000000000001';
const _mainProject = 'aaaaaaaa-0000-0000-0000-000000000002';
const _workSession = '11111111-0000-0000-0000-000000000001';
const _mainSession = '11111111-0000-0000-0000-000000000002';

/// The active workspace scopes every list the user reads (the fleet's two
/// views, the Activity feed, the create-session project picker) and every
/// project the user adds; the switcher that changes it rides on the fleet
/// title, and stays hidden until there is a second workspace.
void main() {
  late FakeCommanderApi api;
  late CommanderStore store;
  late FleetStore fleet;
  late InMemoryPrefStore prefs;

  setUp(() {
    api = FakeCommanderApi();
    store = CommanderStore(api: api, config: testConfig);
    prefs = InMemoryPrefStore();
    fleet = FleetStore.withStores([store], prefs: prefs);
  });

  tearDown(() => fleet.dispose());

  void useSize(WidgetTester tester, Size size) {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
  }

  /// One server with a Work project (one session, waiting for input) and a
  /// Main project (one session).
  void seedTwoWorkspaces() {
    api
      ..workspacesResponse = const [
        WorkspaceDef(name: 'Work', color: '#ff8800'),
      ]
      ..projectsResponse = [
        projectInfo(id: _workProject, name: 'work-repo', workspace: 'Work'),
        projectInfo(id: _mainProject, name: 'main-repo'),
      ]
      ..listSessionsResponse = [
        sessionInfo(
          id: _workSession,
          title: 'Work task',
          projectId: _workProject,
        ),
        sessionInfo(
          id: _mainSession,
          title: 'Main task',
          projectId: _mainProject,
        ),
      ]
      ..agentStatesResponse = AgentStatesSnapshotDto(
        states: [
          AgentStateEntryDto(
            sessionId: sessionInfo(id: _workSession).sessionId,
            state: AgentState.waitingForInput,
          ),
        ],
        commanderRunning: false,
      );
  }

  Widget host(Widget home, {CommanderTokens? tokens}) => FleetScope(
    fleet: fleet,
    child: MaterialApp(
      theme: tokens == null ? null : themeDataFor(tokens),
      home: home,
    ),
  );

  Future<void> pumpPhone(WidgetTester tester, {CommanderTokens? tokens}) async {
    useSize(tester, const Size(430, 900));
    unawaited(store.connect());
    await tester.pumpWidget(host(const PhoneShell(), tokens: tokens));
    await tester.pumpAndSettle();
  }

  group('with only Main', () {
    testWidgets('the fleet title has no switcher and lists everything', (
      tester,
    ) async {
      api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
      await pumpPhone(tester);
      expect(find.byKey(chromeTitleMenuKey), findsNothing);
      expect(find.text('Fleet'), findsOneWidget);
      expect(find.text('Alpha'), findsOneWidget);
    });
  });

  group('with two workspaces', () {
    testWidgets('the phone title names the active workspace and lists its '
        'sessions only', (tester) async {
      seedTwoWorkspaces();
      await pumpPhone(tester);

      expect(find.byKey(chromeTitleMenuKey), findsOneWidget);
      expect(
        find.textContaining('Fleet · Main', findRichText: true),
        findsOneWidget,
      );
      // The caret is an icon: no bundled face has a "▾" glyph to draw.
      expect(
        find.descendant(
          of: find.byKey(chromeTitleMenuKey),
          matching: find.byIcon(Icons.arrow_drop_down),
        ),
        findsOneWidget,
      );
      expect(find.text('Main task'), findsOneWidget);
      expect(find.text('Work task'), findsNothing);
    });

    testWidgets('the picker shows waiting counts and switches the list', (
      tester,
    ) async {
      seedTwoWorkspaces();
      await pumpPhone(tester);

      await tester.tap(find.byKey(chromeTitleMenuKey));
      await tester.pumpAndSettle();
      // Work's session is waiting for input, so its entry says so.
      expect(find.text('●1'), findsOneWidget);

      await tester.tap(find.text('Work').last);
      await tester.pumpAndSettle();

      expect(
        find.textContaining('Fleet · Work', findRichText: true),
        findsOneWidget,
      );
      expect(find.text('Work task'), findsOneWidget);
      expect(find.text('Main task'), findsNothing);
      // Remembered on this device for next launch.
      expect(await prefs.read(FleetStore.lastWorkspaceKey), 'Work');
    });

    testWidgets('LCARS opens the picker as a sheet', (tester) async {
      seedTwoWorkspaces();
      await pumpPhone(tester, tokens: lcarsTokens);

      expect(
        find.textContaining('FLEET · MAIN', findRichText: true),
        findsOneWidget,
      );
      await tester.tap(find.byKey(chromeTitleMenuKey));
      await tester.pumpAndSettle();
      expect(find.byType(BottomSheet), findsOneWidget);
      await tester.tap(find.text('WORK'));
      await tester.pumpAndSettle();
      expect(fleet.activeWorkspace, 'Work');
    });

    testWidgets('the Recent view is scoped too', (tester) async {
      seedTwoWorkspaces();
      await pumpPhone(tester);
      await tester.tap(find.text('Recent'));
      await tester.pumpAndSettle();
      expect(find.text('Main task'), findsOneWidget);
      expect(find.text('Work task'), findsNothing);
    });

    testWidgets('the Activity feed shows the active workspace only', (
      tester,
    ) async {
      seedTwoWorkspaces();
      await fleet.selectWorkspace('Work');
      useSize(tester, const Size(430, 900));
      unawaited(store.connect());
      await tester.pumpWidget(host(const Scaffold(body: ActivityBody())));
      await tester.pumpAndSettle();
      expect(find.text('Work task'), findsWidgets);
      expect(find.text('Main task'), findsNothing);
      expect(
        find.textContaining('Activity · Work', findRichText: true),
        findsOneWidget,
      );
    });

    testWidgets('the wide fleet header carries the switcher, and a switch '
        'clears the selection', (tester) async {
      seedTwoWorkspaces();
      useSize(tester, const Size(1400, 900));
      unawaited(store.connect());
      await tester.pumpWidget(host(const AdaptiveShell()));
      await tester.pumpAndSettle();

      expect(
        find.textContaining('Fleet · Main', findRichText: true),
        findsOneWidget,
      );
      await tester.tap(find.text('Main task'));
      await tester.pumpAndSettle();
      expect(find.text('Select a session'), findsNothing);

      await fleet.selectWorkspace('Work');
      await tester.pumpAndSettle();
      expect(find.text('Select a session'), findsOneWidget);
      expect(find.text('Work task'), findsOneWidget);
    });
  });

  group('new things land in the active workspace', () {
    Future<void> connectWithTwo() async {
      seedTwoWorkspaces();
      await store.connect();
    }

    testWidgets('the create-session picker offers the active workspace\'s '
        'projects only', (tester) async {
      await connectWithTwo();
      await tester.pumpWidget(
        MaterialApp(
          home: CreateSessionPage(store: store, workspace: 'Work'),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('work-repo'));
      await tester.pumpAndSettle();
      expect(find.text('main-repo'), findsNothing);
    });

    testWidgets('a workspace with no projects here says so', (tester) async {
      await connectWithTwo();
      await tester.pumpWidget(
        MaterialApp(
          home: CreateSessionPage(store: store, workspace: 'Elsewhere'),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        find.text('No projects in Elsewhere on this server'),
        findsOneWidget,
      );
    });

    testWidgets('adding a project by path tags it with the workspace', (
      tester,
    ) async {
      await connectWithTwo();
      await tester.pumpWidget(
        MaterialApp(
          home: ProjectsPage(store: store, workspace: 'Work'),
        ),
      );
      await tester.pumpAndSettle();
      // Every project is listed, each naming its workspace.
      expect(find.textContaining('· Work'), findsOneWidget);

      await tester.tap(find.byTooltip('Add project'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Add existing path'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), '/srv/new');
      await tester.tap(find.text('OK'));
      await tester.pumpAndSettle();

      expect(
        api.lastCall('addProject')!.args,
        allOf(
          containsPair('path', '/srv/new'),
          containsPair('workspace', 'Work'),
        ),
      );
    });

    testWidgets('a clone carries the workspace in its request', (tester) async {
      await connectWithTwo();
      api.cloneJobResponse = cloneJob(
        status: CloneStatusDto(
          kind: CloneStatusKind.succeeded,
          projectId: ProjectId(field0: projectInfo().id.field0),
          message: '',
          isGitRepo: false,
        ),
      );
      api.githubReposResponse = [githubRepo(owner: 'acme', name: 'widget')];
      await tester.pumpWidget(
        MaterialApp(
          home: CommanderStoreScope(
            store: store,
            child: CloneRepoPage(store: store, workspace: 'Work'),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('acme/widget'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('clone-confirm-button')));
      await tester.pumpAndSettle();

      final req =
          api.lastCall('startClone')!.args['request']! as CloneRequestDto;
      expect(req.workspace, 'Work');
    });
  });

  testWidgets('openCreateSession hands the page the active workspace', (
    tester,
  ) async {
    seedTwoWorkspaces();
    await store.connect();
    await fleet.selectWorkspace('Work');
    await tester.pumpWidget(
      host(
        Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () => openCreateSession(context, fleet),
              child: const Text('go'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('go'));
    await tester.pumpAndSettle();
    final page = tester.widget<CreateSessionPage>(
      find.byType(CreateSessionPage),
    );
    expect(page.workspace, 'Work');
  });
}
