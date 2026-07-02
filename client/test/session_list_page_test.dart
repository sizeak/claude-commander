import 'package:claude_commander_client/pages/create_session_page.dart';
import 'package:claude_commander_client/pages/session_detail_page.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;
  late InMemoryServerConfigStore store;

  setUp(() {
    api = FakeCommanderApi();
    store = InMemoryServerConfigStore();
  });

  Widget wrap() => MaterialApp(
    home: SessionListPage(api: api, store: store, config: testConfig),
  );

  testWidgets('shows a loading indicator until the list resolves', (
    tester,
  ) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    await tester.pumpWidget(wrap());
    // First frame: the fake's Future hasn't completed yet, so the FutureBuilder
    // is still in the waiting state.
    expect(find.byType(CircularProgressIndicator), findsOneWidget);
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
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.text('No sessions'), findsOneWidget);
  });

  testWidgets('renders the error state and offers retry', (tester) async {
    api.listSessionsError = Exception('boom');
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.textContaining('boom'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Retry'), findsOneWidget);
  });

  testWidgets('tapping a row pushes the detail route', (tester) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.text('Alpha'));
    await tester.pumpAndSettle();

    expect(find.byType(SessionDetailPage), findsOneWidget);
  });

  testWidgets('the FAB pushes the create route', (tester) async {
    api.listSessionsResponse = const [];
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.byType(FloatingActionButton));
    await tester.pumpAndSettle();

    expect(find.byType(CreateSessionPage), findsOneWidget);
  });
}
