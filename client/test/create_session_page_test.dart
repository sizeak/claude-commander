import 'package:claude_commander_client/pages/create_session_page.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;

  setUp(() => api = FakeCommanderApi());

  /// A connected store whose workspace exposes [projects]. The create page reads
  /// its api/handle/projects from the store, so it must be connected first.
  Future<CommanderStore> connectedStore(List<ProjectInfoDto> projects) async {
    api.projectsResponse = projects;
    final store = CommanderStore(api: api, config: testConfig);
    addTearDown(store.dispose);
    await store.connect();
    return store;
  }

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

  Future<void> pumpPage(WidgetTester tester, CommanderStore store) async {
    await tester.pumpWidget(
      MaterialApp(home: CreateSessionPage(store: store)),
    );
    await tester.pumpAndSettle(); // let create-options load
  }

  testWidgets('no registered projects shows an empty state, not a form', (
    tester,
  ) async {
    final store = await connectedStore(const []);
    await pumpPage(tester, store);

    expect(find.textContaining('No projects'), findsOneWidget);
    // No form to fill in and nothing to submit.
    expect(find.widgetWithText(FilledButton, 'Create session'), findsNothing);
    expect(find.widgetWithText(TextFormField, 'Title'), findsNothing);
  });

  testWidgets('projects arriving after the page opens fill the picker in', (
    tester,
  ) async {
    // Open before any project is known (e.g. the workspace snapshot is still in
    // flight) — the empty state shows, not a broken form.
    final store = await connectedStore(const []);
    await pumpPage(tester, store);
    expect(find.textContaining('No projects'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Create session'), findsNothing);

    // The server now has a project; a change-feed tick refetches the snapshot,
    // and the reactive body swaps the empty state for the live form.
    api.projectsResponse = [
      projectInfo(name: 'late-repo', repoPath: '/srv/late'),
    ];
    api.emitChange();
    await tester.pumpAndSettle();

    expect(find.textContaining('No projects'), findsNothing);
    expect(find.text('late-repo'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Create session'), findsOneWidget);
  });

  testWidgets('the base branch and initial prompt fields are gone', (
    tester,
  ) async {
    final store = await connectedStore([projectInfo()]);
    await pumpPage(tester, store);

    expect(find.textContaining('Base branch'), findsNothing);
    expect(find.textContaining('Initial prompt'), findsNothing);
  });

  testWidgets('the project is picked from a dropdown, not typed', (
    tester,
  ) async {
    final store = await connectedStore([
      projectInfo(name: 'my-repo', repoPath: '/srv/repos/my-repo'),
    ]);
    await pumpPage(tester, store);

    // No free-text path field; the project name appears as the dropdown value.
    expect(
      find.widgetWithText(TextFormField, 'Project path (on the server)'),
      findsNothing,
    );
    expect(find.text('my-repo'), findsOneWidget);
  });

  testWidgets('a blank title gates the submit', (tester) async {
    final store = await connectedStore([projectInfo()]);
    await pumpPage(tester, store);

    // Project is preselected; only the (empty) title should block submission.
    await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
    await tester.pumpAndSettle();

    expect(find.text('Required'), findsOneWidget);
    expect(api.countOf('createSession'), 0);
  });

  testWidgets(
    'a successful create sends the picked project path (no base branch / prompt) '
    'and pops with the new id',
    (tester) async {
      api.createSessionResponse = 'created-123';
      final store = await connectedStore([
        projectInfo(name: 'my-repo', repoPath: '/srv/repos/my-repo'),
      ]);
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
                        builder: (_) => CreateSessionPage(store: store),
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
        find.widgetWithText(TextFormField, 'Title'),
        'My work',
      );
      await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
      await tester.pumpAndSettle();

      expect(api.countOf('createSession'), 1);
      final call = api.lastCall('createSession')!;
      expect(call.args['projectPath'], '/srv/repos/my-repo');
      expect(call.args['title'], 'My work');
      // The dropped fields are always sent as null now.
      expect(call.args['baseBranch'], isNull);
      expect(call.args['initialPrompt'], isNull);
      expect(poppedId, 'created-123');
      // The page popped, so it's gone.
      expect(find.byType(CreateSessionPage), findsNothing);
    },
  );

  testWidgets('choosing a different project sends its repo path', (
    tester,
  ) async {
    final store = await connectedStore([
      projectInfo(
        id: 'aaaaaaaa-2222-3333-4444-555555555555',
        name: 'alpha',
        repoPath: '/srv/alpha',
      ),
      projectInfo(
        id: 'bbbbbbbb-2222-3333-4444-555555555555',
        name: 'beta',
        repoPath: '/srv/beta',
      ),
    ]);
    await pumpPage(tester, store);

    // Open the project dropdown (preselected to the first project) and pick beta.
    await tester.tap(find.text('alpha'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('beta').last);
    await tester.pumpAndSettle();

    await tester.enterText(
      find.widgetWithText(TextFormField, 'Title'),
      'My work',
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
    await tester.pumpAndSettle();

    expect(api.lastCall('createSession')!.args['projectPath'], '/srv/beta');
  });

  testWidgets('program dropdown defaults to the server default and is sent', (
    tester,
  ) async {
    withOptions();
    final store = await connectedStore([projectInfo()]);
    await pumpPage(tester, store);

    // The dropdown shows program labels (not a free-text field), defaulted to
    // the server default (Shell = bash).
    expect(find.text('Shell'), findsOneWidget);

    await tester.enterText(
      find.widgetWithText(TextFormField, 'Title'),
      'My work',
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
    await tester.pumpAndSettle();

    expect(api.lastCall('createSession')!.args['program'], 'bash');
  });

  testWidgets('choosing a section applies it via setSection after create', (
    tester,
  ) async {
    withOptions();
    api.createSessionResponse = 'sess-1';
    final store = await connectedStore([projectInfo()]);
    await pumpPage(tester, store);

    await tester.enterText(
      find.widgetWithText(TextFormField, 'Title'),
      'My work',
    );

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
      final store = await connectedStore([projectInfo()]);
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
                        builder: (_) => CreateSessionPage(store: store),
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
        find.widgetWithText(TextFormField, 'Title'),
        'My work',
      );
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
