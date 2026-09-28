import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  /// Every style in a [TextTheme], named so a failure says which one lost its
  /// face rather than just "expected SpaceGrotesk, got null".
  Map<String, TextStyle?> stylesOf(TextTheme t) => {
    'displayLarge': t.displayLarge,
    'displayMedium': t.displayMedium,
    'displaySmall': t.displaySmall,
    'headlineLarge': t.headlineLarge,
    'headlineMedium': t.headlineMedium,
    'headlineSmall': t.headlineSmall,
    'titleLarge': t.titleLarge,
    'titleMedium': t.titleMedium,
    'titleSmall': t.titleSmall,
    'bodyLarge': t.bodyLarge,
    'bodyMedium': t.bodyMedium,
    'bodySmall': t.bodySmall,
    'labelLarge': t.labelLarge,
    'labelMedium': t.labelMedium,
    'labelSmall': t.labelSmall,
  };

  group('themeDataFor', () {
    test('every text style carries the theme face', () {
      // The one that mattered in practice is bodyMedium: `Material` installs it
      // as the `DefaultTextStyle`, so it is what every `Text` without an explicit
      // family inherits — including Mission Control's session row titles, the
      // most prominent text in the fleet list. When `_textTheme` replaced these
      // styles wholesale via `copyWith`, they dropped the family that `.apply()`
      // had set, and those titles silently rendered in the *platform* sans:
      // close enough to Space Grotesk on Linux to pass unnoticed, and Roboto on
      // Android.
      for (final tokens in [missionControlTokens, lcarsTokens]) {
        stylesOf(themeDataFor(tokens).textTheme).forEach((name, style) {
          expect(
            style?.fontFamily,
            tokens.sans,
            reason: '$name lost the theme face (${tokens.chrome.name})',
          );
        });
      }
    });

    test('the per-level colours and tracking survive the face', () {
      // The families must not have been restored by dropping the copyWith that
      // sets these — they are the reason it exists.
      final text = themeDataFor(missionControlTokens).textTheme;
      expect(text.bodyMedium?.color, missionControlTokens.textBright);
      expect(text.bodyMedium?.height, 1.45);
      expect(text.bodySmall?.color, missionControlTokens.textMuted);
      expect(text.titleLarge?.fontWeight, FontWeight.w700);
      expect(text.titleLarge?.letterSpacing, -0.3);
      // LCARS flips the sign of the tracking rather than carrying two sets.
      expect(
        themeDataFor(lcarsTokens).textTheme.titleLarge?.letterSpacing,
        0.6,
      );
    });
  });
}
