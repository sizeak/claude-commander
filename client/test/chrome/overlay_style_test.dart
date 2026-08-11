import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/chrome/chrome_wide.dart';
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

  Widget wide(CommanderTokens tokens) => MaterialApp(
    theme: themeDataFor(tokens),
    home: ChromeWide(
      ChromeWideSpec(
        fleetList: const SizedBox.expand(),
        workspace: const SizedBox.expand(),
        modes: const [],
        needsInputCount: 0,
        activeCount: 0,
        totalCount: 0,
        serverCount: 1,
      ),
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
    // A transparent colour alone is not enough: measured on a Pixel 8a
    // (three-button, Android 17), the platform still painted an opaque light
    // scrim across the navigation bar and hid the footer's bleed behind it.
    // `...ContrastEnforced: false` is what actually turns that scrim off.
    expect(style.systemNavigationBarColor, Colors.transparent);
    expect(style.systemNavigationBarContrastEnforced, isFalse);
    expect(style.systemStatusBarContrastEnforced, isFalse);
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

  // The wide frame is a third `Scaffold`, built by `chrome_wide.dart` rather
  // than `lcars_chrome.dart`, and it bleeds into the same bars. Without its own
  // region it inherited the framework default: light icons over a bright band.
  testWidgets('the wide shell declares the same overlay', (tester) async {
    await tester.pumpWidget(wide(lcarsTokens));
    await tester.pumpAndSettle();

    final region = tester.widget<AnnotatedRegion<SystemUiOverlayStyle>>(
      find
          .ancestor(
            of: find.byType(Scaffold),
            matching: find.byType(AnnotatedRegion<SystemUiOverlayStyle>),
          )
          .first,
    );
    expect(region.value.statusBarIconBrightness, Brightness.dark);
    expect(region.value.systemNavigationBarContrastEnforced, isFalse);
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
