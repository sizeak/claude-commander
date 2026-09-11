import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// `bleed` is the one mechanism the safe-area work rests on: a block grows into
/// the bezel while its label holds the safe region. Both halves are asserted
/// together here, because either alone is satisfied by a wrong implementation —
/// growth alone by "labels follow", a static label alone by doing nothing.
void main() {
  Widget host(Widget child) => MaterialApp(
    theme: themeDataFor(lcarsTokens),
    home: Scaffold(
      body: Align(
        alignment: Alignment.topLeft,
        child: IntrinsicHeight(child: child),
      ),
    ),
  );

  /// The label's offset from its own block's top edge — the quantity a bottom
  /// bleed must leave alone.
  double labelFromTop(WidgetTester tester) =>
      tester.getRect(find.text('FLEET')).top -
      tester.getRect(find.byType(ChromeElbow)).top;

  ChromeElbow block(EdgeInsets bleed) => ChromeElbow(
    color: const Color(0xFFF7A01D),
    height: 38,
    label: 'FLEET',
    labelAlignment: Alignment.center,
    labelSize: 13,
    bleed: bleed,
  );

  testWidgets('a bottom bleed grows the block and holds the label', (
    tester,
  ) async {
    await tester.pumpWidget(host(block(EdgeInsets.zero)));
    final plainHeight = tester.getRect(find.byType(ChromeElbow)).height;
    final plainLabel = labelFromTop(tester);

    await tester.pumpWidget(host(block(const EdgeInsets.only(bottom: 48))));

    expect(
      tester.getRect(find.byType(ChromeElbow)).height,
      plainHeight + 48,
      reason: 'the fill must reach 48px further down',
    );
    expect(
      labelFromTop(tester),
      plainLabel,
      reason: 'the label must not move within the block',
    );
  });

  testWidgets('a top bleed holds the label against the block bottom', (
    tester,
  ) async {
    double labelFromBottom(WidgetTester tester) =>
        tester.getRect(find.byType(ChromeElbow)).bottom -
        tester.getRect(find.text('FLEET')).bottom;

    await tester.pumpWidget(host(block(EdgeInsets.zero)));
    final plain = labelFromBottom(tester);

    await tester.pumpWidget(host(block(const EdgeInsets.only(top: 24))));

    expect(labelFromBottom(tester), plain);
  });

  testWidgets('an unbled block is byte-for-byte the block it was', (
    tester,
  ) async {
    await tester.pumpWidget(host(block(EdgeInsets.zero)));
    expect(tester.getRect(find.byType(ChromeElbow)).height, 38);
  });

  /// The cap's height comes from the *top* inset alone.
  ///
  /// Tested on the widget directly, and that is the whole point: every caller
  /// pre-filters to `EdgeInsets.only(top:)` (`lcars_chrome.dart`'s `_content`
  /// and `_viewContent`, `chrome_wide.dart`'s `_fleet` and workspace cap), so a
  /// frame-level test cannot tell `bleed.top` from `bleed.vertical` no matter
  /// what insets it drives — mutating one to the other left all 50 tests across
  /// the four bleed files green. Only a cap handed a bottom inset it should
  /// ignore can see the difference.
  ///
  /// The rule used to be pinned by the deleted rail/content seam, which was the
  /// one caller that passed the frame's whole bleed; removing the seam removed
  /// the coverage with it. This is that coverage, put back where it cannot be
  /// deleted as collateral again.
  group('the elbow cap height', () {
    Widget cap(EdgeInsets bleed) =>
        ChromeElbowCap(color: const Color(0xFFCC99CC), bleed: bleed);

    testWidgets('is its unbled height with nothing to bleed into', (
      tester,
    ) async {
      await tester.pumpWidget(host(cap(EdgeInsets.zero)));

      expect(
        tester.getSize(find.byType(ChromeElbowCap)).height,
        kElbowCapHeight,
      );
    });

    testWidgets('ignores a bottom inset entirely', (tester) async {
      await tester.pumpWidget(
        host(cap(const EdgeInsets.only(top: 24, bottom: 48))),
      );

      expect(
        tester.getSize(find.byType(ChromeElbowCap)).height,
        kElbowCapBledHeight + 24,
        reason:
            'a cap closes the top of a column; a bottom inset is not its '
            'business — summing both would give 73 here',
      );
    });
  });

  /// The cap's bottom-left radius is unconditional, and both cases are pinned
  /// so it stays that way. It was briefly conditional: while the rail/content
  /// gutter was *filled* across the status-bar band, a bled cap's curve had no
  /// gap to flow out of and bit a black wedge into a solid band instead, so the
  /// bled case gave the radius up. That left the band and the rail meeting at a
  /// bare 90° inner corner, which the user judged worse than the notch an open
  /// gap costs — the fill went instead, and with the gutter open again the
  /// curve has something to flow out of at every inset.
  group('the elbow cap corner', () {
    Radius capCorner(WidgetTester tester) {
      final container = tester.widget<Container>(
        find.descendant(
          of: find.byType(ChromeElbowCap),
          matching: find.byType(Container),
        ),
      );
      final decoration = container.decoration as BoxDecoration;
      return (decoration.borderRadius! as BorderRadius).bottomLeft;
    }

    Widget cap(EdgeInsets bleed) =>
        ChromeElbowCap(color: const Color(0xFFCC99CC), bleed: bleed);

    testWidgets('rounds when there is nothing to bleed into', (tester) async {
      await tester.pumpWidget(host(cap(EdgeInsets.zero)));

      expect(capCorner(tester), Radius.circular(lcarsTokens.elbowRadius * 0.4));
    });

    testWidgets('rounds just the same once the cap is bled', (tester) async {
      await tester.pumpWidget(host(cap(const EdgeInsets.only(top: 24))));

      expect(capCorner(tester), Radius.circular(lcarsTokens.elbowRadius * 0.4));
    });
  });
}
