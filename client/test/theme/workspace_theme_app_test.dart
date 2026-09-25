import 'dart:async';

import 'package:claude_commander_client/main.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/fleet_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_prefs.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_commander_api.dart';
import '../support/fixtures.dart';

const _red = Color(0xFFFF0000);

/// The app renders in the active workspace's theme, and switching workspace
/// swaps it live — crossfading colours in place when the chrome is the same,
/// re-inflating the shells only when it is not.
void main() {
  late FakeCommanderApi api;
  late CommanderStore store;
  late FleetStore fleet;
  late ThemeController theme;

  setUp(() {
    api = FakeCommanderApi()
      ..workspacesResponse = const [
        WorkspaceDef(name: 'Work'),
        WorkspaceDef(name: 'Retro'),
      ]
      ..listSessionsResponse = [sessionInfo(title: 'Alpha')];
    store = CommanderStore(api: api, config: testConfig);
    fleet = FleetStore.withStores([store], prefs: InMemoryPrefStore());
    theme = ThemeController(store: InMemoryPrefStore());
  });

  // No tearDown dispose: CommanderApp owns the fleet and disposes it itself.

  Future<void> pumpApp(WidgetTester tester) async {
    tester.view.physicalSize = const Size(500, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    unawaited(store.connect());
    await tester.pumpWidget(CommanderApp(api: api, fleet: fleet, theme: theme));
    await tester.pumpAndSettle();
  }

  CommanderTokens appTokens(WidgetTester tester) => Theme.of(
    tester.element(find.byType(SessionListBody)),
  ).extension<CommanderTokens>()!;

  testWidgets('switching workspace applies its theme live', (tester) async {
    await theme.setOverride('Work', ThemeRole.primary, _red);
    await pumpApp(tester);
    expect(appTokens(tester).primary, missionControlTokens.primary);

    await fleet.selectWorkspace('Work');
    await tester.pumpAndSettle();
    expect(theme.activeWorkspaceKey, 'Work');
    expect(appTokens(tester).primary, _red);

    await fleet.selectWorkspace(null);
    await tester.pumpAndSettle();
    expect(theme.activeWorkspaceKey, mainWorkspaceThemeKey);
    expect(appTokens(tester).primary, missionControlTokens.primary);
  });

  testWidgets('a same-chrome switch keeps the shells; a chrome change '
      're-inflates them', (tester) async {
    await theme.setOverride('Work', ThemeRole.primary, _red);
    await theme.selectFor('Retro', ThemeId.lcars);
    await pumpApp(tester);
    final before = tester.state(find.byType(SessionListBody));

    await fleet.selectWorkspace('Work');
    await tester.pumpAndSettle();
    expect(appTokens(tester).chrome, ChromeKind.missionControl);
    expect(
      tester.state(find.byType(SessionListBody)),
      same(before),
      reason: 'only colours changed, so nothing beneath may be rebuilt',
    );

    await fleet.selectWorkspace('Retro');
    await tester.pumpAndSettle();
    expect(appTokens(tester).chrome, ChromeKind.lcars);
    expect(
      tester.state(find.byType(SessionListBody)),
      isNot(same(before)),
      reason: 'LCARS builds different chrome, so the shells re-inflate',
    );
  });

  testWidgets('with a single workspace the usual theme applies', (
    tester,
  ) async {
    // A theme left on Main from when there were more workspaces must not stick
    // once the scope selector that could edit it is gone.
    api.workspacesResponse = const [];
    await theme.selectFor(mainWorkspaceThemeKey, ThemeId.lcars);
    await pumpApp(tester);
    expect(fleet.workspacesVisible, isFalse);
    expect(theme.activeWorkspaceKey, isNull);
    expect(appTokens(tester).chrome, ChromeKind.missionControl);
  });
}
