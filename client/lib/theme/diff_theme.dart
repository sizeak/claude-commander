import 'package:flutter/material.dart';

import '../src/rust/api/diff.dart';
import '../src/rust/api/review.dart' show ReviewLineOrigin;
import 'app_colors.dart';
import 'app_theme.dart';

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
/// the palette is retuned. Dark surfaces need a stronger tint than light ones
/// to separate at all, so the ratios are chosen per appearance; this app is
/// dark-only and takes the dark pair. Flutter composites alpha for real, which
/// is why this lands more faithfully here than the terminal's approximation of
/// it can.
@immutable
class DiffTheme extends InheritedWidget {
  const DiffTheme({super.key, required this.colors, required super.child});

  final DiffColors colors;

  /// The diff palette in scope, or the app default when no [DiffTheme] wraps
  /// the caller — so a widget can be dropped anywhere without a wrapper.
  static DiffColors of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<DiffTheme>()?.colors ??
      DiffColors.dark;

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

/// The resolved diff palette. Construct with [DiffColors.derive] rather than
/// listing colours by hand — the whole point is that the emphasis tint is a
/// function of the line tint.
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
    );
  }

  /// The app's dark diff palette.
  static final DiffColors dark = DiffColors.derive(
    background: AppColors.bg,
    gutterBackground: AppColors.diffGutterBg,
    addition: AppColors.green,
    deletion: AppColors.red,
    contextFg: AppColors.terminalFg,
    gutterFg: AppColors.diffGutter,
    hunkHeaderFg: AppColors.teal,
    selection: AppColors.amber,
  );

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
  TextStyle spanStyle(DiffSpanDto span, {double size = 12}) => AppTheme.mono(
    size: size,
    color: foreground(span.role),
    height: 1.45,
  ).copyWith(backgroundColor: span.emphasis ? emphasisFill(span.role) : null);

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
          other.alignmentGapFill == alignmentGapFill;

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
  );
}
