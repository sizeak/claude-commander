import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:flutter_test/flutter_test.dart';

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
}
