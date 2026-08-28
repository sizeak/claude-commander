import 'dart:async';

import 'package:claude_commander_client/pages/session_detail_page.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
import 'package:claude_commander_client/util/session_filter.dart';
import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;
  late CommanderStore store;
  late WorkspaceStore workspace;

  setUp(() {
    // The page filters and ranks inside `build`, and the real scorer is in the
    // cdylib, which `flutter test` does not load. A subsequence stand-in is
    // enough for these tests: they assert which rows survive a query, not how
    // the shared scorer ranks them (that is covered by
    // `claude-commander-viewmodel`'s own tests).
    debugSessionScorer = (session, query) {
      final q = query.toLowerCase();
      for (final field in [session.title, session.branch, session.program]) {
        if (field.toLowerCase().contains(q)) return field.length;
      }
      return null;
    };
    addTearDown(() => debugSessionScorer = null);

    api = FakeCommanderApi();
    store = CommanderStore(api: api, config: testConfig);
    workspace = WorkspaceStore.withStores([store]);
  });

  tearDown(() => workspace.dispose());

  // Host the layout-agnostic [SessionListBody] directly (the phone/wide shells
  // that embed it are tested separately): a Scaffold + Builder so row taps can
  // push the detail route via the shared [openSessionDetail] helper.
  Widget wrap() => WorkspaceScope(
    workspace: workspace,
    child: MaterialApp(
      home: Scaffold(
        body: Builder(
          builder: (context) => SessionListBody(
            onSelect: (store, session) =>
                openSessionDetail(context, store, session),
          ),
        ),
      ),
    ),
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
    // A session with an open PR shows its PR badge; one without shows its state
    // word ('stopped') in the trailing slot.
    expect(find.textContaining('PR #7'), findsOneWidget);
    expect(find.text('stopped'), findsOneWidget);
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

    // Sentence-cased by `errorText`, which also drops Dart's 'Exception: '.
    expect(find.text('Boom'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Retry'), findsOneWidget);
  });

  /// The regression behind [errorText]: an unreachable server threw the
  /// bridge's `AnyhowException`, whose message is the Rust error's `Debug` form
  /// — a `Stack backtrace:` and ten `<unknown>` frames — and the list rendered
  /// it verbatim, filling the screen.
  testWidgets('an unreachable server shows one line, not a Rust backtrace', (
    tester,
  ) async {
    api.workspaceSnapshotError = AnyhowException(
      'backend unavailable: could not connect to server\n'
      '\n'
      'Stack backtrace:\n'
      '   0: <unknown>\n'
      '   1: <unknown>\n'
      '   2: __start_thread',
    );
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.text('Could not connect to server'), findsOneWidget);
    expect(find.textContaining('Stack backtrace'), findsNothing);
    expect(find.textContaining('AnyhowException'), findsNothing);
    expect(find.widgetWithText(FilledButton, 'Retry'), findsOneWidget);
  });

  testWidgets('surfaces a lone server\'s degraded connection', (tester) async {
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

    // A lone server has no group header, so its connection state shows in the
    // in-body status strip.
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

  testWidgets('an unread session shows the unread glyph', (tester) async {
    api.listSessionsResponse = [sessionInfo(title: 'Unread one', unread: true)];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    // The unread state renders the ◆ glyph via the shared session descriptor.
    expect(find.text('◆'), findsOneWidget);
  });

  testWidgets('a read session shows no unread glyph', (tester) async {
    api.listSessionsResponse = [sessionInfo(title: 'Read one')];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.text('◆'), findsNothing);
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
          child: MaterialApp(
            home: Scaffold(body: SessionListBody(onSelect: (_, _) {})),
          ),
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

  /// A degraded server's reason shares the group header with the server's own
  /// name. With the reason unbounded it took whatever it wanted and squeezed the
  /// name to a stub ("192…"), so the row named nothing. Asserted geometrically
  /// (the name's painted box is the wider of the two) rather than by golden —
  /// it is a layout rule, not a rasterisation.
  testWidgets('a degraded reason cannot squeeze out the server name', (
    tester,
  ) async {
    final apiA = FakeCommanderApi()..listSessionsResponse = const [];
    final apiB = FakeCommanderApi()..listSessionsResponse = const [];
    const longName = 'http://192.168.1.10:4900';
    final storeA = CommanderStore(
      api: apiA,
      config: const ServerConfig(
        id: 'a',
        name: longName,
        baseUrl: 'http://192.168.1.10:4900',
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
        child: MaterialApp(
          home: Scaffold(body: SessionListBody(onSelect: (_, _) {})),
        ),
      ),
    );
    await tester.pumpAndSettle();

    apiA.emitConnection(
      const ConnectionStateDto(
        kind: ConnectionStateKind.degraded,
        reason: 'backend unavailable: could not connect to server',
      ),
    );
    await tester.pumpAndSettle();

    final name = tester.getSize(find.text(longName));
    final note = tester.getSize(find.text('could not connect to server'));
    expect(name.width, greaterThan(note.width));
  });

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

  testWidgets('typing in the search box filters the list', (tester) async {
    api.listSessionsResponse = [
      sessionInfo(title: 'Alpha refactor'),
      sessionInfo(
        id: '99999999-2222-3333-4444-555555555555',
        title: 'Beta cleanup',
      ),
    ];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    // Both rows visible before searching.
    expect(find.text('Alpha refactor'), findsOneWidget);
    expect(find.text('Beta cleanup'), findsOneWidget);

    await tester.enterText(find.byType(TextField), 'alpha');
    await tester.pumpAndSettle();

    // The query fuzzy-filters in place: only the match survives.
    expect(find.text('Alpha refactor'), findsOneWidget);
    expect(find.text('Beta cleanup'), findsNothing);

    // A query that matches nothing shows the no-matches note.
    await tester.enterText(find.byType(TextField), 'zzzzz');
    await tester.pumpAndSettle();
    expect(find.text('No matches'), findsOneWidget);
  });

  testWidgets('the Recent view shows sessions newest-first', (tester) async {
    api.listSessionsResponse = [
      sessionInfo(
        id: '11111111-2222-3333-4444-555555555555',
        title: 'Older',
        lastAttachedAt: DateTime.utc(2026, 1, 1),
      ),
      sessionInfo(
        id: '22222222-2222-3333-4444-555555555555',
        title: 'Newer',
        lastAttachedAt: DateTime.utc(2026, 1, 5),
      ),
    ];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    // Switch to the Recent tab.
    await tester.tap(find.text('Recent'));
    await tester.pumpAndSettle();

    expect(find.text('Newer'), findsOneWidget);
    expect(find.text('Older'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('Newer')).dy,
      lessThan(tester.getTopLeft(find.text('Older')).dy),
    );
  });

  testWidgets('the Recent view keeps a never-attached session, ordered by its '
      'creation time', (tester) async {
    // A session created seconds ago has never been attached. Keyed on the attach
    // time alone it vanished from this tab entirely — so the session the user
    // had just created was missing from the list, with no way to attach it (and
    // so no way to make it appear).
    api.listSessionsResponse = [
      sessionInfo(
        id: '11111111-2222-3333-4444-555555555555',
        title: 'Attached',
        createdAt: DateTime.utc(2026, 1, 1),
        lastAttachedAt: DateTime.utc(2026, 1, 3),
      ),
      sessionInfo(
        id: '22222222-2222-3333-4444-555555555555',
        title: 'Just created',
        createdAt: DateTime.utc(2026, 1, 9),
        lastAttachedAt: null,
      ),
    ];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.text('Recent'));
    await tester.pumpAndSettle();

    expect(find.text('Just created'), findsOneWidget);
    expect(find.text('Attached'), findsOneWidget);
    // Its creation time is the most recent thing that happened, so it leads.
    expect(
      tester.getTopLeft(find.text('Just created')).dy,
      lessThan(tester.getTopLeft(find.text('Attached')).dy),
    );
  });

  testWidgets('a quick filter stops applying once its chip drops out of the '
      'row', (tester) async {
    final waiting = sessionInfo(
      id: '11111111-2222-3333-4444-555555555555',
      title: 'Waiting',
    );
    api.listSessionsResponse = [waiting];
    api.agentStatesResponse = AgentStatesSnapshotDto(
      states: [
        AgentStateEntryDto(
          sessionId: waiting.sessionId,
          state: AgentState.waitingForInput,
        ),
      ],
      commanderRunning: false,
    );
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    // Turn the needs-input filter on while there is something to show.
    await tester.tap(find.textContaining('needs input'));
    await tester.pumpAndSettle();
    expect(find.text('Waiting'), findsOneWidget);

    // The agent stops waiting and a new session shows up. The needs-input count
    // is now zero, so its chip is gone from the row — the only control that
    // could clear the filter. It must therefore stop filtering, rather than
    // hiding every session (including the one just created) for good.
    api.listSessionsResponse = [
      waiting,
      sessionInfo(
        id: '22222222-2222-3333-4444-555555555555',
        title: 'Just created',
      ),
    ];
    api.agentStatesResponse = const AgentStatesSnapshotDto(
      states: [],
      commanderRunning: false,
    );
    api.emitChange();
    await tester.pumpAndSettle();

    expect(find.textContaining('needs input'), findsNothing);
    expect(find.text('Just created'), findsOneWidget);
    expect(find.text('Waiting'), findsOneWidget);
    expect(find.text('No matches'), findsNothing);
  });

  testWidgets('the Recent view hides stopped sessions even if attached', (
    tester,
  ) async {
    api.listSessionsResponse = [
      sessionInfo(
        id: '11111111-2222-3333-4444-555555555555',
        title: 'Live',
        status: SessionStatus.running,
        lastAttachedAt: DateTime.utc(2026, 1, 1),
      ),
      sessionInfo(
        id: '22222222-2222-3333-4444-555555555555',
        title: 'Dead',
        status: SessionStatus.stopped,
        lastAttachedAt: DateTime.utc(2026, 1, 5),
      ),
    ];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.text('Recent'));
    await tester.pumpAndSettle();

    // Stopped is excluded despite being the more-recently-attached of the two.
    expect(find.text('Dead'), findsNothing);
    expect(find.text('Live'), findsOneWidget);
  });

  testWidgets('searching within the Recent tab filters it', (tester) async {
    api.listSessionsResponse = [
      sessionInfo(
        id: '11111111-2222-3333-4444-555555555555',
        title: 'Alpha',
        lastAttachedAt: DateTime.utc(2026, 1, 1),
      ),
      sessionInfo(
        id: '22222222-2222-3333-4444-555555555555',
        title: 'Beta',
        lastAttachedAt: DateTime.utc(2026, 1, 2),
      ),
    ];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.text('Recent'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'alpha');
    await tester.pumpAndSettle();

    expect(find.text('Alpha'), findsOneWidget);
    expect(find.text('Beta'), findsNothing);
  });

  testWidgets('the clear button empties the query and restores the list', (
    tester,
  ) async {
    api.listSessionsResponse = [
      sessionInfo(title: 'Alpha'),
      sessionInfo(id: '22222222-2222-3333-4444-555555555555', title: 'Beta'),
    ];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), 'alpha');
    await tester.pumpAndSettle();
    expect(find.text('Beta'), findsNothing);

    await tester.tap(find.byTooltip('Clear'));
    await tester.pumpAndSettle();

    // Field emptied and both rows are back.
    expect(find.byTooltip('Clear'), findsNothing);
    expect(find.text('Alpha'), findsOneWidget);
    expect(find.text('Beta'), findsOneWidget);
  });

  testWidgets('creating a session refreshes the list immediately', (
    tester,
  ) async {
    api.listSessionsResponse = [sessionInfo(title: 'Existing')];
    api.createSessionResponse = 'created-1';
    await store.connect();
    await tester.pumpWidget(
      WorkspaceScope(
        workspace: workspace,
        child: MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: SessionListBody(onSelect: (_, _) {}),
              floatingActionButton: FloatingActionButton(
                onPressed: () => openCreateSession(context, workspace),
                child: const Icon(Icons.add),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.byType(FloatingActionButton));
    await tester.pumpAndSettle();

    // The server will report the new session on the next snapshot fetch. (Its
    // own project, as the fake synthesizes one per session — hence the distinct
    // name, so the create page's picker keeps unique values while it pops.)
    api.listSessionsResponse = [
      sessionInfo(title: 'Existing'),
      sessionInfo(
        id: '22222222-2222-3333-4444-555555555555',
        title: 'Just created',
        projectName: 'other-repo',
      ),
    ];
    final before = api.countOf('workspaceSnapshot');
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Title'),
      'Just created',
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
    await tester.pumpAndSettle();

    // No change-feed tick has fired: without an explicit refetch the list would
    // still be showing the pre-create snapshot, and the session the user just
    // made would look lost until the next poll.
    expect(api.countOf('workspaceSnapshot'), greaterThan(before));
    expect(find.text('Just created'), findsOneWidget);
  });

  testWidgets('the Recent tab shows a spinner while the server is loading', (
    tester,
  ) async {
    api.listSessionsResponse = const [];
    // Pump before connecting: no snapshot yet.
    await tester.pumpWidget(wrap());
    await tester.tap(find.text('Recent'));
    await tester.pump();

    // The loading state surfaces on Recent too, not a bare empty note.
    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(find.text('No recent sessions'), findsNothing);

    await store.connect();
    await tester.pumpAndSettle();
  });
}
