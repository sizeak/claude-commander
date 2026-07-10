import 'package:claude_commander_client/pages/projects_page.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/src/rust/api/simple.dart'
    show ScanResultDto;
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;
  late CommanderStore store;

  setUp(() {
    api = FakeCommanderApi();
    store = CommanderStore(api: api, config: testConfig);
  });

  tearDown(() => store.dispose());

  Widget wrap() => MaterialApp(home: ProjectsPage(store: store));

  /// Connect the store (so the page has a live handle + workspace), then pump.
  Future<void> pump(WidgetTester tester) async {
    await store.connect();
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();
  }

  Future<void> enterPathAndConfirm(WidgetTester tester, String path) async {
    await tester.enterText(find.byType(TextField), path);
    await tester.tap(
      find.descendant(
        of: find.byType(AlertDialog),
        matching: find.widgetWithText(FilledButton, 'OK'),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('renders the current projects', (tester) async {
    api.projectsResponse = [
      projectInfo(name: 'my-repo', repoPath: '/srv/repos/my-repo'),
    ];
    await pump(tester);

    expect(find.text('my-repo'), findsOneWidget);
    expect(find.text('/srv/repos/my-repo'), findsOneWidget);
  });

  testWidgets('renders the empty state with no projects', (tester) async {
    await pump(tester);

    expect(find.textContaining('No projects'), findsOneWidget);
  });

  testWidgets('adding a project calls addProject then refreshes', (
    tester,
  ) async {
    await pump(tester);
    final refreshesBefore = api.countOf('workspaceSnapshot');

    await tester.tap(find.byType(FloatingActionButton));
    await tester.pumpAndSettle();
    await enterPathAndConfirm(tester, '/srv/repos/new');

    expect(api.countOf('addProject'), 1);
    expect(api.lastCall('addProject')!.args['path'], '/srv/repos/new');
    // A refresh follows the add so the new project shows without a manual pull.
    expect(api.countOf('workspaceSnapshot'), greaterThan(refreshesBefore));
  });

  testWidgets('scanning a directory calls scanDirectory and reports counts', (
    tester,
  ) async {
    api.scanDirectoryResponse = const ScanResultDto(added: 2, skipped: 1);
    await pump(tester);

    await tester.tap(find.byTooltip('Scan directory'));
    await tester.pumpAndSettle();
    await enterPathAndConfirm(tester, '/srv/repos');

    expect(api.countOf('scanDirectory'), 1);
    expect(api.lastCall('scanDirectory')!.args['path'], '/srv/repos');
    expect(find.textContaining('Added 2, skipped 1'), findsOneWidget);
  });

  testWidgets('removing a project confirms then calls removeProject', (
    tester,
  ) async {
    api.projectsResponse = [
      projectInfo(id: 'bbbbbbbb-2222-3333-4444-555555555555', name: 'gone'),
    ];
    await pump(tester);

    await tester.tap(find.byTooltip('Remove'));
    await tester.pumpAndSettle();
    // Confirm dialog is up.
    await tester.tap(
      find.descendant(
        of: find.byType(AlertDialog),
        matching: find.widgetWithText(FilledButton, 'Remove'),
      ),
    );
    await tester.pumpAndSettle();

    expect(api.countOf('removeProject'), 1);
    expect(
      api.lastCall('removeProject')!.args['id'],
      'bbbbbbbb-2222-3333-4444-555555555555',
    );
  });

  testWidgets('expanding a project loads its branches', (tester) async {
    api.projectsResponse = [projectInfo(name: 'my-repo')];
    api.listBranchesResponse = const [
      BranchInfo(name: 'main', isRemote: false),
      BranchInfo(name: 'origin/feature', isRemote: true),
    ];
    await pump(tester);

    await tester.tap(find.text('my-repo'));
    await tester.pumpAndSettle();

    expect(api.countOf('listBranches'), 1);
    expect(api.lastCall('listBranches')!.args['fetch'], false);
    expect(find.text('main'), findsOneWidget);
    expect(find.text('origin/feature'), findsOneWidget);
  });
}
