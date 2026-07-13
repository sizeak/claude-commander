import 'package:claude_commander_client/pages/programs_page.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';

void main() {
  /// Pump a host route that pushes [ProgramsPage], so the page's save-then-pop
  /// returns to a real prior route (not an empty navigator).
  Future<void> pumpPrograms(WidgetTester tester, FakeCommanderApi api) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () => Navigator.of(context).push(
                  MaterialPageRoute(
                    builder: (_) => ProgramsPage(api: api, handle: 'h'),
                  ),
                ),
                child: const Text('go'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('go'));
    await tester.pumpAndSettle();
  }

  testWidgets('loads current programs and saves them back', (tester) async {
    final api = FakeCommanderApi()
      ..createOptionsResponse = const CreateOptions(
        defaultProgram: 'claude',
        programs: [
          ProgramInfo(label: 'Claude', command: 'claude'),
          ProgramInfo(label: 'Shell', command: 'bash'),
        ],
        sections: [],
      );

    await pumpPrograms(tester, api);

    // Both existing programs render as editable rows.
    expect(find.text('Claude'), findsOneWidget);
    expect(find.text('bash'), findsOneWidget);

    await tester.tap(find.byIcon(Icons.save));
    await tester.pumpAndSettle();

    expect(api.lastSetPrograms, isNotNull);
    expect(api.lastSetPrograms!.map((p) => p.label).toList(), [
      'Claude',
      'Shell',
    ]);
    // Returned to the host route after saving.
    expect(find.text('go'), findsOneWidget);
  });

  testWidgets('drops blank rows on save', (tester) async {
    final api = FakeCommanderApi()
      ..createOptionsResponse = const CreateOptions(
        defaultProgram: 'claude',
        programs: [ProgramInfo(label: 'Claude', command: 'claude')],
        sections: [],
      );

    await pumpPrograms(tester, api);

    // Add an empty row, then save without filling it.
    await tester.tap(find.byIcon(Icons.add));
    await tester.pumpAndSettle();
    await tester.tap(find.byIcon(Icons.save));
    await tester.pumpAndSettle();

    // The blank row is dropped — only the real program is saved.
    expect(api.lastSetPrograms!.length, 1);
    expect(api.lastSetPrograms!.single.label, 'Claude');
  });

  testWidgets('edits a program command before saving', (tester) async {
    final api = FakeCommanderApi()
      ..createOptionsResponse = const CreateOptions(
        defaultProgram: 'claude',
        programs: [ProgramInfo(label: 'Claude', command: 'claude')],
        sections: [],
      );

    await pumpPrograms(tester, api);

    // The command field carries the current value; replace it.
    final commandField = find.widgetWithText(TextField, 'claude');
    expect(commandField, findsOneWidget);
    await tester.enterText(commandField, 'claude --resume');
    await tester.tap(find.byIcon(Icons.save));
    await tester.pumpAndSettle();

    expect(api.lastSetPrograms!.single.command, 'claude --resume');
  });
}
