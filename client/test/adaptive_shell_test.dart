import 'dart:async';

import 'package:claude_commander_client/pages/activity_page.dart';
import 'package:claude_commander_client/pages/adaptive_shell.dart';
import 'package:claude_commander_client/pages/phone_shell.dart';
import 'package:claude_commander_client/pages/review_page.dart';
import 'package:claude_commander_client/pages/session_detail_page.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/pages/terminal_page.dart';
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
    child: const MaterialApp(home: AdaptiveShell()),
  );

  void useSize(WidgetTester tester, Size size) {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
  }

  /// Bring up the wide shell with a single session ready and connected.
  Future<void> pumpWide(WidgetTester tester) async {
    useSize(tester, const Size(1400, 900));
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    api.getSessionDetailResponse = sessionDetail(
      info: sessionInfo(title: 'Alpha'),
    );
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();
  }

  testWidgets('narrow layout is the phone shell', (tester) async {
    useSize(tester, const Size(500, 900));
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.byType(PhoneShell), findsOneWidget);
    // No persistent workspace pane in the narrow layout.
    expect(find.text('Select a session'), findsNothing);
    expect(find.byType(SessionDetailBody), findsNothing);
  });

  testWidgets(
    'wide layout is rail + workspace; the empty-state shows until a session '
    'is selected',
    (tester) async {
      await pumpWide(tester);

      // The rail carries the shared list and the empty workspace shows the
      // placeholder — no route push, no detail body yet.
      expect(find.byType(SessionListBody), findsOneWidget);
      expect(find.text('Select a session'), findsOneWidget);
      expect(find.byType(SessionDetailBody), findsNothing);
    },
  );

  testWidgets(
    'selecting a session opens its workspace on the Agent tab, in place',
    (tester) async {
      await pumpWide(tester);

      await tester.tap(find.text('Alpha'));
      await tester.pumpAndSettle();

      // The live agent terminal is shown in place, not the Overview body, and
      // no phone detail route was pushed.
      expect(find.byType(TerminalBody), findsOneWidget);
      expect(find.byType(SessionDetailBody), findsNothing);
      expect(find.byType(SessionDetailPage), findsNothing);
      expect(find.text('Select a session'), findsNothing);
      // The underline tab row is present.
      expect(find.byKey(const ValueKey('ws-tab-detail')), findsOneWidget);
      expect(find.byKey(const ValueKey('ws-tab-terminal')), findsOneWidget);
      expect(find.byKey(const ValueKey('ws-tab-shell')), findsOneWidget);
      expect(find.byKey(const ValueKey('ws-tab-review')), findsOneWidget);
      expect(find.text('Agent'), findsOneWidget);
    },
  );

  testWidgets('the Overview and Shell tabs switch bodies in place', (
    tester,
  ) async {
    await pumpWide(tester);
    await tester.tap(find.text('Alpha'));
    await tester.pumpAndSettle();

    // Overview tab → the detail body, off the default agent attach.
    await tester.tap(find.byKey(const ValueKey('ws-tab-detail')));
    await tester.pumpAndSettle();
    expect(find.byType(SessionDetailBody), findsOneWidget);
    expect(find.byType(TerminalBody), findsNothing);

    // Agent tab → back to an agent-pane attach.
    await tester.tap(find.byKey(const ValueKey('ws-tab-terminal')));
    await tester.pump();
    await tester.pump();
    expect(find.byType(TerminalBody), findsOneWidget);
    expect(find.byType(SessionDetailBody), findsNothing);

    // Shell tab → the paired shell attach.
    await tester.tap(find.byKey(const ValueKey('ws-tab-shell')));
    await tester.pump();
    await tester.pump();
    expect(find.byType(TerminalBody), findsOneWidget);
  });

  testWidgets('the Changes tab switches to the review body', (tester) async {
    await pumpWide(tester);
    api.openReviewResponse = reviewSnapshot(files: const []);

    await tester.tap(find.text('Alpha'));
    await tester.pumpAndSettle();

    await tester.tap(find.byKey(const ValueKey('ws-tab-review')));
    await tester.pumpAndSettle();

    expect(find.byType(ReviewBody), findsOneWidget);
    expect(find.byType(SessionDetailBody), findsNothing);
  });

  testWidgets(
    'the FLEET/ACTIVITY toggle swaps the workspace to the Activity feed while '
    'the rail stays visible',
    (tester) async {
      await pumpWide(tester);
      // A session is selected first, to prove ACTIVITY overrides the workspace.
      await tester.tap(find.text('Alpha'));
      await tester.pumpAndSettle();
      expect(find.byType(TerminalBody), findsOneWidget);

      await tester.tap(find.text('ACTIVITY'));
      await tester.pumpAndSettle();

      // Workspace is now the Activity feed; the rail (list) is still there.
      expect(find.byType(ActivityBody), findsOneWidget);
      expect(find.byType(TerminalBody), findsNothing);
      expect(find.byType(SessionListBody), findsOneWidget);

      // Toggling back to FLEET restores the selected session's workspace.
      await tester.tap(find.text('FLEET'));
      await tester.pumpAndSettle();
      expect(find.byType(ActivityBody), findsNothing);
      expect(find.byType(TerminalBody), findsOneWidget);
    },
  );

  testWidgets(
    'the wide workspace shows the terminal-snapshot preview and captures pane '
    'lines for it',
    (tester) async {
      useSize(tester, const Size(1400, 900));
      api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
      api.getSessionDetailResponse = sessionDetail(
        info: sessionInfo(title: 'Alpha'),
        paneContent: 'live pane text',
      );
      unawaited(store.connect());
      await tester.pumpWidget(wrap());
      await tester.pumpAndSettle();

      await tester.tap(find.text('Alpha'));
      await tester.pumpAndSettle();
      // The preview lives on Overview; selection lands on Agent by default.
      await tester.tap(find.byKey(const ValueKey('ws-tab-detail')));
      await tester.pumpAndSettle();

      expect(find.text('Terminal snapshot'), findsOneWidget);
      expect(find.text('live pane text'), findsOneWidget);
      expect(api.lastCall('getSessionDetail')!.args['lines'], 200);
    },
  );
}
