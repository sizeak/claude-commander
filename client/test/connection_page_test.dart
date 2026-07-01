import 'package:claude_commander_client/pages/connection_page.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';

void main() {
  late FakeCommanderApi api;
  late InMemoryServerConfigStore store;

  setUp(() {
    api = FakeCommanderApi();
    store = InMemoryServerConfigStore();
  });

  Widget wrap() => MaterialApp(
    home: ConnectionPage(api: api, store: store),
  );

  testWidgets('an empty URL blocks save (form validation, no nav)', (
    tester,
  ) async {
    await tester.pumpWidget(wrap());
    // Clear the pre-filled URL so the field is empty.
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Server URL'),
      '',
    );
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Bearer token'),
      'tok',
    );
    await tester.tap(find.text('Save & connect'));
    await tester.pumpAndSettle();

    expect(find.text('Required'), findsOneWidget);
    expect(api.countOf('health'), 0);
    expect(find.byType(SessionListPage), findsNothing);
  });

  testWidgets('a failing health shows an error and does not navigate', (
    tester,
  ) async {
    api.healthResponse = false;
    await tester.pumpWidget(wrap());
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Bearer token'),
      'tok',
    );
    await tester.tap(find.text('Test connection'));
    await tester.pumpAndSettle();

    expect(find.textContaining('did not return OK'), findsOneWidget);
    expect(find.byType(SessionListPage), findsNothing);
    // Test never saves.
    expect(await store.load(), isNull);
  });

  testWidgets('success saves the config and navigates to the list', (
    tester,
  ) async {
    api.healthResponse = true;
    api.healthTmuxResponse = true;
    await tester.pumpWidget(wrap());
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Server URL'),
      'http://example.test:7878',
    );
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Bearer token'),
      'secret',
    );
    await tester.tap(find.text('Save & connect'));
    await tester.pumpAndSettle();

    expect(find.byType(SessionListPage), findsOneWidget);
    final saved = await store.load();
    expect(saved, isNotNull);
    expect(saved!.baseUrl, 'http://example.test:7878');
    expect(saved.token, 'secret');
  });
}
