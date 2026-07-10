import 'package:claude_commander_client/pages/create_session_page.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;

  setUp(() => api = FakeCommanderApi());

  /// create-options with a couple of programs (default = Shell) and one section.
  void withOptions() {
    api.createOptionsResponse = const CreateOptions(
      defaultProgram: 'bash',
      programs: [
        ProgramInfo(label: 'Claude', command: 'claude'),
        ProgramInfo(label: 'Shell', command: 'bash'),
      ],
      sections: ['review'],
    );
  }

  Future<void> fillRequired(WidgetTester tester) async {
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Project path (on the server)'),
      '/home/me/repo',
    );
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Title'),
      'My work',
    );
  }

  testWidgets('blank project path / title gate the submit', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: CreateSessionPage(api: api, handle: testHandle),
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
                          CreateSessionPage(api: api, handle: testHandle),
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

  testWidgets('program dropdown defaults to the server default and is sent', (
    tester,
  ) async {
    withOptions();
    await tester.pumpWidget(
      MaterialApp(home: CreateSessionPage(api: api, handle: testHandle)),
    );
    await tester.pumpAndSettle(); // let create-options load

    // The dropdown shows program labels (not a free-text field), defaulted to
    // the server default (Shell = bash).
    expect(find.text('Shell'), findsOneWidget);

    await fillRequired(tester);
    await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
    await tester.pumpAndSettle();

    expect(api.lastCall('createSession')!.args['program'], 'bash');
  });

  testWidgets('choosing a section applies it via setSection after create', (
    tester,
  ) async {
    withOptions();
    api.createSessionResponse = 'sess-1';
    await tester.pumpWidget(
      MaterialApp(home: CreateSessionPage(api: api, handle: testHandle)),
    );
    await tester.pumpAndSettle();

    await fillRequired(tester);

    // Pick the 'review' section from the (default 'None') section dropdown.
    await tester.tap(find.text('None'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('review').last);
    await tester.pumpAndSettle();

    await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
    await tester.pumpAndSettle();

    expect(api.countOf('setSection'), 1);
    expect(api.lastCall('setSection')!.args['id'], 'sess-1');
    expect(api.lastCall('setSection')!.args['section'], 'review');
  });

  testWidgets(
    'a setSection failure after create is non-fatal: still pops, no re-create',
    (tester) async {
      withOptions();
      api.createSessionResponse = 'sess-1';
      // The session is created, but applying the section fails.
      api.setSectionError = Exception('bad section');
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
                            CreateSessionPage(api: api, handle: testHandle),
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

      await fillRequired(tester);
      // Pick a section so setSection runs (and fails).
      await tester.tap(find.text('None'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('review').last);
      await tester.pumpAndSettle();

      await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
      await tester.pumpAndSettle();

      // The session was created exactly once and committed: the page popped with
      // the id, and the section failure did NOT re-arm the form (no duplicate).
      expect(api.countOf('createSession'), 1);
      expect(poppedId, 'sess-1');
      expect(find.byType(CreateSessionPage), findsNothing);
    },
  );
}
