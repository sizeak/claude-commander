import 'package:claude_commander_client/main.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';

void main() {
  testWidgets('shows the add-server form when no server is configured', (
    tester,
  ) async {
    final workspace = WorkspaceStore(
      api: FakeCommanderApi(),
      listStore: InMemoryServerListStore(),
    );
    await workspace.loadAndConnectAll();
    await tester.pumpWidget(
      CommanderApp(api: FakeCommanderApi(), workspace: workspace),
    );

    expect(find.text('Connect to a server'), findsOneWidget);
    expect(find.byKey(const Key('urlField')), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Connect'), findsOneWidget);
  });
}
