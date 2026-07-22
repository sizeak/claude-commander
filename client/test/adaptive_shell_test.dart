import 'dart:async';

import 'package:claude_commander_client/pages/adaptive_shell.dart';
import 'package:claude_commander_client/pages/session_detail_page.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;
  late InMemoryServerConfigStore configStore;
  late CommanderStore store;

  setUp(() {
    api = FakeCommanderApi();
    configStore = InMemoryServerConfigStore();
    store = CommanderStore(api: api, config: testConfig);
  });

  tearDown(() => store.dispose());

  Widget wrap() => CommanderStoreScope(
    store: store,
    child: MaterialApp(
      home: AdaptiveShell(configStore: configStore, onConnected: (_) async {}),
    ),
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

  testWidgets(
    'the wide detail pane shows the terminal-snapshot preview and captures '
    'pane lines for it',
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

      // The wide layout keeps the snapshot card...
      expect(find.text('Terminal snapshot'), findsOneWidget);
      expect(find.text('live pane text'), findsOneWidget);
      // ...and therefore asks the server to capture pane lines (200 = the
      // preview's capture depth; symmetric with the phone test's null).
      expect(api.lastCall('getSessionDetail')!.args['lines'], 200);
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
