import 'package:claude_commander_client/pages/create_session_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;

  setUp(() => api = FakeCommanderApi());

  testWidgets('blank project path / title gate the submit', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: CreateSessionPage(api: api, config: testConfig),
      ),
    );

    // Both required fields empty → validation fails, no create call.
    await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
    await tester.pumpAndSettle();

    expect(find.text('Required'), findsNWidgets(2));
    expect(api.countOf('createSession'), 0);
  });

  testWidgets('a successful create pops with the new id', (tester) async {
    api.createSessionResponse = 'created-123';
    String? poppedId;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () async {
                  poppedId = await Navigator.of(context).push<String>(
                    MaterialPageRoute(
                      builder: (_) =>
                          CreateSessionPage(api: api, config: testConfig),
                    ),
                  );
                },
                child: const Text('open'),
              ),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();

    await tester.enterText(
      find.widgetWithText(TextFormField, 'Project path (on the server)'),
      '/home/me/repo',
    );
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Title'),
      'My work',
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
    await tester.pumpAndSettle();

    expect(api.countOf('createSession'), 1);
    expect(api.lastCall('createSession')!.args['projectPath'], '/home/me/repo');
    expect(api.lastCall('createSession')!.args['title'], 'My work');
    expect(poppedId, 'created-123');
    // The page popped, so it's gone.
    expect(find.byType(CreateSessionPage), findsNothing);
  });
}
