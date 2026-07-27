import 'dart:async';

import 'package:claude_commander_client/pages/create_session_page.dart';
import 'package:claude_commander_client/pages/phone_shell.dart';
import 'package:claude_commander_client/pages/session_detail_page.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
import 'package:claude_commander_client/widgets/brand_mark.dart';
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
    child: const MaterialApp(home: PhoneShell()),
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
}
