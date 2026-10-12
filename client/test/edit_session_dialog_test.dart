import 'package:claude_commander_client/pages/edit_session_dialog.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fixtures.dart';

Future<void> openEditor(
  WidgetTester tester, {
  required SessionInfo session,
  required List<ProgramInfo> programs,
  void Function(SessionEdit?)? onSaved,
}) async {
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              final result = await showDialog<SessionEdit>(
                context: context,
                builder: (_) => EditSessionDialog(
                  session: session,
                  peers: const [],
                  options: Future.value(
                    CreateOptions(
                      defaultProgram: 'claude',
                      programs: programs,
                      sections: const [],
                    ),
                  ),
                ),
              );
              onSaved?.call(result);
            },
            child: const Text('Edit'),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('Edit'));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'unconfigured live program remains selectable when a change is pending',
    (tester) async {
      SessionEdit? result;
      await openEditor(
        tester,
        session: sessionInfo(
          program: 'custom-agent --flag',
          pendingProgram: 'codex',
        ),
        programs: const [ProgramInfo(label: 'Codex alias', command: 'codex')],
        onSaved: (value) => result = value,
      );
      expect(find.text('Codex alias'), findsOneWidget);
      await tester.tap(
        find.widgetWithText(DropdownButtonFormField<String>, 'Program'),
      );
      await tester.pumpAndSettle();
      expect(find.text('custom-agent --flag'), findsOneWidget);
      await tester.tap(find.text('custom-agent --flag'));
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(FilledButton, 'Save'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('No, save for next restart'));
      await tester.pumpAndSettle();
      expect(result?.program, 'custom-agent --flag');
      expect(result?.restart, false);
    },
  );

  testWidgets('program selector shows aliases and saves the selected command', (
    tester,
  ) async {
    SessionEdit? result;
    await openEditor(
      tester,
      session: sessionInfo(program: 'claude --model opus'),
      programs: const [
        ProgramInfo(label: 'Claude alias', command: 'claude --model opus'),
        ProgramInfo(
          label: 'Codex alias',
          command: 'codex --sandbox workspace-write',
        ),
      ],
      onSaved: (value) => result = value,
    );
    final programField = find.widgetWithText(
      DropdownButtonFormField<String>,
      'Program',
    );
    expect(programField, findsOneWidget);
    expect(find.widgetWithText(TextFormField, 'Program'), findsNothing);
    expect(find.text('Configured programs'), findsNothing);
    expect(find.text('Claude alias'), findsOneWidget);
    expect(find.text('claude --model opus'), findsNothing);
    await tester.tap(programField);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Codex alias').last);
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Save'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('No, save for next restart'));
    await tester.pumpAndSettle();
    expect(result?.program, 'codex --sandbox workspace-write');
    expect(result?.restart, false);
  });

  testWidgets(
    'pending program alias is selected and saving does not warn again',
    (tester) async {
      SessionEdit? result;
      await openEditor(
        tester,
        session: sessionInfo(
          program: 'claude',
          pendingProgram: 'codex --sandbox workspace-write',
        ),
        programs: const [
          ProgramInfo(label: 'Claude alias', command: 'claude'),
          ProgramInfo(
            label: 'Codex alias',
            command: 'codex --sandbox workspace-write',
          ),
        ],
        onSaved: (value) => result = value,
      );
      expect(find.text('Codex alias'), findsOneWidget);
      await tester.tap(find.widgetWithText(FilledButton, 'Save'));
      await tester.pumpAndSettle();
      expect(find.text('Restart session?'), findsNothing);
      expect(result?.program, 'codex --sandbox workspace-write');
      expect(result?.restart, false);
    },
  );

  testWidgets(
    'unconfigured current program is retained and duplicate commands are safe',
    (tester) async {
      SessionEdit? result;
      await openEditor(
        tester,
        session: sessionInfo(program: 'custom-agent --flag'),
        programs: const [
          ProgramInfo(label: 'Claude alias', command: 'claude'),
          ProgramInfo(label: 'Another alias', command: 'claude'),
        ],
        onSaved: (value) => result = value,
      );
      expect(tester.takeException(), isNull);
      expect(find.text('custom-agent --flag'), findsOneWidget);
      final programField = find.widgetWithText(
        DropdownButtonFormField<String>,
        'Program',
      );
      await tester.tap(programField);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Claude alias').last);
      await tester.pumpAndSettle();
      await tester.tap(programField);
      await tester.pumpAndSettle();
      await tester.tap(find.text('custom-agent --flag').last);
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(FilledButton, 'Save'));
      await tester.pumpAndSettle();
      expect(result?.program, 'custom-agent --flag');
      expect(result?.restart, false);
    },
  );

  testWidgets('long session choices fit the edit dialog on a phone', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(393, 852));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    const section = 'A section with a very long descriptive name';
    const baseTitle = 'A parent session with a very long descriptive name';
    const baseBranch = 'a-parent-session-with-a-very-long-descriptive-name';
    final session = sessionInfo(program: 'codex', sectionOverride: section);
    final parent = sessionInfo(
      id: 'aaaaaaaa-2222-3333-4444-555555555555',
      projectId: session.projectId.field0.toString(),
      title: baseTitle,
      branch: baseBranch,
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () => showDialog<void>(
                context: context,
                builder: (_) => EditSessionDialog(
                  session: session,
                  peers: [session, parent],
                  options: Future.value(
                    const CreateOptions(
                      defaultProgram: 'claude',
                      programs: [
                        ProgramInfo(
                          label: 'A configured program with a very long label',
                          command: 'codex',
                        ),
                      ],
                      sections: [section],
                    ),
                  ),
                ),
              ),
              child: const Text('Edit'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Edit'));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('Project base'));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('$baseTitle ($baseBranch)').last);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });
}
