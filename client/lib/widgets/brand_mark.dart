import 'package:flutter/material.dart';

import '../theme/app_colors.dart';

/// The Commander brand mark: three stacked chevrons ("rank" marks) painted with
/// the vertical brand gradient, on the deck's rounded slate tile. Used in the
/// connect screen, the Fleet header/rail, and notifications.
class BrandMark extends StatelessWidget {
  /// Overall side length of the rounded tile.
  final double size;

  /// When false, only the chevron marks are drawn (no tile background).
  final bool tile;

  const BrandMark({super.key, this.size = 32, this.tile = true});

  @override
  Widget build(BuildContext context) {
    final marks = CustomPaint(
      size: Size.square(size),
      painter: _ChevronPainter(),
    );
    if (!tile) return SizedBox.square(dimension: size, child: marks);
    return Container(
      width: size,
      height: size,
      padding: EdgeInsets.all(size * 0.16),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(size * 0.26),
        gradient: const LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Color(0xFF2B2D3A), Color(0xFF1B1C26)],
        ),
        border: Border.all(color: Colors.white.withValues(alpha: 0.05)),
      ),
      child: marks,
    );
  }
}

class _ChevronPainter extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    // The deck draws on a 1024×1024 viewBox; scale our three chevron polylines
    // (up-pointing carets at increasing y) to fit.
    const vb = 1024.0;
    final s = size.width / vb;
    Offset p(double x, double y) => Offset(x * s, y * s);

    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 98 * s
      ..strokeCap = StrokeCap.square
      ..strokeJoin = StrokeJoin.miter
      ..strokeMiterLimit = 6
      ..shader = const LinearGradient(
        begin: Alignment.topCenter,
        end: Alignment.bottomCenter,
        colors: AppColors.brandGradient,
        stops: [0.0, 0.5, 1.0],
      ).createShader(Rect.fromLTWH(0, 0, size.width, size.height));

    for (final dy in const [0.0, 150.0, 300.0]) {
      final path = Path()
        ..moveTo(p(300, 470 + dy).dx, p(300, 470 + dy).dy)
        ..lineTo(p(512, 288 + dy).dx, p(512, 288 + dy).dy)
        ..lineTo(p(724, 470 + dy).dx, p(724, 470 + dy).dy);
      canvas.drawPath(path, paint);
    }
  }

  @override
  bool shouldRepaint(_ChevronPainter oldDelegate) => false;
}
