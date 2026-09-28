import 'package:flutter_test/flutter_test.dart';

/// Drives real safe-area insets from the view, as the platform does.
///
/// Pins `devicePixelRatio` to 1 first, and that is not incidental:
/// [FakeViewPadding] and `tester.view.physicalSize` are in **physical** pixels
/// while `tester.getRect` returns **logical** ones, so every assertion mixing
/// the two is only true at a ratio of 1. Pinning it here means no test has to
/// know the binding's default — the same reason `useGoldenSurface` pins it
/// (`test/support/golden.dart:105-110`).
///
/// The physical size is restated at the same moment, and that is what keeps the
/// *logical* surface the size it already was: the binding's default is
/// 2400×1800 at a ratio of 3, so dropping the ratio alone would silently
/// triple the surface a widget lays out in. A test that measures a shift
/// between an unbled pump and a bled one would then be reading the surface
/// change, not the bleed.
void useInsets(
  WidgetTester tester, {
  double top = 0,
  double bottom = 0,
  double left = 0,
  double right = 0,
  // Defaults to [bottom], which is what makes the common call model a
  // keyboard-down device: `padding` and `viewPadding` equal, as they are on a
  // real platform with nothing covering the edge. Pass this separately to
  // model the keyboard up — on a real device that collapses `padding.bottom`
  // to 0 while `viewPadding.bottom` keeps the inset, and a test that sets both
  // equal cannot tell a shell reading `padding` (correct) from one reading
  // `viewPadding` (wrong).
  double? viewBottom,
}) {
  final logical = tester.view.physicalSize / tester.view.devicePixelRatio;
  tester.view.devicePixelRatio = 1.0;
  tester.view.physicalSize = logical;
  tester.view.padding = FakeViewPadding(
    top: top,
    bottom: bottom,
    left: left,
    right: right,
  );
  // `viewPadding` as well, and it is not redundant: with no keyboard up the two
  // are equal on a real platform, and `SafeArea(maintainBottomViewPadding: true)`
  // — the terminal's wrapper — reads `viewPadding.bottom` *unconditionally*
  // (`widgets/safe_area.dart:117-119`, Flutter 3.41.5), not only while the
  // keyboard is covering it. Setting `padding` alone left that SafeArea holding
  // nothing off the bottom, so the terminal's exemption looked correct in a test
  // that was measuring a page with no hold at all.
  tester.view.viewPadding = FakeViewPadding(
    top: top,
    bottom: viewBottom ?? bottom,
    left: left,
    right: right,
  );
  addTearDown(tester.view.reset);
}

/// The surface's height in the logical pixels `getRect` reports, so a test can
/// say "the physical bottom edge" without restating the ratio.
double surfaceHeight(WidgetTester tester) =>
    tester.view.physicalSize.height / tester.view.devicePixelRatio;

/// The surface's width in the logical pixels `getRect` reports, so a test can
/// say "the physical right edge" without restating the ratio or hardcoding the
/// harness's surface size.
double surfaceWidth(WidgetTester tester) =>
    tester.view.physicalSize.width / tester.view.devicePixelRatio;
