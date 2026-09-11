import 'package:flutter/material.dart';

import '../../theme/tokens.dart';

/// A [ChromeElbow]'s padding around centred content, and the reason it is
/// public: `lcars_chrome.dart`'s button bar has to know how wide a block will
/// come out *before* building it, so it can fold a run that would not fit onto
/// a second line. Measuring against a copy of this number would let the two
/// drift silently — a block one pixel wider than the folder believed is a run
/// that overflows again.
const kElbowCentredPadding = EdgeInsets.symmetric(horizontal: 8, vertical: 6);

/// The ceiling [ChromeElbow] clamps accessibility text scaling to. A rail only
/// has so much vertical room to give, so scaling applies — just bounded.
/// Public for the same reason as [kElbowCentredPadding].
const kElbowMaxTextScale = 1.3;

/// The style [ChromeElbow] draws a label in. Public so a caller that must
/// predict a block's width lays the same text out the block will.
TextStyle elbowLabelStyle(
  CommanderTokens t, {
  required double size,
  required FontWeight weight,
  Color? color,
}) => TextStyle(
  fontFamily: t.sans,
  fontSize: size,
  fontWeight: weight,
  letterSpacing: size * 0.04,
  height: 1.1,
  color: color,
);

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
    // A corner is rounded only while it faces the canvas. `edge` is the bleed
    // on the screen edge that corner sits against, and once the block grows
    // into it the radius stops curving the bracket *into* the canvas and starts
    // biting a quarter-circle *out of* the screen's own corner — measured on a
    // Pixel 8a, the rail's amber band only reached x=0 at y=84, leaving a 32dp
    // black wedge in the top-left of the display and its mirror at the bottom.
    // Note this is about a corner sitting *on the bezel*, which is why
    // [ChromeElbowCap] does not do the same: its bottom-left is interior, faces
    // the canvas at every inset, and stays rounded.
    Radius radiusFor(ElbowCorner which, double edge) =>
        corner == which && edge == 0 ? r : Radius.zero;
    final centred = icon != null || labelAlignment == Alignment.center;
    final decorated = Container(
      decoration: BoxDecoration(
        color: color,
        borderRadius: BorderRadius.only(
          topLeft: radiusFor(ElbowCorner.topLeft, bleed.top),
          topRight: radiusFor(ElbowCorner.topRight, bleed.top),
          bottomLeft: radiusFor(ElbowCorner.bottomLeft, bleed.bottom),
          bottomRight: radiusFor(ElbowCorner.bottomRight, bleed.bottom),
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
              ? kElbowCentredPadding
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
              maxScaleFactor: kElbowMaxTextScale,
              // Lifted by the typeface's own cap drop, and only where the label
              // claims to be vertically centred — an edge-aligned one is not.
              // A `Transform` rather than padding: this must move the glyphs
              // without changing the block's height, which several tests and
              // every golden pin.
              child: Transform.translate(
                offset: Offset(
                  0,
                  labelAlignment.y == 0 ? -labelSize * t.capInkDrop : 0,
                ),
                child: Text(
                  label!,
                  textAlign: TextAlign.right,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: elbowLabelStyle(
                    t,
                    size: labelSize,
                    weight: labelWeight,
                    color: labelColor ?? t.canvas,
                  ),
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

/// [ChromeElbowCap]'s height when unbled — the desktop, tablet and
/// zero-inset shape. Every existing golden depends on this exact value, so it
/// stays fixed; only the *bled* cap's height moved away from it, to
/// [kElbowCapBledHeight].
const kElbowCapHeight = 16.0;

/// The bled cap's own height, added to the inset it bleeds into — see
/// [ChromeElbowCap.build]. Deliberately independent of [kElbowCapHeight]: the
/// bled cap used to add the *unbled* height to the inset (`inset + 16`), which on
/// a Pixel 8a's 46dp status-bar inset (devicePixelRatio 2.625) produced a
/// 62dp band — 35% taller than the status bar itself, and read on device as
/// an oversized slab. The user asked for the band to stop essentially where
/// the status bar does; measured on a Pixel 8a, 1dp here gives a 47.2dp band
/// against the 46dp inset — a 1.1dp overhang.
const kElbowCapBledHeight = 1.0;

/// The short horizontal bar that caps the top of an LCARS content column,
/// closing the bracket the rail opens. Rounded on its bottom-left so it flows
/// out of the rail's top elbow.
class ChromeElbowCap extends StatelessWidget {
  final Color color;

  /// The cap's height when unbled. Ignored once [bleed] has a top inset, which
  /// swaps in [kElbowCapBledHeight] on top of the inset instead.
  final double height;

  /// Bezel this cap eats. It carries no content, so there is nothing to
  /// compensate; all it changes is the cap's height.
  final EdgeInsets bleed;

  const ChromeElbowCap({
    super.key,
    required this.color,
    this.height = kElbowCapHeight,
    this.bleed = EdgeInsets.zero,
  });

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Container(
      // `bleed.top`, not `bleed.vertical`: a cap closes the *top* of a content
      // column, so a bottom inset has no business in its height. Every caller
      // hands a cap a top-only bleed, which is why summing both once looked
      // equivalent — until the same arithmetic was shared with a fill that got
      // the frame's whole bleed, and the bottom inset ran it 24dp past the cap
      // on a gesture-nav Pixel 8a.
      height: bleed.top > 0 ? bleed.top + kElbowCapBledHeight : height,
      decoration: BoxDecoration(
        color: color,
        // A smaller radius than a rail elbow: the deck caps content columns at
        // roughly 12–14px against the rail's 30–44px.
        //
        // Unconditional, and it took a round trip to get back here. The radius
        // exists so the cap *flows out of* the rail's top elbow, which needs the
        // gap between the two to curve across; while that gap was filled behind
        // the status bar the curve had nothing to flow out of and bit a black
        // wedge into a solid band, so a bled cap squared it. That left the band
        // and the rail meeting at a bare 90° inner corner, which the user
        // judged worse than the notch an open gap costs. The fill is gone (see
        // `bleed.dart`), the gap is open at every inset, and the curve is back
        // unconditionally.
        borderRadius: BorderRadius.only(
          bottomLeft: Radius.circular(t.elbowRadius * 0.4),
        ),
      ),
    );
  }
}
