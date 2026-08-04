import 'package:flutter/material.dart';

import '../src/rust/api/diff.dart';
import '../src/rust/api/review.dart' show ReviewLineOrigin;
import 'tokens.dart';

/// Colours and text styles for the review diff: one semantic [DiffRole] in, the
/// ink to draw it in out.
///
/// This is the Dart half of the contract `diffgrid` defines — the TUI's
/// `ReviewPalette` is the other — so both frontends paint the same diff the
/// same way, and a role added upstream has exactly one place to be answered in
/// each.
///
/// ## Derived tints, not picked colours
///
/// A line's fill is its role's accent at [_lineAlpha] over the page
/// background, and word-diff emphasis is the **same accent again** at
/// [_emphasisAlpha] composited *on top of that fill* rather than chosen
/// independently. An emphasised run therefore always reads as "more of this
/// line", never as a different colour, and the two can never drift apart when
/// the palette is retuned. Flutter composites alpha for real, which is why this
/// lands more faithfully here than the terminal's approximation of it can.
///
/// Dark surfaces need a stronger tint than light ones to separate at all, so
/// the ratios below are the dark pair — and they stay correct in both themes
/// without an appearance flag, because every tint is composited over the
/// *theme's own* [CommanderTokens.canvas] (near-black in Mission Control, pure
/// black in LCARS) rather than over a background assumed here.
@immutable
class DiffTheme extends InheritedWidget {
  const DiffTheme({super.key, required this.colors, required super.child});

  final DiffColors colors;

  /// The diff palette in scope, or one derived from the ambient
  /// [CommanderTokens] when no [DiffTheme] wraps the caller — so a widget can
  /// be dropped anywhere without a wrapper and still paint in the active
  /// theme's colours rather than a baked-in default's.
  static DiffColors of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<DiffTheme>()?.colors ??
      DiffColors.fromTokens(CommanderTokens.of(context));

  @override
  bool updateShouldNotify(DiffTheme oldWidget) => oldWidget.colors != colors;
}

/// Tint strength for a changed line's fill, over the page background.
const double _lineAlpha = 0.20;

/// Tint strength for a word-diff-emphasised run, over that line's own fill.
const double _emphasisAlpha = 0.20;

/// Tint strength for the gutter cell of a changed line: a touch stronger than
/// the line, so the number column reads as its own band without breaking the
/// row apart.
const double _gutterAlpha = 0.28;

/// Single-entry memo behind [DiffColors.fromTokens]; see its doc for why one
/// entry and why it is kept at all. Top-level rather than static fields so the
/// `@immutable` palette itself stays free of mutable state.
CommanderTokens? _memoKey;
DiffColors? _memo;

/// The resolved diff palette. Construct with [DiffColors.fromTokens] — or
/// [DiffColors.derive] to pick the accents by hand — rather than listing every
/// colour: the whole point is that the emphasis tint is a function of the line
/// tint.
@immutable
class DiffColors {
  const DiffColors({
    required this.background,
    required this.gutterBackground,
    required this.additionFg,
    required this.deletionFg,
    required this.additionFill,
    required this.deletionFill,
    required this.additionEmphasis,
    required this.deletionEmphasis,
    required this.additionGutterFill,
    required this.deletionGutterFill,
    required this.contextFg,
    required this.gutterFg,
    required this.hunkHeaderFg,
    required this.hunkHeaderFill,
    required this.expandedFg,
    required this.expandedFill,
    required this.selectionFill,
    required this.alignmentGapFill,
    required this.codeStyle,
  });

  /// The surface the line tints are composited over.
  final Color background;

  /// The surface the gutter tints are composited over.
  final Color gutterBackground;

  final Color additionFg;
  final Color deletionFg;
  final Color additionFill;
  final Color deletionFill;
  final Color additionEmphasis;
  final Color deletionEmphasis;
  final Color additionGutterFill;
  final Color deletionGutterFill;
  final Color contextFg;
  final Color gutterFg;
  final Color hunkHeaderFg;
  final Color hunkHeaderFill;

  /// Context revealed out of a gap: display-only, and tinted to say so.
  final Color expandedFg;
  final Color expandedFill;

  /// The band under a selected line range.
  final Color selectionFill;

  /// The blank half of a side-by-side pair — a dim fill, so the eye reads
  /// "nothing here" rather than "not drawn yet".
  final Color alignmentGapFill;

  /// The base style for a code run: the theme's mono face at its metadata
  /// weight, carrying no run-specific ink. [spanStyle] copies the size, colour
  /// and emphasis fill on. Held as a whole style rather than a font-family name
  /// so the face's *other* defaults keep coming from
  /// [CommanderTokens.meta] instead of being restated here.
  final TextStyle codeStyle;

  /// Derives a whole palette from two accents and a pair of surfaces.
  factory DiffColors.derive({
    required Color background,
    required Color gutterBackground,
    required Color addition,
    required Color deletion,
    required Color contextFg,
    required Color gutterFg,
    required Color hunkHeaderFg,
    required Color selection,
    required TextStyle codeStyle,
  }) {
    Color over(Color base, Color tint, double alpha) =>
        Color.alphaBlend(tint.withValues(alpha: alpha), base);

    final additionFill = over(background, addition, _lineAlpha);
    final deletionFill = over(background, deletion, _lineAlpha);
    return DiffColors(
      background: background,
      gutterBackground: gutterBackground,
      additionFg: addition,
      deletionFg: deletion,
      additionFill: additionFill,
      deletionFill: deletionFill,
      // Composited ON the line fill, not on the background: emphasis is "more
      // of this line".
      additionEmphasis: over(additionFill, addition, _emphasisAlpha),
      deletionEmphasis: over(deletionFill, deletion, _emphasisAlpha),
      additionGutterFill: over(gutterBackground, addition, _gutterAlpha),
      deletionGutterFill: over(gutterBackground, deletion, _gutterAlpha),
      contextFg: contextFg,
      gutterFg: gutterFg,
      hunkHeaderFg: hunkHeaderFg,
      hunkHeaderFill: over(background, hunkHeaderFg, 0.07),
      expandedFg: gutterFg,
      expandedFill: over(background, hunkHeaderFg, 0.04),
      selectionFill: over(background, selection, 0.24),
      alignmentGapFill: over(background, gutterFg, 0.10),
      codeStyle: codeStyle,
    );
  }

