import 'dart:async';

import 'package:claude_commander_client/pages/create_session_page.dart';
import 'package:claude_commander_client/pages/session_detail_page.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
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

  Widget wrap() => WorkspaceScope(
    workspace: workspace,
    child: const MaterialApp(home: SessionListPage()),
  );

  testWidgets('shows a loading indicator until the snapshot resolves', (
    tester,
  ) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    // Pump before connecting: no snapshot yet, so the spinner is up.
    await tester.pumpWidget(wrap());
    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    await store.connect();
    await tester.pumpAndSettle();
    // Once resolved, the spinner is gone and the row shows.
    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(find.text('Alpha'), findsOneWidget);
  });

  testWidgets('renders session rows with title and chips', (tester) async {
    api.listSessionsResponse = [
      sessionInfo(title: 'Alpha', status: SessionStatus.running, prNumber: 7),
      sessionInfo(
        id: '99999999-2222-3333-4444-555555555555',
        title: 'Beta',
        status: SessionStatus.stopped,
      ),
    ];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.text('Alpha'), findsOneWidget);
    expect(find.text('Beta'), findsOneWidget);
    expect(find.text('running'), findsOneWidget);
    expect(find.text('stopped'), findsOneWidget);
    expect(find.textContaining('PR #7'), findsOneWidget);
  });

  testWidgets('renders the empty state with no sessions', (tester) async {
    api.listSessionsResponse = const [];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.text('No sessions'), findsOneWidget);
  });

  testWidgets('renders the error state and offers retry', (tester) async {
    api.workspaceSnapshotError = Exception('boom');
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.textContaining('boom'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Retry'), findsOneWidget);
  });

  testWidgets('shows the connection state in the app bar', (tester) async {
    api.listSessionsResponse = const [];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    api.emitConnection(
      const ConnectionStateDto(
        kind: ConnectionStateKind.degraded,
        reason: 'flaky',
      ),
    );
    await tester.pumpAndSettle();

    expect(find.textContaining('Degraded: flaky'), findsOneWidget);
  });

  testWidgets('tapping a row pushes the detail route', (tester) async {
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

  testWidgets('an unread session shows the unread indicator', (tester) async {
    api.listSessionsResponse = [sessionInfo(title: 'Unread one', unread: true)];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.byIcon(Icons.circle), findsOneWidget);
  });

  testWidgets('a read session shows no unread indicator', (tester) async {
    api.listSessionsResponse = [sessionInfo(title: 'Read one')];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.byIcon(Icons.circle), findsNothing);
  });

  testWidgets('a paused cascade shows the resume/abandon banner', (
    tester,
  ) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    api.cascadePausedResponse = sessionInfo().sessionId;
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.textContaining('Cascade paused'), findsOneWidget);

    await tester.tap(find.widgetWithText(FilledButton, 'Resume'));
    await tester.pumpAndSettle();
    expect(api.countOf('cascadeResume'), 1);

    await tester.tap(find.widgetWithText(OutlinedButton, 'Abandon'));
    await tester.pumpAndSettle();
    expect(api.countOf('cascadeAbandon'), 1);
  });

  testWidgets('no banner when no cascade is paused', (tester) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.textContaining('Cascade paused'), findsNothing);
  });

  testWidgets(
    'with several servers, sessions are grouped under a header per server',
    (tester) async {
      final apiA = FakeCommanderApi()
        ..listSessionsResponse = [
          sessionInfo(
            id: '11111111-1111-1111-1111-111111111111',
            title: 'AlphaOnA',
            projectName: 'repo-a',
          ),
        ];
      final apiB = FakeCommanderApi()
        ..listSessionsResponse = [
          sessionInfo(
            id: '22222222-2222-2222-2222-222222222222',
            title: 'BetaOnB',
            projectName: 'repo-b',
          ),
        ];
      final storeA = CommanderStore(
        api: apiA,
        config: const ServerConfig(
          id: 'a',
          name: 'laptop',
          baseUrl: 'http://a:7878',
          token: 't',
        ),
      );
      final storeB = CommanderStore(
        api: apiB,
        config: const ServerConfig(
          id: 'b',
          name: 'codespace',
          baseUrl: 'http://b:7878',
          token: 't',
        ),
      );
      final ws = WorkspaceStore.withStores([storeA, storeB]);
      addTearDown(ws.dispose);
      unawaited(storeA.connect());
      unawaited(storeB.connect());

      await tester.pumpWidget(
        WorkspaceScope(
          workspace: ws,
          child: const MaterialApp(home: SessionListPage()),
        ),
      );
      await tester.pumpAndSettle();

      // Both server headers show, each with its own session.
      expect(find.text('laptop'), findsOneWidget);
      expect(find.text('codespace'), findsOneWidget);
      expect(find.text('AlphaOnA'), findsOneWidget);
      expect(find.text('BetaOnB'), findsOneWidget);
    },
  );

  testWidgets('a long program name does not overflow a narrow tile', (
    tester,
  ) async {
    api.listSessionsResponse = [
      sessionInfo(
        title: 'Alpha',
        program:
            'claude --dangerously-skip-permissions --resume --model opus-4-8',
      ),
    ];
    unawaited(store.connect());
    // Mimic the desktop master column's ~340px width — where an unconstrained
    // trailing Text used to throw "Trailing widget consumes the entire tile
    // width" for a long program string.
    await tester.pumpWidget(
      WorkspaceScope(
        workspace: workspace,
        child: MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 340,
              child: SessionListBody(onSelect: (_, _) {}),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(tester.takeException(), isNull);
    expect(find.text('Alpha'), findsOneWidget);
  });
}
