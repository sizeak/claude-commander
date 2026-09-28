import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/golden.dart';
import '../support/ink.dart';

/// Where a block's label sits inside it.
///
/// Measured on painted pixels, because the defect this pins is invisible to box
/// geometry: a `Text` centres its *line box*, and a typeface whose ascender
/// reaches well above its capitals then draws those capitals low inside that
/// box. Antonio does — on a Pixel 8a's lifecycle bar every label sat with 38
/// physical pixels of clearance above the caps and 29 below.
///
/// The real fonts are mandatory. Against the test binding's fallback face every
/// glyph's ink *is* its box, so a centring assertion passes on type the app
/// never renders.
void main() {
  Widget block(CommanderTokens t, double size) => RepaintBoundary(
    key: inkBoundary,
    child: MaterialApp(
      theme: themeDataFor(t),
      home: Scaffold(
        backgroundColor: t.canvas,
        body: Align(
          alignment: Alignment.topLeft,
          child: SizedBox(
            width: 300,
            child: ChromeElbow(
              color: t.borderSubtle,
              labelColor: t.nav,
              height: 90,
              label: 'SHELLXKMP',
              labelAlignment: Alignment.center,
              labelSize: size,
              labelWeight: FontWeight.w700,
            ),
          ),
        ),
      ),
    ),
  );

  /// How far the label's ink sits below the block's own centre line.
  Future<double> drop(WidgetTester tester, CommanderTokens t, double s) async {
    await loadCommanderFonts();
    await tester.pumpWidget(block(t, s));
    await tester.pumpAndSettle();
    final box = tester.getRect(find.byType(ChromeElbow));
    final ink = await inkBounds(tester, box, t.borderSubtle);
    return ink.center.dy - box.center.dy;
  }

  // A block's label sizes run 11–15; 44 is here to catch a correction applied
  // as a flat number of pixels rather than as a fraction of the font size,
  // which would look right across the narrow range the app actually uses and
  // wrong the moment anything got larger.
  //
  // The tolerance is the measurement's own floor, not slack: `inkBounds`
  // rasterises at one image pixel per logical pixel, so its centre is quantised
  // to half a pixel, and Flutter lands the baseline on the pixel grid too.
  // Uncorrected, Antonio misses by 1.5 at 11 and 7.5 at 44 — well outside it.
  for (final size in [11.0, 13.0, 44.0]) {
    testWidgets('LCARS centres a label\'s capitals at ${size.toInt()}px', (
      tester,
    ) async {
      expect(await drop(tester, lcarsTokens, size), lessThan(0.8));
    });
  }

  // The control, and the reason the correction is a per-typeface token rather
  // than one number applied everywhere: Space Grotesk already puts its capitals
  // on the centre line, so Mission Control neither needs nor gets a shift.
  // Measured uncorrected at 11/22/44/88px: 0.0, +0.5, -1.0, -0.5.
  for (final size in [11.0, 13.0, 44.0]) {
    testWidgets('Mission Control was already centred at ${size.toInt()}px', (
      tester,
    ) async {
      expect(await drop(tester, missionControlTokens, size), lessThan(0.8));
    });
  }
}
