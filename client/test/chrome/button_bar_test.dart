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

  Widget bar(double width, {List<String> of = labels}) => RepaintBoundary(
    key: inkBoundary,
    child: MaterialApp(
      theme: themeDataFor(lcarsTokens),
      home: Scaffold(
        backgroundColor: lcarsTokens.canvas,
        body: Align(
          alignment: Alignment.topLeft,
          child: SizedBox(
            width: width,
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

  // Folded lines are a block, not a ragged left-aligned stack: every line is
  // widened to the widest one's natural width and the stack is centred in its
  // column. Widened by growing each block in proportion to the width it
  // already wanted, so a stretched line's blocks keep their relative sizes and
  // none is ever squeezed below the label it has to hold.
  testWidgets('folded lines share one width, centred in the column', (
    tester,
  ) async {
    await loadCommanderFonts();
    await tester.pumpWidget(bar(300));
    await tester.pumpAndSettle();

    Rect line(String first, String last) => tester
        .getRect(find.widgetWithText(ChromeElbow, first.toUpperCase()))
        .expandToInclude(
          tester.getRect(find.widgetWithText(ChromeElbow, last.toUpperCase())),
        );

    final top = line('Shell', 'Restart');
    final bottom = line('Cascade', 'Delete');

    expect(top.width, moreOrLessEquals(bottom.width, epsilon: 0.5));
    expect(top.center.dx, moreOrLessEquals(bottom.center.dx, epsilon: 0.5));
    expect(
      top.center.dx,
      moreOrLessEquals(150, epsilon: 0.5),
      reason: 'the stack sits on the 300dp column\'s centre line',
    );
    // Neither line was stretched past the column.
    expect(top.width, lessThanOrEqualTo(300));
    expect(tester.takeException(), isNull);
  });

  // What makes the stretch *proportional* rather than an equal share, and the
  // only assertion that can tell the two apart: equal shares would hand every
  // block a third of the widest line, which is less than RESTART's own label
  // needs while SHELL sits in slack — so RESTART would ellipsise. No folded
  // block may come out narrower than the width it had unfolded.
  testWidgets('stretching a line grows every block and shrinks none', (
    tester,
  ) async {
    await loadCommanderFonts();
    double widthOf(String label) => tester
        .getSize(find.widgetWithText(ChromeElbow, label.toUpperCase()))
        .width;

    await tester.pumpWidget(bar(1200));
    await tester.pumpAndSettle();
    final natural = {for (final label in labels) label: widthOf(label)};

    await tester.pumpWidget(bar(300));
    await tester.pumpAndSettle();

    for (final label in labels) {
      expect(
        widthOf(label),
        // A hair of tolerance: the widest line is stretched to its own width,
        // so its blocks grow by nothing and integer flex rounding can shave a
        // fraction off one. An equal-share stretch misses by whole dp.
        greaterThanOrEqualTo(natural[label]! - 1),
        reason: '$label was squeezed below the label it has to hold',
      );
    }
  });

  // The single-line case is the shape every existing golden pins, so folding
  // must not disturb it: with room, one line at natural widths.
  testWidgets('a run that fits stays on one line', (tester) async {
    await loadCommanderFonts();
    await tester.pumpWidget(bar(1200));
    await tester.pumpAndSettle();

    final tops = {
      for (final label in labels)
        tester
            .getRect(find.widgetWithText(ChromeElbow, label.toUpperCase()))
            .top,
    };
    expect(tops.length, 1);
  });
}