  /// The diff palette for a theme's tokens — the only place the diff's roles
  /// are bound to the app's semantic ones.
  ///
  /// **The result is memoised on the token set's identity**, mirroring
  /// `terminalThemeFor`, because [DiffTheme.of] resolves this on every build of
  /// every diff view. Unlike the terminal's, this memo is an economy and not a
  /// correctness fix — [DiffColors] compares by value, so a fresh-but-equal
  /// instance costs a rebuild of nothing. A single entry, again for the same
  /// reason: `AnimatedTheme` interpolates fresh tokens every frame during a
  /// theme switch, and a map keyed on those would grow one palette per frame.
  factory DiffColors.fromTokens(CommanderTokens t) {
    final cached = _memo;
    if (cached != null && identical(_memoKey, t)) return cached;
    final built = DiffColors.derive(
      background: t.canvas,
      gutterBackground: t.diffGutterBg,
      addition: t.success,
      deletion: t.danger,
      contextFg: t.terminalFg,
      gutterFg: t.diffGutter,
      // The hunk header takes `working` — Mission Control's teal, LCARS'
      // amber — because it is structural chrome around the diff rather than
      // part of it, and neither theme's `primary` would read as that in both.
      hunkHeaderFg: t.working,
      // A selected range is an attention band — but `held`, not `attention`.
      // Both are the same amber in Mission Control, so this is identical there.
      // Under LCARS they differ, and `attention`'s salmon over black lands within
      // a few units of a deletion line's own `danger` fill, making a selected
      // deletion nearly impossible to see. `held`'s tan separates cleanly.
      selection: t.held,
      codeStyle: t.meta(),
    );
    _memoKey = t;
    _memo = built;
    return built;
  }

  /// The background a whole row of this origin sits on. `null` — a context
  /// line — keeps the page background rather than painting one.
  Color? lineFill(ReviewLineOrigin? origin) => switch (origin) {
    ReviewLineOrigin.addition => additionFill,
    ReviewLineOrigin.deletion => deletionFill,
    _ => null,
  };

  /// The gutter cell background for a row of this origin.
  Color gutterFill(ReviewLineOrigin? origin) => switch (origin) {
    ReviewLineOrigin.addition => additionGutterFill,
    ReviewLineOrigin.deletion => deletionGutterFill,
    _ => gutterBackground,
  };

  /// The background behind a word-diff-emphasised run, or `null` where the role
  /// carries no emphasis of its own (only changed lines can).
  Color? emphasisFill(DiffRole role) => switch (role) {
    DiffRole.addition => additionEmphasis,
    DiffRole.deletion => deletionEmphasis,
    _ => null,
  };

  /// The foreground for a run of the given role.
  Color foreground(DiffRole role) => switch (role) {
    DiffRole.addition => additionFg,
    DiffRole.deletion => deletionFg,
    DiffRole.hunkHeader => hunkHeaderFg,
    DiffRole.expandedContext => expandedFg,
    // `DiffRole` mirrors a `#[non_exhaustive]` Rust enum: an unrecognised role
    // renders as plain code rather than as nothing at all.
    _ => contextFg,
  };

  /// The style for one run, emphasis background included.
  TextStyle spanStyle(DiffSpanDto span, {double size = 12}) =>
      codeStyle.copyWith(
        fontSize: size,
        height: 1.45,
        color: foreground(span.role),
        backgroundColor: span.emphasis ? emphasisFill(span.role) : null,
      );

  /// The `+` / `−` sign column glyph and colour for a row of this origin.
  (String, Color) sign(ReviewLineOrigin? origin) => switch (origin) {
    ReviewLineOrigin.addition => ('+', additionFg),
    ReviewLineOrigin.deletion => ('−', deletionFg),
    _ => (' ', gutterFg),
  };

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is DiffColors &&
          other.background == background &&
          other.gutterBackground == gutterBackground &&
          other.additionFg == additionFg &&
          other.deletionFg == deletionFg &&
          other.additionFill == additionFill &&
          other.deletionFill == deletionFill &&
          other.additionEmphasis == additionEmphasis &&
          other.deletionEmphasis == deletionEmphasis &&
          other.additionGutterFill == additionGutterFill &&
          other.deletionGutterFill == deletionGutterFill &&
          other.contextFg == contextFg &&
          other.gutterFg == gutterFg &&
          other.hunkHeaderFg == hunkHeaderFg &&
          other.hunkHeaderFill == hunkHeaderFill &&
          other.expandedFg == expandedFg &&
          other.expandedFill == expandedFill &&
          other.selectionFill == selectionFill &&
          other.alignmentGapFill == alignmentGapFill &&
          other.codeStyle == codeStyle;

  @override
  int get hashCode => Object.hash(
    background,
    gutterBackground,
    additionFg,
    deletionFg,
    additionFill,
    deletionFill,
    additionEmphasis,
    deletionEmphasis,
    additionGutterFill,
    deletionGutterFill,
    contextFg,
    gutterFg,
    hunkHeaderFg,
    hunkHeaderFill,
    expandedFg,
    expandedFill,
    selectionFill,
    alignmentGapFill,
    codeStyle,
  );
}
