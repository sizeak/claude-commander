import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/fleet_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_prefs.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:claude_commander_client/widgets/workspace_menu.dart';
import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_commander_api.dart';
import '../support/fixtures.dart';

const _red = Color(0xFFFF0000);

/// The fleet title's workspace switcher colours each entry — and the current
/// one's label — with that workspace's resolved primary, so a themed workspace
/// is recognisable before it is switched to.
void main() {
  late FleetStore fleet;
  late ThemeController theme;

  setUp(() async {
    final api = FakeCommanderApi()
      ..workspacesResponse = const [
        WorkspaceDef(name: 'Work'),
        WorkspaceDef(name: 'Retro'),
      ];
    final store = CommanderStore(api: api, config: testConfig);
    fleet = FleetStore.withStores([store], prefs: InMemoryPrefStore());
    await store.connect();
    theme = ThemeController(store: InMemoryPrefStore());
    await theme.setOverride('Work', ThemeRole.primary, _red);
    await theme.selectFor('Retro', ThemeId.lcars);
  });

  tearDown(() => fleet.dispose());

  test('each entry is dotted in its workspace\'s resolved primary', () {
    final menu = workspaceTitleMenu(fleet, theme: theme)!;
    expect(
      {for (final i in menu.items) i.label: i.color},
      {
        'Main': missionControlTokens.primary,
        'Work': _red,
        'Retro': lcarsTokens.primary,
      },
    );
  });

  test('the current label follows the active workspace', () async {
    expect(
      workspaceTitleMenu(fleet, theme: theme)!.color,
      missionControlTokens.primary,
    );
    await fleet.selectWorkspace('Work');
    expect(workspaceTitleMenu(fleet, theme: theme)!.color, _red);
  });

  test('without a theme controller nothing is coloured', () {
    final menu = workspaceTitleMenu(fleet)!;
    expect(menu.color, isNull);
    expect(menu.items.map((i) => i.color), everyElement(isNull));
  });
}
