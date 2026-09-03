import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/golden.dart' show goldenThemes;

/// A phone held sideways has ~360dp of height, and the page header used to take
/// a fixed 56 (Mission Control's app bar) or ~60 (LCARS' title block) of it
/// regardless. Both themes compact that header below [kShortViewportHeight].
///
/// Measured off the body's top edge rather than the header's own widgets: that
/// is the number the page actually loses, it is comparable across the two
/// themes, and it does not pin either theme's internal composition.
void main() {
  Widget page(CommanderTokens tokens, {TextScaler? scaler}) => MaterialApp(
    theme: themeDataFor(tokens),
    builder: (context, child) => MediaQuery(
      data: MediaQuery.of(
        context,
      ).copyWith(textScaler: scaler ?? TextScaler.noScaling),
      child: child!,
    ),
    home: ChromePage(
      code: '47-B',
      title: 'Detail',
      subtitle: 'GENIO · FIX/AUTH-BYPASS',
      body: const SizedBox.expand(key: Key('page-body')),
    ),
  );

  /// Pumps [tokens]' page at [size] and returns where the body starts.
  Future<double> bodyTop(
    WidgetTester tester,
    CommanderTokens tokens,
    Size size, {
    TextScaler? scaler,
  }) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(page(tokens, scaler: scaler));
    await tester.pumpAndSettle();
    return tester.getRect(find.byKey(const Key('page-body'))).top;
  }

  /// Where the subtitle's ink ends. The number a compacted header has to
  /// contain.
  double subtitleBottom(WidgetTester tester) => tester
      .getRect(
        find.textContaining(RegExp('auth-bypass', caseSensitive: false)).first,
      )
      .bottom;

  for (final MapEntry(key: name, value: tokens) in goldenThemes.entries) {
    group(name, () {
      testWidgets('the header shrinks in a short viewport', (tester) async {
        final tall = await bodyTop(tester, tokens, const Size(800, 600));
        final short = await bodyTop(tester, tokens, const Size(800, 360));

        expect(short, lessThan(tall));
        // A budget rather than each theme's exact height, and a
        // non-arbitrary one: a compacted header — LCARS' cap, title and
        // subtitle included — must cost less than a single *uncompacted*
        // Material app bar. Measured today: 36 (Mission Control) and 54
        // (LCARS), against 80 apiece upright.
        expect(short, lessThan(kToolbarHeight));
      });

      /// A compacted header is a *pinned* height, so it has to be able to hold
      /// the type inside it. Mission Control's fixed 36 clipped the subtitle
      /// from 1.3× upwards (measured: the subtitle ended 38.5dp down) until the
      /// bar grew with the scale; LCARS clamps its own title scaling instead.
      /// Either way the contract is the same — the header contains its text.
      testWidgets('the compact header contains its type at 1.5x text', (
        tester,
      ) async {
        final top = await bodyTop(
          tester,
          tokens,
          const Size(800, 360),
          scaler: const TextScaler.linear(1.5),
        );

        expect(subtitleBottom(tester), lessThanOrEqualTo(top));
      });

      /// And under a **non-linear** scaler, which is the one that matters:
      /// Android 14+ font scaling grows small text more than large, so a header
      /// sized by scaling its own height would grow by less than the 9pt
      /// subtitle inside it and clip on exactly the devices asking for bigger
      /// text — while a linear-scaler test like the one above sailed through.
      /// A test that can only inject a linear scaler cannot see that class of
      /// bug, so this injects the shape of the real curve.
      testWidgets('and under a non-linear scaler', (tester) async {
        final top = await bodyTop(
          tester,
          tokens,
          const Size(800, 360),
          scaler: const _NonLinearScaler(),
        );

        expect(subtitleBottom(tester), lessThanOrEqualTo(top));
      });

      testWidgets('an upright phone keeps the full header', (tester) async {
        expect(
          await bodyTop(tester, tokens, const Size(360, 800)),
          greaterThanOrEqualTo(kToolbarHeight),
        );
      });
    });
  }
}

/// A text scaler with the *shape* of Android 14+'s non-linear font scaling —
/// small sizes grow, large ones barely do — taken to the limit of that shape:
/// body-sized type scales fully while anything display-sized does not scale at
/// all.
///
/// Deliberately a caricature rather than the platform's real curve. A gentler
/// one (1.6× under 10pt, 1.15× above) was tried first and passed *both* the
/// correct implementation and the broken height-scaling one, because the bar's
/// 3dp of slack absorbed the difference — a test that cannot tell them apart is
/// not a test. What has to be exercised is the assumption itself: that
/// `scale(size) / size` is the same for the bar's height as for the type inside
/// it. Here it is 1.0 versus 1.34.
///
/// Extends rather than implements [TextScaler], so `clamp` is the framework's
/// own (`painting/text_scaler.dart:62-75`, returning its `_ClampedTextScaler`)
/// rather than a stand-in written here. That matters: the chrome relies on
/// clamping being applied per *font size*, and a hand-rolled clamp in the test
/// would only prove the test agrees with itself.
class _NonLinearScaler extends TextScaler {
  const _NonLinearScaler();

  @override
  double scale(double fontSize) => fontSize >= 24 ? fontSize : fontSize * 1.34;

  // Abstract on [TextScaler] and deprecated there, so an implementer has no
  // choice about declaring it. Nothing in the chrome reads it — the height is
  // derived through `scale` — so the estimate it returns is never consulted.
  @override
  // ignore: deprecated_member_use
  double get textScaleFactor => 1.34;
}
