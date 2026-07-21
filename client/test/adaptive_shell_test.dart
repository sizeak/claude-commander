import 'dart:async';

import 'package:claude_commander_client/pages/adaptive_shell.dart';
import 'package:claude_commander_client/pages/session_detail_page.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
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

  testWidgets(
    'wide layout is master-detail; selecting a session updates the pane in '
    'place without a route push',
    (tester) async {
      useSize(tester, const Size(1400, 900));
      api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
      api.getSessionDetailResponse = sessionDetail(
        info: sessionInfo(title: 'Alpha'),
      );
      unawaited(store.connect());
      await tester.pumpWidget(wrap());
      await tester.pumpAndSettle();

      // Master list is present alongside an empty detail pane.
      expect(find.byType(SessionListBody), findsOneWidget);
      expect(find.text('Select a session'), findsOneWidget);
      expect(find.byType(SessionDetailBody), findsNothing);

      // Selecting a session fills the detail pane in place — no route was pushed.
      await tester.tap(find.text('Alpha'));
      await tester.pumpAndSettle();

      expect(find.byType(SessionDetailBody), findsOneWidget);
      expect(find.text('Select a session'), findsNothing);
      // The wide layout updates the pane, it does not push the phone detail page.
      expect(find.byType(SessionDetailPage), findsNothing);
    },
  );

  testWidgets('narrow layout uses the stacked push flow', (tester) async {
    useSize(tester, const Size(500, 900));
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    api.getSessionDetailResponse = sessionDetail(
      info: sessionInfo(title: 'Alpha'),
    );
    unawaited(store.connect());
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    // The narrow shell is the phone list page — no persistent detail pane.
    expect(find.byType(SessionListPage), findsOneWidget);
    expect(find.text('Select a session'), findsNothing);
    expect(find.byType(SessionDetailPage), findsNothing);

    // Tapping a row pushes the detail route (stacked navigation).
    await tester.tap(find.text('Alpha'));
    await tester.pumpAndSettle();
    expect(find.byType(SessionDetailPage), findsOneWidget);
  });
}
