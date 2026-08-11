import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/golden.dart';
import '../support/ink.dart';

/// The LCARS lifecycle bar: one contiguous run of lettered blocks.
///
/// Measured on painted pixels rather than widget boxes, for the reason
/// `ink.dart` records — a `Text` centres its *line box*, not its ink, so a
/// block can be geometrically perfect and still look wrong. The real fonts are
/// mandatory here: against Ahem's filled boxes every glyph's ink *is* its box,
/// so a centring assertion passes on type the app never renders.
void main() {
  const labels = ['Shell', 'Kill', 'Restart', 'Cascade', 'Push', 'Delete'];

  /// [width] null renders the bar unconstrained, which is how a test reads a
  /// block's *natural* width: a bar given a bounded column always fills it, so
  /// there is no bounded width at which the blocks are their own size.
  Widget bar(double? width, {List<String> of = labels}) => RepaintBoundary(
    key: inkBoundary,
    child: MaterialApp(
      theme: themeDataFor(lcarsTokens),
      home: Scaffold(
        backgroundColor: lcarsTokens.canvas,
        body: Align(
          alignment: Alignment.topLeft,
          child: _sized(
            width,
            child: ChromeButtonBar(
              ChromeButtonBarSpec(
                buttons: [
                  for (final label in of)
                    ChromeBarButton(
                      label: label,
                      icon: Icons.code,
                      onPressed: () {},
                    ),
                ],
              ),
            ),
          ),
        ),
      ),
    ),
  );

  // Six blocks at their natural label widths do not fit a phone's content
  // column — on a Pixel 8a (411dp wide) the run missed by 1.1dp and Flutter
  // painted its overflow banner over a clipped DELETE; on a 360dp phone it
  // misses by far more. The run folds onto further lines rather than
  // overflowing, each line a contiguous run of its own.
  testWidgets('the run folds onto a second line rather than overflowing', (
    tester,
  ) async {
    await loadCommanderFonts();
    await tester.pumpWidget(bar(300));
    await tester.pumpAndSettle();

    final tops = {
      for (final label in labels)
        label: tester
            .getRect(find.widgetWithText(ChromeElbow, label.toUpperCase()))
            .top,
    };
    expect(
      tops.values.toSet().length,
      2,
      reason: 'six blocks in 300dp need exactly two lines: $tops',
    );
    // Balanced and in order, not greedily packed — a 4/2 split leaves a
    // stranded pair, and reordering would move DELETE somewhere unexpected.
    expect(tops['Shell'], tops['Kill']);
    expect(tops['Kill'], tops['Restart']);
    expect(tops['Cascade'], tops['Push']);
    expect(tops['Push'], tops['Delete']);
    expect(tops['Cascade']!, greaterThan(tops['Restart']!));
    expect(tester.takeException(), isNull);
  });

  // Folded lines are a block, not a ragged left-aligned stack: every line fills
  // the column, so the lines share both edges and the run reads as one shape.
  // Filled by growing each block in proportion to the width it already wanted,
  // so a stretched line's blocks keep their relative sizes and none is ever
  // squeezed below the label it has to hold.
  testWidgets('every folded line fills the column', (tester) async {
    await loadCommanderFonts();
    await tester.pumpWidget(bar(300));
    await tester.pumpAndSettle();

    Rect line(String first, String last) => tester
        .getRect(find.widgetWithText(ChromeElbow, first.toUpperCase()))
        .expandToInclude(
          tester.getRect(find.widgetWithText(ChromeElbow, last.toUpperCase())),
        );

    for (final run in [line('Shell', 'Restart'), line('Cascade', 'Delete')]) {
      expect(run.left, moreOrLessEquals(0, epsilon: 0.5));
      expect(run.right, moreOrLessEquals(300, epsilon: 0.5));
    }
    expect(tester.takeException(), isNull);
  });

  /// The tightest column that still folds to two lines. The test below needs it
  /// rather than the 300dp the others use: a wide column has slack enough that
  /// even an equal-share stretch clears every label, so it cannot tell a correct
  /// implementation from a wrong one.
  const tight = 170.0;

  // What makes the stretch *proportional* rather than an equal share, and the
  // only assertion that can tell the two apart. At [tight] an equal third is
  // less than RESTART's own label needs while SHELL sits in slack, so RESTART
  // ellipsises. No folded block may come out narrower than it was unfolded.
  testWidgets('stretching a line grows every block and shrinks none', (
    tester,
  ) async {
    await loadCommanderFonts();
    double widthOf(String label) => tester
        .getSize(find.widgetWithText(ChromeElbow, label.toUpperCase()))
        .width;

    await tester.pumpWidget(bar(null));
    await tester.pumpAndSettle();
    final natural = {for (final label in labels) label: widthOf(label)};

    await tester.pumpWidget(bar(tight));
    await tester.pumpAndSettle();

    for (final label in labels) {
      expect(
        widthOf(label),
        // A hair of tolerance for integer flex rounding, which can shave a
        // fraction off a block that had almost no room to grow. An equal-share
        // stretch misses by whole dp, well outside this.
        greaterThanOrEqualTo(natural[label]! - 1),
        reason: '$label was squeezed below the label it has to hold',
      );
    }
  });

  // A run with room stays on one line — and fills the column exactly as a
  // folded line does. One rule, not two: in landscape the six lifecycle actions
  // fit, and the unfilled run hugged the left of a column whose every card ran
  // its full width.
  testWidgets('a run that fits stays on one line and fills the column', (
    tester,
  ) async {
    await loadCommanderFonts();
    await tester.pumpWidget(bar(600));
    await tester.pumpAndSettle();

    Rect blockOf(String label) =>
        tester.getRect(find.widgetWithText(ChromeElbow, label.toUpperCase()));

    expect({for (final label in labels) blockOf(label).top}.length, 1);
    final run = blockOf('Shell').expandToInclude(blockOf('Delete'));
    expect(run.left, moreOrLessEquals(0, epsilon: 0.5));
    expect(run.right, moreOrLessEquals(600, epsilon: 0.5));
  });
}

/// [SizedBox] for a width, or an [UnconstrainedBox] for none.
Widget _sized(double? width, {required Widget child}) => width == null
    ? UnconstrainedBox(child: child)
    : SizedBox(width: width, child: child);
