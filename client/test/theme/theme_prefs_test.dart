import 'dart:convert';

import 'package:claude_commander_client/theme/theme_controller.dart';
import 'package:claude_commander_client/theme/theme_prefs.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';

const _red = Color(0xFFFF0000);
const _green = Color(0xFF00FF00);
const _blue = Color(0xFF0000FF);

void main() {
  group('ThemeRole', () {
    test('covers exactly the overridable roles, in the documented order', () {
      expect(ThemeRole.values.map((r) => r.wire), [
        'primary',
        'primarySoft',
        'working',
        'nav',
        'info',
        'unread',
        'attention',
        'held',
        'success',
        'danger',
        'idle',
      ]);
    });

    test('reads each role off a token set', () {
      const t = lcarsTokens;
      expect(ThemeRole.primary.of(t), t.primary);
      expect(ThemeRole.primarySoft.of(t), t.primarySoft);
      expect(ThemeRole.working.of(t), t.working);
      expect(ThemeRole.nav.of(t), t.nav);
      expect(ThemeRole.info.of(t), t.info);
      expect(ThemeRole.unread.of(t), t.unread);
      expect(ThemeRole.attention.of(t), t.attention);
      expect(ThemeRole.held.of(t), t.held);
      expect(ThemeRole.success.of(t), t.success);
      expect(ThemeRole.danger.of(t), t.danger);
      expect(ThemeRole.idle.of(t), t.idle);
    });
  });

  group('applyOverrides', () {
    test('no overrides hands back the preset itself', () {
      expect(
        identical(applyOverrides(lcarsTokens, const {}), lcarsTokens),
        isTrue,
      );
    });

    test('replaces only the overridden roles', () {
      final t = applyOverrides(missionControlTokens, {
        ThemeRole.primary: _red,
        ThemeRole.danger: _blue,
      });
      expect(t.primary, _red);
      expect(t.danger, _blue);
      expect(t.working, missionControlTokens.working);
      expect(t.canvas, missionControlTokens.canvas);
      expect(t.chrome, missionControlTokens.chrome);
    });

    test('a role override repaints the session tones drawn from it', () {
      final t = applyOverrides(missionControlTokens, {
        ThemeRole.working: _green,
        ThemeRole.attention: _red,
      });
      expect(t.toneStyle(SessionTone.working).accent, _green);
      expect(t.toneStyle(SessionTone.pushing).accent, _green);
      expect(t.toneStyle(SessionTone.waiting).accent, _red);
      // Untouched roles keep their tones.
      expect(
        t.toneStyle(SessionTone.unread).accent,
        missionControlTokens.toneStyle(SessionTone.unread).accent,
      );
    });

    test('a tone that the preset paints from a different role is left alone', () {
      // LCARS paints `merging` lilac (its nav), not periwinkle (its info), so an
      // info override must not reach it.
      final t = applyOverrides(lcarsTokens, {ThemeRole.info: _red});
      expect(t.toneStyle(SessionTone.creating).accent, _red);
      expect(
        t.toneStyle(SessionTone.merging).accent,
        lcarsTokens.toneStyle(SessionTone.merging).accent,
      );
    });
  });

  group('resolveTheme', () {
    const usual = ThemePref(
      themeId: ThemeId.lcars,
      overrides: {ThemeRole.primary: _red, ThemeRole.nav: _green},
    );

    test('no workspace entry is the usual theme with its overrides', () {
      final r = resolveTheme(usual: usual, workspace: null);
      expect(r.id, ThemeId.lcars);
      expect(r.overrides, {ThemeRole.primary: _red, ThemeRole.nav: _green});
      expect(r.tokens.primary, _red);
      expect(r.tokens.nav, _green);
      expect(r.tokens.chrome, ChromeKind.lcars);
    });

    test('an empty usual theme is Mission Control', () {
      final r = resolveTheme(usual: const ThemePref(), workspace: null);
      expect(r.id, ThemeId.missionControl);
      expect(identical(r.tokens, missionControlTokens), isTrue);
    });

    test('an entry without a theme layers its overrides over the usual '
        'theme and its overrides', () {
      final r = resolveTheme(
        usual: usual,
        workspace: const ThemePref(overrides: {ThemeRole.primary: _blue}),
      );
      expect(r.id, ThemeId.lcars);
      expect(r.tokens.primary, _blue, reason: 'the entry wins');
      expect(r.tokens.nav, _green, reason: 'the usual override carries');
    });

    test('an entry with a theme takes that preset and only its own '
        'overrides', () {
      final r = resolveTheme(
        usual: usual,
        workspace: const ThemePref(
          themeId: ThemeId.missionControl,
          overrides: {ThemeRole.danger: _blue},
        ),
      );
      expect(r.id, ThemeId.missionControl);
      expect(r.tokens.danger, _blue);
      expect(r.tokens.primary, missionControlTokens.primary);
      expect(r.tokens.nav, missionControlTokens.nav);
      expect(r.tokens.chrome, ChromeKind.missionControl);
    });

    test('equal inputs resolve equal, so a no-op switch is detectable', () {
      final a = resolveTheme(usual: usual, workspace: null);
      final b = resolveTheme(usual: usual, workspace: const ThemePref());
      expect(a, b);
      expect(a.hashCode, b.hashCode);
      expect(
        a == resolveTheme(usual: const ThemePref(), workspace: null),
        isFalse,
      );
    });
  });

  group('roleSource', () {
    const usual = ThemePref(
      themeId: ThemeId.lcars,
      overrides: {ThemeRole.primary: _red},
    );

    test('editing the usual theme: overridden or the preset', () {
      expect(
        roleSource(usual: usual, workspace: null, role: ThemeRole.primary),
        RoleSource.overridden,
      );
      expect(
        roleSource(usual: usual, workspace: null, role: ThemeRole.nav),
        RoleSource.preset,
      );
    });

    test('editing a workspace that inherits: the usual override shows as '
        'inherited', () {
      const ws = ThemePref(overrides: {ThemeRole.nav: _blue});
      expect(
        roleSource(usual: usual, workspace: ws, role: ThemeRole.primary),
        RoleSource.usual,
      );
      expect(
        roleSource(usual: usual, workspace: ws, role: ThemeRole.nav),
        RoleSource.overridden,
      );
      expect(
        roleSource(usual: usual, workspace: ws, role: ThemeRole.idle),
        RoleSource.preset,
      );
    });

    test('a workspace with its own preset inherits nothing from the usual '
        'theme', () {
      const ws = ThemePref(themeId: ThemeId.lcars);
      expect(
        roleSource(usual: usual, workspace: ws, role: ThemeRole.primary),
        RoleSource.preset,
      );
    });
  });

  group('ThemePref persistence', () {
    test('round-trips through JSON', () {
      const pref = ThemePref(
        themeId: ThemeId.lcars,
        overrides: {ThemeRole.primary: _red, ThemeRole.idle: _blue},
      );
      final json = jsonDecode(jsonEncode(pref.toJson()));
      expect(ThemePref.fromJson(json), pref);
      expect(pref.toJson(), {
        'themeId': 'lcars',
        'overrides': {'primary': '#ff0000', 'idle': '#0000ff'},
      });
    });

    test('an entry without a theme omits it', () {
      const pref = ThemePref(overrides: {ThemeRole.primary: _red});
      expect(pref.toJson(), {
        'overrides': {'primary': '#ff0000'},
      });
      expect(ThemePref.fromJson(pref.toJson()), pref);
    });

    test('tolerates junk from a newer or corrupt build', () {
      // Unknown theme → inherit; unknown role or malformed hex → dropped; a
      // non-map → empty. None of it may throw, because the theme loads before
      // the first frame.
      expect(
        ThemePref.fromJson({
          'themeId': 'nostromo',
          'overrides': {
            'primary': '#00ff00',
            'sparkle': '#ffffff',
            'danger': 'red',
            'idle': 7,
          },
        }),
        const ThemePref(overrides: {ThemeRole.primary: _green}),
      );
      expect(ThemePref.fromJson('lcars'), const ThemePref());
      expect(ThemePref.fromJson(null), const ThemePref());
      expect(ThemePref.fromJson({'overrides': 3}), const ThemePref());
    });

    test('the usual theme keeps the plain spelling when it has no '
        'overrides', () {
      // So a build that predates overrides still reads the choice.
      expect(
        encodeUsualTheme(const ThemePref(themeId: ThemeId.lcars)),
        'lcars',
      );
      expect(encodeUsualTheme(const ThemePref()), ThemeId.missionControl.wire);
      const withOverrides = ThemePref(
        themeId: ThemeId.lcars,
        overrides: {ThemeRole.primary: _red},
      );
      expect(decodeUsualTheme(encodeUsualTheme(withOverrides)), withOverrides);
    });

    test('decodes the plain spelling older builds wrote', () {
      expect(
        decodeUsualTheme('lcars'),
        const ThemePref(themeId: ThemeId.lcars),
      );
      expect(
        decodeUsualTheme(null),
        const ThemePref(themeId: ThemeId.missionControl),
      );
      expect(
        decodeUsualTheme('{not json'),
        const ThemePref(themeId: ThemeId.missionControl),
      );
    });

    test('the workspace map round-trips and drops malformed entries', () {
      final prefs = {
        'main': const ThemePref(themeId: ThemeId.lcars),
        'Work': const ThemePref(overrides: {ThemeRole.primary: _red}),
      };
      expect(decodeWorkspaceThemes(encodeWorkspaceThemes(prefs)), prefs);
      expect(decodeWorkspaceThemes(null), isEmpty);
      expect(decodeWorkspaceThemes('[1,2]'), isEmpty);
      expect(decodeWorkspaceThemes('garbage'), isEmpty);
      // An entry that decodes to nothing is not kept as an empty override.
      expect(decodeWorkspaceThemes('{"Work": {}, "x": 1}'), isEmpty);
    });
  });

  group('workspaceThemeKey', () {
    test('Main has the reserved key, the rest their names', () {
      expect(workspaceThemeKey(null), 'main');
      expect(workspaceThemeKey('Work'), 'Work');
    });
  });
}
