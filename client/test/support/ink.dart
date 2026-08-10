import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

/// The key [inkCentre] rasterises. Put it on a [RepaintBoundary] wrapping
/// whatever the test wants measured — the footer alone, or the whole shell.
const inkBoundary = ValueKey('ink-boundary');

/// The ink centroid of [rect] — in the same coordinates `tester.getRect`
/// reports, so a caller measures a widget and hands the result straight over.
///
/// Weighted by how far each pixel is from [fill]. A centroid rather than a
/// bounding box: both are exact for a symmetric glyph, but antialiased edge
/// pixels perturb a bbox asymmetrically and cancel out in a weighted mean.
///
/// Pixels rather than widget boxes, because the defect this pins was invisible
/// to box geometry. The LCARS footer's create block used to draw a `Text('+')`,
/// and a text glyph centres its *line box*, not its ink: '+' sits above the
/// baseline, so the box could be dead centre while the cross painted ~2px low.
/// The horizontal half of the same bug came from `ChromeElbow`'s inboard
/// padding bias (6 left, 9 right), which is right for a rail's right-aligned
/// labels and wrong for a centred one.
///
/// [RenderRepaintBoundary.toImage] rasterises the layer at one image pixel per
/// logical pixel by default (`pixelRatio` defaults to 1.0), so this is
/// independent of the view's `devicePixelRatio`.
Future<Offset> inkCentre(WidgetTester tester, Rect rect, Color fill) async {
  final raster = await _rasterise(tester);
  final local = rect.shift(-raster.origin);
  var weight = 0.0, sx = 0.0, sy = 0.0;
  for (var y = local.top.ceil(); y < local.bottom.floor(); y++) {
    for (var x = local.left.ceil(); x < local.right.floor(); x++) {
      final i = (y * raster.image.width + x) * 4;
      final w =
          ((raster.data.getUint8(i) - (fill.r * 255)).abs() +
              (raster.data.getUint8(i + 1) - (fill.g * 255)).abs() +
              (raster.data.getUint8(i + 2) - (fill.b * 255)).abs()) /
          765;
      weight += w;
      sx += w * (x + 0.5);
      sy += w * (y + 0.5);
    }
  }
  expect(weight, greaterThan(1), reason: 'the block painted no glyph at all');
  return Offset(sx / weight, sy / weight) + raster.origin;
}

/// The colour of the single pixel at [point], in the same coordinates
/// `tester.getRect` reports.
///
/// Sampled from the same rasterisation [inkCentre] uses, but returned raw
/// rather than reduced to a centroid — useful where the question is "is this
/// exact pixel painted with X" rather than "where does a glyph's ink sit".
Future<Color> pixelAt(WidgetTester tester, Offset point) async {
  final raster = await _rasterise(tester);
  final local = point - raster.origin;
  final x = local.dx.floor();
  final y = local.dy.floor();
  final i = (y * raster.image.width + x) * 4;
  return Color.fromARGB(
    raster.data.getUint8(i + 3),
    raster.data.getUint8(i),
    raster.data.getUint8(i + 1),
    raster.data.getUint8(i + 2),
  );
}

/// Rasterises the [inkBoundary] repaint boundary once, so [inkCentre] and
/// [pixelAt] share the same `toImage`/`toByteData` dance rather than each
/// repeating it.
Future<({ui.Image image, ByteData data, Offset origin})> _rasterise(
  WidgetTester tester,
) async {
  final finder = find.byKey(inkBoundary);
  final origin = tester.getTopLeft(finder);
  final image = await tester.runAsync(
    () => (tester.renderObject(finder) as RenderRepaintBoundary).toImage(),
  );
  final data = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.rawRgba),
  );
  addTearDown(image!.dispose);
  return (image: image, data: data!, origin: origin);
}
