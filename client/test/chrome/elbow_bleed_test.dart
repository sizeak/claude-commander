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

  /// The cap's bottom-left radius is conditional on the bleed, and the two cases
  /// have to be pinned together: the rounding is the desktop/zero-inset shape
  /// and must survive, while a bled cap has to give it up. On a Pixel 8a the
  /// rounded bled cap cut a black wedge out of the top band, at the cap's own
  /// left edge — the rail/content gutter is filled past the cap there, so the
  /// curve had nothing to flow out of and bit into a solid band instead.
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

    testWidgets('is square once the cap is bled', (tester) async {
      await tester.pumpWidget(host(cap(const EdgeInsets.only(top: 24))));

      expect(capCorner(tester), Radius.zero);
    });
  });
}
