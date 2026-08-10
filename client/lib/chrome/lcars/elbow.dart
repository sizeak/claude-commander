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

  /// A centred glyph, drawn instead of [label].
  ///
  /// An icon rather than the character it stands for, because a text glyph
  /// centres its *line box*, not its ink: the footer's create block drew a
  /// `Text('+')` whose box was dead centre while the cross painted ~2px low.
  /// A Material `Icon` is a square with its glyph centred in the em box, so
  /// centring the box centres what is seen — measured on painted pixels by
  /// `test/chrome/footer_nav_test.dart`, which is the only thing here that can
  /// hold the icon font to that.
  final IconData? icon;

  /// Where the label sits inside the block. LCARS aligns rail labels to the
  /// inboard edge — bottom-right on a top elbow, top-right on a bottom elbow,
  /// centre-right in between. An [icon] is always centred.
  final Alignment labelAlignment;

  final VoidCallback? onTap;

  /// Label size. Rail blocks are 11–12px; the elbow caps are 12–15px.
  final double labelSize;

  final FontWeight labelWeight;

  /// [icon]'s size, ignored when there is none.
  final double iconSize;

  /// Bezel this block eats. Added to the block's fill **and** to its padding in
  /// one expression, so a block can reach the screen edge without its label
  /// leaving the safe region — for a *vertical* bleed the two numbers cannot
  /// drift because they are the same number: the padding takes the whole
  /// [EdgeInsets] (below) but the fill only grows by its `vertical` getter
  /// (`grown`, below), so a horizontal component would pad without widening —
  /// asserted away rather than merely documented, since every call site today
  /// is vertical-only anyway. Supplied by `LcarsBleedScope`; zero everywhere
  /// else, which is why an unbled block is exactly the block it was.
  final EdgeInsets bleed;

  const ChromeElbow({
    super.key,
    required this.color,
    this.labelColor,
    this.corner = ElbowCorner.none,
    this.height,
    this.label,
    this.icon,
    this.labelAlignment = Alignment.centerRight,
    this.onTap,
    this.labelSize = 11,
    this.labelWeight = FontWeight.w600,
    this.iconSize = 18,
    this.bleed = EdgeInsets.zero,
  }) : assert(
         label == null || icon == null,
         'a block carries one or the other',
       );

  @override
  Widget build(BuildContext context) {
    // Dart cannot evaluate `EdgeInsets` field access inside a const
    // constructor's initializer list (it is not a primitive type), so this
    // is checked here instead of alongside the `label`/`icon` assert above.
    assert(bleed.left == 0 && bleed.right == 0, 'bleed is vertical only');
    final t = CommanderTokens.of(context);
    final r = Radius.circular(t.elbowRadius);
    final centred = icon != null || labelAlignment == Alignment.center;
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
      // An edge-aligned label sits against the block's inboard edge, so the
      // padding is biased that way (the extra 3px on the right is the gutter a
      // rail's right-aligned labels keep off the content column). Centred
      // content has no edge to sit against, and the bias would push it 1.5px
      // off the block's own centre line — visible on the footer's create block,
      // which is only 46px wide.
      padding:
          (centred
              ? const EdgeInsets.symmetric(horizontal: 8, vertical: 6)
              : const EdgeInsets.fromLTRB(6, 6, 9, 6)) +
          bleed,
      alignment: icon != null
          ? Alignment.center
          : (label == null ? null : labelAlignment),
      child: icon != null
          ? Icon(icon, size: iconSize, color: labelColor ?? t.canvas)
          : label == null
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
    // The same bleed again, and deliberately so: growing the box without growing
    // the padding would slide the label into the bezel.
    final grown = height == null ? null : height! + bleed.vertical;
    // A labelled block may need to grow past `height`; an unlabelled one (a rail
    // filler or a colour band) is exactly the height it was given.
    final content = grown == null
        ? decorated
        : label == null
        ? SizedBox(height: grown, child: decorated)
        : ConstrainedBox(
            constraints: BoxConstraints(minHeight: grown),
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

  /// Bezel this cap eats; added straight to its height. It carries no content,
  /// so there is nothing to compensate.
  final EdgeInsets bleed;

  const ChromeElbowCap({
    super.key,
    required this.color,
    this.height = 16,
    this.bleed = EdgeInsets.zero,
  });

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Container(
      height: height + bleed.vertical,
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
