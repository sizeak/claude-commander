import 'package:flutter/material.dart';

import '../../theme/tokens.dart';

/// Which single corner of a block is rounded.
///
/// LCARS' signature shape is a rectangle with exactly **one** large-radius
/// corner. A rail is a stack of such blocks where only the first and last are
/// rounded, which is what makes the column read as one continuous bracket
/// wrapping the content rather than a list of separate buttons.
enum ElbowCorner { topLeft, topRight, bottomLeft, bottomRight, none }

/// One LCARS block: a filled rectangle with at most one rounded corner, an
/// optionally right- or bottom-aligned label, and an optional tap target.
///
/// The geometry is the theme's, not the caller's — [CommanderTokens.elbowRadius]
/// sizes the corner — so every block in every rail agrees without each call site
/// repeating a magic number. That consistency is most of what makes the theme
/// read as LCARS rather than as coloured rectangles.
class ChromeElbow extends StatelessWidget {
  final Color color;

  /// Text colour. Defaults to the canvas, since blocks are filled with a bright
  /// accent and carry near-black text.
  final Color? labelColor;

  final ElbowCorner corner;

  /// The block's height. Treated as a **minimum** when the block carries a
  /// [label], so a label that wraps to two lines grows the block instead of
  /// being clipped — 'ADD SERVER' in a 34px block did exactly that. Null lets
  /// the block flex (the rail's inert filler).
  final double? height;

  final String? label;

  /// Where the label sits inside the block. LCARS aligns rail labels to the
  /// inboard edge — bottom-right on a top elbow, top-right on a bottom elbow,
  /// centre-right in between.
  final Alignment labelAlignment;

  final VoidCallback? onTap;

  /// Label size. Rail blocks are 11–12px; the elbow caps are 12–15px.
  final double labelSize;

  final FontWeight labelWeight;

  const ChromeElbow({
    super.key,
    required this.color,
    this.labelColor,
    this.corner = ElbowCorner.none,
    this.height,
    this.label,
    this.labelAlignment = Alignment.centerRight,
    this.onTap,
    this.labelSize = 11,
    this.labelWeight = FontWeight.w600,
  });

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final r = Radius.circular(t.elbowRadius);
    final decorated = Container(
      decoration: BoxDecoration(
        color: color,
        borderRadius: BorderRadius.only(
          topLeft: corner == ElbowCorner.topLeft ? r : Radius.zero,
          topRight: corner == ElbowCorner.topRight ? r : Radius.zero,
          bottomLeft: corner == ElbowCorner.bottomLeft ? r : Radius.zero,
          bottomRight: corner == ElbowCorner.bottomRight ? r : Radius.zero,
        ),
      ),
      padding: const EdgeInsets.fromLTRB(6, 6, 9, 6),
      alignment: label == null ? null : labelAlignment,
      child: label == null
          ? null
          // Two lines maximum, and the scaler is clamped: the block grows to fit
          // a wrapped label, but a rail only has so much vertical room to give,
          // so accessibility scaling still applies — just bounded.
          : MediaQuery.withClampedTextScaling(
              maxScaleFactor: 1.3,
              child: Text(
                label!,
                textAlign: TextAlign.right,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontFamily: t.sans,
                  fontSize: labelSize,
                  fontWeight: labelWeight,
                  letterSpacing: labelSize * 0.04,
                  height: 1.1,
                  color: labelColor ?? t.canvas,
                ),
              ),
            ),
    );
    // A labelled block may need to grow past `height`; an unlabelled one (a rail
    // filler or a colour band) is exactly the height it was given.
    final content = height == null
        ? decorated
        : label == null
        ? SizedBox(height: height, child: decorated)
        : ConstrainedBox(
            constraints: BoxConstraints(minHeight: height!),
            child: decorated,
          );
    if (onTap == null) return content;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: onTap,
      child: Semantics(button: true, label: label, child: content),
    );
  }
}

/// The short horizontal bar that caps the top of an LCARS content column,
/// closing the bracket the rail opens. Rounded on its bottom-left so it flows
/// out of the rail's top elbow.
class ChromeElbowCap extends StatelessWidget {
  final Color color;
  final double height;

  const ChromeElbowCap({super.key, required this.color, this.height = 16});

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Container(
      height: height,
      decoration: BoxDecoration(
        color: color,
        // A smaller radius than a rail elbow: the deck caps content columns at
        // roughly 12–14px against the rail's 30–44px.
        borderRadius: BorderRadius.only(
          bottomLeft: Radius.circular(t.elbowRadius * 0.4),
        ),
      ),
    );
  }
}
