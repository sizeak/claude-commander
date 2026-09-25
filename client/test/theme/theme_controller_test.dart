import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_prefs.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';

const _red = Color(0xFFFF0000);
const _blue = Color(0xFF0000FF);

void main() {
  group('ThemeId.fromWire', () {
    test('round-trips every theme through its persisted spelling', () {
      for (final id in ThemeId.values) {
        expect(ThemeId.fromWire(id.wire), id);
      }
    });

    test('falls back to Mission Control for absent or unknown values', () {
      // A preferences file written by a newer build can name a theme this one
      // does not have; launching must not throw.
      expect(ThemeId.fromWire(null), ThemeId.missionControl);
      expect(ThemeId.fromWire(''), ThemeId.missionControl);
      expect(ThemeId.fromWire('nostromo'), ThemeId.missionControl);
    });

    test('the wire spellings are stable and not the Dart names', () {
      // Renaming the enum constant must not reset every user's theme, so the
      // persisted form is pinned here deliberately.
      expect(ThemeId.missionControl.wire, 'mission_control');
      expect(ThemeId.lcars.wire, 'lcars');
    });
  });

  group('ThemeController', () {
    test('defaults to Mission Control with nothing stored', () async {
      final c = ThemeController(store: InMemoryPrefStore());
      await c.load();
      expect(c.id, ThemeId.missionControl);
      expect(c.tokens.primary, ThemeId.missionControl.tokens.primary);
    });

    test('load restores a persisted choice', () async {
      final store = InMemoryPrefStore({ThemeController.prefKey: 'lcars'});
      final c = ThemeController(store: store);
      await c.load();
      expect(c.id, ThemeId.lcars);
    });

    test('load notifies only when the stored choice differs', () async {
      var notifications = 0;
      final c = ThemeController(store: InMemoryPrefStore())
        ..addListener(() => notifications++);
      await c.load();
      expect(notifications, 0, reason: 'already on the default');

      final c2 = ThemeController(
        store: InMemoryPrefStore({ThemeController.prefKey: 'lcars'}),
      )..addListener(() => notifications++);
      await c2.load();
      expect(notifications, 1);
    });

    test('select persists and notifies', () async {
      final store = InMemoryPrefStore();
      var notifications = 0;
      final c = ThemeController(store: store)
        ..addListener(() => notifications++);

      await c.select(ThemeId.lcars);
      expect(c.id, ThemeId.lcars);
      expect(notifications, 1);
      expect(await store.read(ThemeController.prefKey), 'lcars');
    });

    test('selecting the current theme is a no-op', () async {
      var notifications = 0;
      final c = ThemeController(store: InMemoryPrefStore())
        ..addListener(() => notifications++);
      await c.select(ThemeId.missionControl);
      expect(notifications, 0);
    });

    test('a round trip through the store survives a fresh controller', () async {
      // What actually matters on relaunch: the deck requires the theme to be
      // restored before the first frame, so a new controller over the same store
      // must come up already themed.
      final store = InMemoryPrefStore();
      await ThemeController(store: store).select(ThemeId.lcars);

      final relaunched = ThemeController(store: store);
      expect(relaunched.id, ThemeId.missionControl, reason: 'before load()');
      await relaunched.load();
      expect(relaunched.id, ThemeId.lcars);
    });
  });

  group('per-workspace themes', () {
    test('load restores usual overrides and every workspace theme', () async {
      final store = InMemoryPrefStore({
        ThemeController.prefKey:
            '{"themeId":"lcars","overrides":{"primary":"#ff0000"}}',
        ThemeController.workspacesPrefKey:
            '{"Work":{"themeId":"mission_control"},'
            '"main":{"overrides":{"danger":"#0000ff"}}}',
      });
      final c = ThemeController(store: store);
      await c.load();
      expect(c.id, ThemeId.lcars);
      expect(c.tokens.primary, _red);
      expect(c.workspaceTheme('Work')!.themeId, ThemeId.missionControl);
      expect(c.workspaceTheme('main')!.overrides, {ThemeRole.danger: _blue});
    });

    test(
      'a failure reading the workspace themes keeps the usual theme',
      () async {
        final c = ThemeController(
          store: _FailingKeyStore({
            ThemeController.prefKey: 'lcars',
          }, failing: ThemeController.workspacesPrefKey),
        );
        await c.load();
        expect(c.id, ThemeId.lcars);
      },
    );

    test('switching workspace swaps the tokens and notifies', () async {
      final store = InMemoryPrefStore({
        ThemeController.workspacesPrefKey: '{"Work":{"themeId":"lcars"}}',
      });
      var notifications = 0;
      final c = ThemeController(store: store);
      await c.load();
      c.addListener(() => notifications++);

      c.setActiveWorkspace('Work');
      expect(c.id, ThemeId.lcars);
      expect(c.tokens.chrome, ChromeKind.lcars);
      expect(notifications, 1);

      c.setActiveWorkspace(null);
      expect(c.id, ThemeId.missionControl);
      expect(notifications, 2);
    });

    test('a switch that resolves to the same theme does not notify', () async {
      // The fleet notifies on every snapshot; rethemeing the app each time
      // would restart the crossfade for nothing.
      var notifications = 0;
      final c = ThemeController(store: InMemoryPrefStore())
        ..addListener(() => notifications++);
      c.setActiveWorkspace('Work');
      c.setActiveWorkspace('main');
      c.setActiveWorkspace(null);
      expect(notifications, 0);
      expect(identical(c.tokens, missionControlTokens), isTrue);
    });

    test('tokens stay the same object until the theme changes', () async {
      final c = ThemeController(store: InMemoryPrefStore());
      await c.setOverride(null, ThemeRole.primary, _red);
      final first = c.tokens;
      c.setActiveWorkspace('Work');
      expect(identical(c.tokens, first), isTrue);
    });

    test('selectFor a workspace persists it under the workspace map', () async {
      final store = InMemoryPrefStore();
      final c = ThemeController(store: store)..setActiveWorkspace('Work');
      await c.selectFor('Work', ThemeId.lcars);
      expect(c.id, ThemeId.lcars);
      expect(c.usual.themeId ?? ThemeId.missionControl, ThemeId.missionControl);
      expect(
        decodeWorkspaceThemes(
          await store.read(ThemeController.workspacesPrefKey),
        ),
        {'Work': const ThemePref(themeId: ThemeId.lcars)},
      );
      expect(await store.read(ThemeController.prefKey), isNull);
    });

    test('a usual override is stored as JSON, and clearing it restores the '
        'plain spelling', () async {
      final store = InMemoryPrefStore();
      final c = ThemeController(store: store);
      await c.setOverride(null, ThemeRole.primary, _red);
      expect(c.tokens.primary, _red);
      expect(
        decodeUsualTheme(await store.read(ThemeController.prefKey)).overrides,
        {ThemeRole.primary: _red},
      );
      await c.setOverride(null, ThemeRole.primary, null);
      expect(await store.read(ThemeController.prefKey), 'mission_control');
      expect(c.tokens.primary, missionControlTokens.primary);
    });

    test('a workspace override layers over the usual theme', () async {
      final c = ThemeController(store: InMemoryPrefStore());
      await c.select(ThemeId.lcars);
      await c.setOverride('Work', ThemeRole.danger, _blue);
      c.setActiveWorkspace('Work');
      expect(c.id, ThemeId.lcars);
      expect(c.tokens.danger, _blue);
      expect(c.tokensFor(null).danger, lcarsTokens.danger);
    });

    test('reset drops the workspace back to the usual theme', () async {
      final store = InMemoryPrefStore();
      final c = ThemeController(store: store)..setActiveWorkspace('Work');
      await c.selectFor('Work', ThemeId.lcars);
      await c.resetWorkspace('Work');
      expect(c.workspaceTheme('Work'), isNull);
      expect(c.id, ThemeId.missionControl);
      expect(
        decodeWorkspaceThemes(
          await store.read(ThemeController.workspacesPrefKey),
        ),
        isEmpty,
      );
    });

    test('a rename copies the theme first and drops the old key on commit, '
        'without ever changing the rendered theme', () async {
      // The page runs this around the fleet rename: the copy lands before the
      // fleet moves the active workspace to the new name, so every step of the
      // move resolves to the same theme.
      final store = InMemoryPrefStore();
      final c = ThemeController(store: store)..setActiveWorkspace('Work');
      await c.selectFor('Work', ThemeId.lcars);
      final seen = <ThemeId>[];
      c.addListener(() => seen.add(c.id));

      final pending = c.beginRename('Work', 'Job')!;
      expect(c.workspaceTheme('Work')!.themeId, ThemeId.lcars);
      expect(c.workspaceTheme('Job')!.themeId, ThemeId.lcars);
      c.setActiveWorkspace('Job'); // the fleet catching up
      await c.commitRename(pending);

      expect(c.workspaceTheme('Work'), isNull);
      expect(c.workspaceTheme('Job')!.themeId, ThemeId.lcars);
      expect(c.id, ThemeId.lcars);
      expect(seen.where((id) => id != ThemeId.lcars), isEmpty);
      expect(
        decodeWorkspaceThemes(
          await store.read(ThemeController.workspacesPrefKey),
        ).keys,
        ['Job'],
      );
    });

    test('an aborted rename puts both keys back as they were', () async {
      final c = ThemeController(store: InMemoryPrefStore());
      await c.selectFor('Work', ThemeId.lcars);
      await c.setOverride('Job', ThemeRole.primary, _red);

      final pending = c.beginRename('Work', 'Job')!;
      await c.abortRename(pending);

      expect(c.workspaceTheme('Work')!.themeId, ThemeId.lcars);
      expect(c.workspaceTheme('Job')!.overrides, {ThemeRole.primary: _red});
    });

    test('renaming onto a stale entry drops it even when the old name had '
        'none', () async {
      // Left behind by a workspace deleted elsewhere: the renamed workspace now
      // owns the name, and must not silently wear a theme it never had.
      final c = ThemeController(store: InMemoryPrefStore());
      await c.selectFor('Job', ThemeId.lcars);

      final pending = c.beginRename('Work', 'Job')!;
      await c.commitRename(pending);

      expect(c.workspaceTheme('Job'), isNull);
    });

    test('renaming a workspace with no theme changes nothing', () async {
      final store = InMemoryPrefStore();
      var notifications = 0;
      final c = ThemeController(store: store)
        ..addListener(() => notifications++);
      final pending = c.beginRename('Work', 'Job')!;
      await c.commitRename(pending);
      expect(notifications, 0);
      expect(await store.read(ThemeController.workspacesPrefKey), isNull);
    });

    test('Main is never renamed by key', () {
      final c = ThemeController(store: InMemoryPrefStore());
      expect(c.beginRename(mainWorkspaceThemeKey, 'Home'), isNull);
    });

    test(
      'forgetting a deleted workspace drops its theme but never Main\'s',
      () async {
        final c = ThemeController(store: InMemoryPrefStore());
        await c.selectFor('Work', ThemeId.lcars);
        await c.selectFor(mainWorkspaceThemeKey, ThemeId.lcars);
        await c.forgetWorkspace('Work');
        await c.forgetWorkspace(mainWorkspaceThemeKey);
        expect(c.workspaceTheme('Work'), isNull);
        expect(c.workspaceTheme(mainWorkspaceThemeKey), isNotNull);
      },
    );
  });
}

/// A store whose read of one key throws — a corrupt value on the platform side.
class _FailingKeyStore extends InMemoryPrefStore {
  final String failing;

  _FailingKeyStore(super.initial, {required this.failing});

  @override
  Future<String?> read(String key) =>
      key == failing ? Future.error(StateError('corrupt')) : super.read(key);
}
