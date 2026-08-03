import 'package:claude_commander_client/theme/app_theme.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// A **migration-scoped** test: it asserts that `themeDataFor(
/// missionControlTokens)` reproduces the old `AppTheme.dark()` exactly, by
/// comparing the two side by side while both still exist.
///
/// This is the strongest available evidence that Phase 1 of the theming work is
/// visually neutral — far stronger than the token parity test, because it covers
/// the ~15 Material component themes (dialogs, menus, snackbars, inputs,
/// buttons) that no widget test in this repo asserts on.
///
/// Delete this file in the same commit that deletes `lib/theme/app_theme.dart`.
/// Its whole purpose is the changeover; afterwards there is nothing to compare
/// against and `tokens_test.dart` is the standing guard.
void main() {
  final old = AppTheme.dark();
  final now = themeDataFor(missionControlTokens);

  group('themeDataFor(missionControlTokens) == AppTheme.dark()', () {
    test('colour scheme', () {
      expect(now.colorScheme, old.colorScheme);
    });

    test('scaffold, canvas, divider', () {
      expect(now.scaffoldBackgroundColor, old.scaffoldBackgroundColor);
      expect(now.canvasColor, old.canvasColor);
      expect(now.dividerColor, old.dividerColor);
    });

    test('the base font family', () {
      // Compared via a level that `_textTheme` does not overwrite. The explicitly
      // set levels (bodyMedium and friends) carry a *null* family in both themes,
      // because `TextTheme.copyWith` replaces a style outright rather than
      // merging — so `.apply(fontFamily:)` is undone for exactly those levels.
      // Pre-existing behaviour, preserved deliberately.
      expect(now.textTheme.headlineSmall?.fontFamily, 'SpaceGrotesk');
      expect(
        now.textTheme.headlineSmall?.fontFamily,
        old.textTheme.headlineSmall?.fontFamily,
      );
      expect(now.textTheme.bodyMedium?.fontFamily, isNull);
    });

    test('text theme, including per-level letter spacing', () {
      // The old theme tracked titleLarge at -0.3, titleMedium at -0.2 and
      // titleSmall not at all. A single shared helper would have flattened them.
      for (final pick in <(String, TextStyle? Function(TextTheme))>[
        ('titleLarge', (t) => t.titleLarge),
        ('titleMedium', (t) => t.titleMedium),
        ('titleSmall', (t) => t.titleSmall),
        ('bodyMedium', (t) => t.bodyMedium),
        ('bodySmall', (t) => t.bodySmall),
        ('labelLarge', (t) => t.labelLarge),
      ]) {
        final (name, get) = pick;
        expect(get(now.textTheme), get(old.textTheme), reason: name);
      }
    });

    test('app bar', () {
      expect(now.appBarTheme, old.appBarTheme);
    });

    test('cards and inputs', () {
      expect(now.cardTheme, old.cardTheme);
      expect(now.inputDecorationTheme, old.inputDecorationTheme);
    });

    test('buttons', () {
      expect(now.filledButtonTheme, old.filledButtonTheme);
      expect(now.outlinedButtonTheme, old.outlinedButtonTheme);
      expect(now.textButtonTheme, old.textButtonTheme);
    });

    test('overlays: dialogs, menus, snackbars', () {
      expect(now.dialogTheme, old.dialogTheme);
      expect(now.popupMenuTheme, old.popupMenuTheme);
      expect(now.snackBarTheme, old.snackBarTheme);
      expect(now.dropdownMenuTheme, old.dropdownMenuTheme);
    });

    test('icons, list tiles, progress, dividers', () {
      expect(now.iconTheme, old.iconTheme);
      expect(now.listTileTheme, old.listTileTheme);
      expect(now.progressIndicatorTheme, old.progressIndicatorTheme);
      expect(now.dividerTheme, old.dividerTheme);
    });

    test('the caret matches what the old theme resolved to by default', () {
      // The old theme set no textSelectionTheme, so Material 3 derived the caret
      // from colorScheme.primary. Stating it explicitly must not change it.
      expect(now.textSelectionTheme.cursorColor, old.colorScheme.primary);
    });
  });

  test('the token extension is installed', () {
    expect(
      themeDataFor(lcarsTokens).extension<CommanderTokens>()?.primary,
      lcarsTokens.primary,
    );
  });
}
