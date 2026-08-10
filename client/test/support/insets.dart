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
  addTearDown(tester.view.reset);
}

/// The surface's height in the logical pixels `getRect` reports, so a test can
/// say "the physical bottom edge" without restating the ratio.
double surfaceHeight(WidgetTester tester) =>
    tester.view.physicalSize.height / tester.view.devicePixelRatio;
