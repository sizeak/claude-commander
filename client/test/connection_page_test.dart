import 'package:claude_commander_client/pages/connection_page.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';

void main() {
  late FakeCommanderApi api;
  ServerConfig? submitted;

  setUp(() {
    api = FakeCommanderApi();
    submitted = null;
  });

  Widget wrap({ServerConfig? existing}) => MaterialApp(
    home: ConnectionPage(
      api: api,
      existing: existing,
      onSubmit: (cfg) async => submitted = cfg,
    ),
  );

  testWidgets('an empty URL blocks save (form validation, no submit)', (
    tester,
  ) async {
    await tester.pumpWidget(wrap());
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Server URL'),
      '',
    );
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Bearer token'),
      'tok',
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Add server'));
    await tester.pumpAndSettle();

    expect(find.text('Required'), findsOneWidget);
    expect(submitted, isNull);
  });

  testWidgets('a failing health shows an error (test probe never submits)', (
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
    expect(submitted, isNull);
  });

  testWidgets('a successful probe submits the assembled config', (tester) async {
    api.healthResponse = true;
    api.healthTmuxResponse = true;
    await tester.pumpWidget(wrap());
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Name'),
      'laptop',
    );
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Server URL'),
      'http://example.test:7878',
    );
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Bearer token'),
      'secret',
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Add server'));
    await tester.pumpAndSettle();

    expect(submitted, isNotNull);
    expect(submitted!.name, 'laptop');
    expect(submitted!.baseUrl, 'http://example.test:7878');
    expect(submitted!.token, 'secret');
    expect(submitted!.id, isNotEmpty);
  });

  testWidgets('a failed probe offers "Save anyway", which submits', (
    tester,
  ) async {
    api.healthResponse = false; // probe will fail
    await tester.pumpWidget(wrap());
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Server URL'),
      'http://down.test:7878',
    );
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Bearer token'),
      'secret',
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Add server'));
    // Explicit pumps (not pumpAndSettle): while the confirm dialog is open the
    // save button shows a spinner (_busy), an animation that never settles.
    await tester.pump(); // start _save → probe
    await tester.pump(const Duration(milliseconds: 50)); // probe resolves, dialog opens

    // The confirm dialog appears; nothing submitted yet.
    expect(find.text('Connection test failed'), findsOneWidget);
    expect(submitted, isNull);

    await tester.tap(find.widgetWithText(FilledButton, 'Save anyway'));
    await tester.pump(); // dialog pops, onSubmit runs
    await tester.pump(const Duration(milliseconds: 50));

    expect(submitted, isNotNull);
    // Name defaulted from the host when none was typed.
    expect(submitted!.name, 'down.test:7878');
    expect(submitted!.baseUrl, 'http://down.test:7878');
  });

  testWidgets('editing preserves the server id', (tester) async {
    api.healthResponse = true;
    api.healthTmuxResponse = true;
    await tester.pumpWidget(
      wrap(
        existing: const ServerConfig(
          id: 'keep-me',
          name: 'laptop',
          baseUrl: 'http://a:7878',
          token: 'tok',
        ),
      ),
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Save'));
    await tester.pumpAndSettle();

    expect(submitted!.id, 'keep-me');
  });
}
