import 'package:claude_commander_client/pages/adaptive_shell.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/pages/settings_page.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:claude_commander_client/window/window_controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_commander_api.dart';
import '../support/fake_window_service.dart';
import '../support/fixtures.dart';
import '../support/golden.dart';

/// Reference images for the screens, in both themes.
///
/// Where the form goldens pin each chrome element on its own, these pin how a
/// screen composes them — the spacing between a group heading and its run, the
/// wide shell's column split, which pane owns the navigation. A form can be
/// perfect in isolation and still land wrong on a page.
///
/// The fixtures are deliberately static: no relative timestamps and no live
/// state, because a golden that renders "2m ago" is a test that fails at 3am.
void main() {
  late FakeCommanderApi api;
  late CommanderStore store;
  late WorkspaceStore workspace;

  // Two projects, so the list has more than one group and its headings appear.
  // Sessions group by project *id*, not name, hence the shared ids.
  const commander = '99999999-1111-1111-1111-111111111111';
  const conan = '88888888-1111-1111-1111-111111111111';

  setUp(() {
    api = FakeCommanderApi();
    api.listSessionsResponse = [
      sessionInfo(
        title: 'conversation-model',
        status: SessionStatus.stopped,
        projectId: commander,
        projectName: 'claude-commander',
      ),
      sessionInfo(
        id: '22222222-2222-3333-4444-555555555555',
        title: 'flutter-ful',
        branch: 'flutter-ful',
        projectId: commander,
        projectName: 'claude-commander',
      ),
      sessionInfo(
        id: '33333333-2222-3333-4444-555555555555',
        title: 'slack-1',
        prNumber: 231,
        projectId: commander,
        projectName: 'claude-commander',
      ),
      sessionInfo(
        id: '44444444-2222-3333-4444-555555555555',
        title: 'libspeex',
        projectId: conan,
        projectName: 'conan-center-index',
      ),
    ];
    store = CommanderStore(api: api, config: testConfig);
    workspace = WorkspaceStore.withStores([store]);
  });

  tearDown(() => workspace.dispose());

  /// Pumps [child] with a connected server behind it, in one theme.
  Future<void> pumpPage(
    WidgetTester tester, {
    required CommanderTokens tokens,
    required Widget child,
    required Size size,
    bool bareScaffold = true,
  }) async {
    await loadCommanderFonts();
    useGoldenSurface(tester, size);
    await store.connect();
    // Connected, not connecting: the transient state would put a "Connecting…"
    // banner over the list and a half-lit dot on the settings server row, so the
    // references would pin a state the user sees for a second at startup.
    api.emitConnection(
      const ConnectionStateDto(kind: ConnectionStateKind.connected, reason: ''),
    );
    final app = MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: themeDataFor(tokens),
      home: bareScaffold
          ? Scaffold(backgroundColor: tokens.canvas, body: child)
          : child,
    );
    await tester.pumpWidget(
      WorkspaceScope(
        workspace: workspace,
        // Both scopes, as `main()` mounts them: the settings screen reads the
        // theme for its row caption and the window controller for its WINDOW
        // section, and a null controller would silently drop that section.
        child: WindowScope(
          controller: WindowController(
            store: InMemoryPrefStore(),
            service: FakeWindowService(),
          ),
          child: ThemeScope(
            controller: ThemeController(store: InMemoryPrefStore()),
            child: app,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  void forEachTheme(
    String name,
    Size size,
    Widget Function() build, {
    bool bareScaffold = true,
  }) {
    goldenThemes.forEach((theme, tokens) {
      testWidgets('$name · $theme', (tester) async {
        await pumpPage(
          tester,
          tokens: tokens,
          size: size,
          child: build(),
          bareScaffold: bareScaffold,
        );
        await expectGolden(tester, '${name}_$theme');
      });
    });
  }

  // The fleet list: two project groups, a PR badge, and the state chips — the
  // densest arrangement of list rows in the app.
  forEachTheme(
    'session_list',
    const Size(420, 720),
    () => SessionListBody(onSelect: (_, _) {}),
  );

  // The settings screen, which is also the only visual coverage of the desktop
  // WINDOW section's rows and their state words.
  forEachTheme(
    'settings',
    const Size(420, 720),
    () => const SettingsPage(),
    bareScaffold: false,
  );

  // The wide shell, above the LCARS three-column threshold so both themes are at
  // their full desktop layout rather than the folded one.
  forEachTheme(
    'wide_shell',
    const Size(1400, 900),
    () => const AdaptiveShell(),
    bareScaffold: false,
  );
}
