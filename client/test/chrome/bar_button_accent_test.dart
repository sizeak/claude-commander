import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../theme/mission_control_reference.dart';

/// Pins the *role assignment* of a button bar, not just the palette.
///
/// This is the gap that let a real regression through: `tokens_test.dart` proves
/// every Mission Control colour still has its old value, but nothing proved a
/// widget still used the right one. The lifecycle bar has five hues where
/// `ChromeActionKind` has three values, so migrating it collapsed Kill (amber)
/// and Restart (teal) onto the neutral `textBright` — invisible to every existing
/// test, and to `flutter analyze`.
void main() {
  Future<Color?> iconColour(
    WidgetTester tester,
    ChromeBarButton button, {
    CommanderTokens tokens = missionControlTokens,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: themeDataFor(tokens),
        home: Scaffold(
          body: ChromeButtonBar(ChromeButtonBarSpec(buttons: [button])),
        ),
      ),
    );
    // The colour lands on the IconButton, which resolves it for its Icon — so
    // reading Icon.color directly gets null.
    return tester.widget<IconButton>(find.byType(IconButton)).color;
  }

  group('ChromeBarButton.accent', () {
    testWidgets('overrides the colour that kind alone would give', (
      tester,
    ) async {
      expect(
        await iconColour(
          tester,
          ChromeBarButton(
            label: 'Kill',
            icon: Icons.stop,
            accent: missionControlTokens.attentionOn,
            onPressed: () {},
          ),
        ),
        McRef.amberText,
        reason: 'Kill was amber before the chrome layer, not neutral',
      );
    });

    testWidgets('a normal button with no accent stays neutral', (tester) async {
      expect(
        await iconColour(
          tester,
          ChromeBarButton(label: 'Shell', icon: Icons.code, onPressed: () {}),
        ),
        McRef.textBright,
      );
    });

    testWidgets('destructive still wins its danger colour', (tester) async {
      expect(
        await iconColour(
          tester,
          ChromeBarButton(
            label: 'Delete',
            icon: Icons.delete_outline,
            kind: ChromeActionKind.destructive,
            onPressed: () {},
          ),
        ),
        McRef.red,
      );
    });

    testWidgets('LCARS renders blocks, not Material icon buttons', (
      tester,
    ) async {
      // The accent is Mission-Control-only by design: LCARS' block set is a small
      // fixed palette and per-button hues would dilute it. It does not even build
      // an IconButton, which is the structural difference the accent cannot reach.
      await tester.pumpWidget(
        MaterialApp(
          theme: themeDataFor(lcarsTokens),
          home: Scaffold(
            body: ChromeButtonBar(
              ChromeButtonBarSpec(
                buttons: [
                  ChromeBarButton(
                    label: 'Kill',
                    icon: Icons.stop,
                    accent: missionControlTokens.attentionOn,
                    onPressed: () {},
                  ),
                ],
              ),
            ),
          ),
        ),
      );
      expect(find.byType(IconButton), findsNothing);
      expect(
        find.textContaining(RegExp('kill', caseSensitive: false)),
        findsOneWidget,
      );
    });
  });
}
