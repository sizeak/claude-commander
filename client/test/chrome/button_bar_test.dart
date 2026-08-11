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
