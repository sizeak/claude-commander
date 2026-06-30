import 'package:claude_commander_client/main.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('shows the connection form when no config is saved', (
    tester,
  ) async {
    await tester.pumpWidget(const CommanderApp(initialConfig: null));
    expect(find.text('Connect to server'), findsOneWidget);
    expect(find.widgetWithText(TextFormField, 'Server URL'), findsOneWidget);
    expect(find.text('Save & connect'), findsOneWidget);
  });
}
