import 'package:claude_commander_client/pages/edit_session_dialog.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fixtures.dart';

void main() {
  testWidgets('long session choices fit the edit dialog on a phone', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(393, 852));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    const section = 'A section with a very long descriptive name';
    const baseTitle = 'A parent session with a very long descriptive name';
    const baseBranch = 'a-parent-session-with-a-very-long-descriptive-name';
    final session = sessionInfo(program: 'claude', sectionOverride: section);
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
