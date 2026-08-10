import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// LCARS bands are bright, so the system icons over them must be dark — the
/// inverse of Mission Control, which keeps light icons on its near-black canvas.
void main() {
  Widget page(CommanderTokens tokens) => MaterialApp(
    theme: themeDataFor(tokens),
    home: ChromePage(
      code: '47-B',
      title: 'Detail',
      body: const SizedBox.expand(),
    ),
  );

  // The phone shell's own frame, distinct from a pushed page's — [buildPage]
  // and [buildShell] wrap separate `Scaffold`s, so one covering the other
  // would leave the shell's overlay style unverified.
  Widget shell(CommanderTokens tokens) => MaterialApp(
    theme: themeDataFor(tokens),
    home: ChromeShell(
      ChromeShellSpec(body: const SizedBox.expand(), items: const []),
    ),
  );

  SystemUiOverlayStyle styleOf(WidgetTester tester) => tester
      .widgetList<AnnotatedRegion<SystemUiOverlayStyle>>(
        find.byType(AnnotatedRegion<SystemUiOverlayStyle>),
      )
      .first
      .value;

  testWidgets('LCARS asks for dark icons on transparent bars', (tester) async {
    await tester.pumpWidget(page(lcarsTokens));
    await tester.pumpAndSettle();

    final style = styleOf(tester);
    expect(style.statusBarIconBrightness, Brightness.dark);
    expect(style.systemNavigationBarIconBrightness, Brightness.dark);
    // Brightness alone is not enough: a pre-15 three-button device keeps an
    // opaque navigation bar, which would hide the footer's bleed behind it.
    expect(style.systemNavigationBarColor, Colors.transparent);
  });

  testWidgets('the phone shell declares the same overlay as a page', (
    tester,
  ) async {
    await tester.pumpWidget(shell(lcarsTokens));
    await tester.pumpAndSettle();

    // Scoped to the chrome's own region — an ancestor of the `Scaffold` —
    // rather than `styleOf`'s bare `.first`: `buildPage` and `buildShell` wrap
    // separate `Scaffold`s, so without a shell-specific pump, gutting
    // `buildShell`'s style leaves every other test in this file green.
    final region = tester.widget<AnnotatedRegion<SystemUiOverlayStyle>>(
      find
          .ancestor(
            of: find.byType(Scaffold),
            matching: find.byType(AnnotatedRegion<SystemUiOverlayStyle>),
          )
          .first,
    );
    expect(region.value.statusBarIconBrightness, Brightness.dark);
    expect(region.value.systemNavigationBarColor, Colors.transparent);
  });

  testWidgets('Mission Control declares no LCARS overlay', (tester) async {
    await tester.pumpWidget(page(missionControlTokens));
    await tester.pumpAndSettle();

    // Not a bare `find.byType`: `AppBar` always wraps its own Material in an
    // `AnnotatedRegion<SystemUiOverlayStyle>` regardless of theme
    // (`app_bar.dart:1232` in the pinned 3.41.5 SDK) — that region is a
    // *descendant* of the page's `Scaffold`, never an ancestor of it, so
    // scoping to "ancestor of Scaffold" finds only a chrome-level region.
    expect(
      find.ancestor(
        of: find.byType(Scaffold),
        matching: find.byType(AnnotatedRegion<SystemUiOverlayStyle>),
      ),
      findsNothing,
      reason: 'the Material theme keeps the framework default',
    );
  });
}
