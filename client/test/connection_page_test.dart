import 'package:claude_commander_client/pages/connection_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';

void main() {
  late FakeCommanderApi api;
  late InMemoryServerConfigStore store;
  ServerConfig? connectedWith;

  setUp(() {
    api = FakeCommanderApi();
    store = InMemoryServerConfigStore();
    connectedWith = null;
  });

  Widget wrap() => MaterialApp(
    home: ConnectionPage(
      api: api,
      store: store,
      onConnected: (cfg) async => connectedWith = cfg,
    ),
  );

  testWidgets('an empty URL blocks save (form validation, no connect)', (
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
    expect(connectedWith, isNull);
    expect(await store.load(), isNull);
  });

  testWidgets('a failing health shows an error', (tester) async {
    api.healthResponse = false;
    await tester.pumpWidget(wrap());
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Bearer token'),
      'tok',
    );
    await tester.tap(find.text('Test connection'));
    await tester.pumpAndSettle();

    expect(find.textContaining('did not return OK'), findsOneWidget);
    // A test-connection probe never saves or connects.
    expect(await store.load(), isNull);
    expect(connectedWith, isNull);
  });

  testWidgets('success saves the config and hands off to onConnected', (
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

    final saved = await store.load();
    expect(saved, isNotNull);
    expect(saved!.baseUrl, 'http://example.test:7878');
    expect(saved.token, 'secret');
    // The app (not the page) owns the handle; the page just hands off the config.
    expect(connectedWith?.baseUrl, 'http://example.test:7878');
    expect(connectedWith?.token, 'secret');
  });
}
