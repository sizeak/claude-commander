import 'dart:ui' show lerpDouble;

import 'package:flutter/material.dart';

/// A session's visual state, independent of any theme's colours.
///
/// Replaces the raw `Color` that `StateDescriptor` used to carry. That mattered
/// for more than tidiness: two call sites compared `descriptor.color ==
/// AppColors.amber` to mean "this row wants attention", and because *both*
/// `waitingForInput` and `cascadePaused` were amber, the check silently meant
/// "waiting **or** cascade-paused". Splitting them into [waiting] and [held]
/// without preserving that union would have dropped the amber tint from paused
/// cascades — see `sessionWantsAttention`.
enum SessionTone {
  /// Agent actively working.
  working,

  /// Agent blocked on a human answer.
  waiting,

  /// Cascade paused mid-stack, awaiting a decision.
  held,

  /// Finished with output the user hasn't seen.
  unread,

  /// Running but doing nothing.
  idle,

  /// Lifecycle-stopped.
  stopped,

  /// Being created.
  creating,

  /// Mid-merge.
  merging,

  /// Mid-push.
  pushing,
}

/// True when [tone] should pull the eye to its row — the themed replacement for
/// the old `descriptor.color == AppColors.amber` test. Deliberately the **union**
/// of [SessionTone.waiting] and [SessionTone.held], because that is what the
/// pre-theming check resolved to when both states shared one amber.
bool sessionWantsAttention(SessionTone tone) =>
    tone == SessionTone.waiting || tone == SessionTone.held;

/// How one session state paints: its accent (glyph, leading block, border), the
/// text colour to use *on* [tintedSurface], and the row fill itself.
///
/// A triple rather than a single colour because LCARS panel rows are genuinely
/// three-part — a solid colour block, a 2px top border, and a distinct tinted
/// near-black body. Returning one `Color` would push the fill derivation out to
/// every call site, which is what the pre-theming code did ad hoc with
/// `withValues(alpha:)` and which the two themes must do differently.
@immutable
class ToneStyle {
  /// Glyph / leading block / top border.
  final Color accent;

  /// Text sitting on [tintedSurface]. Often a lightened [accent] so small text
  /// keeps its contrast against the tint.
  final Color onTint;

  /// The row body. May be translucent (Mission Control tints with alpha over
  /// the canvas); LCARS uses hand-tuned opaque near-blacks from the deck.
  final Color tintedSurface;

  const ToneStyle({
    required this.accent,
    required this.onTint,
    required this.tintedSurface,
  });

  static ToneStyle lerp(ToneStyle a, ToneStyle b, double t) => ToneStyle(
    accent: Color.lerp(a.accent, b.accent, t)!,
    onTint: Color.lerp(a.onTint, b.onTint, t)!,
    tintedSurface: Color.lerp(a.tintedSurface, b.tintedSurface, t)!,
  );
}

/// Which family of chrome — page frames, rails, list-row shapes, button bars — a
/// theme renders.
///
/// Lives here rather than in a second `ThemeExtension` because `ThemeId` already
/// holds its own [CommanderTokens], so tokens cannot hold a `ThemeId` back
/// without a circular import. A plain enum breaks the cycle with no extra
/// plumbing, and `Chrome.of` maps it to the implementation.
enum ChromeKind { missionControl, lcars }

/// Set true by `main()` so a missing [CommanderTokens] extension is loud in the
/// running app but silent under `flutter_test`.
///
/// [CommanderTokens.of] falls back to [missionControlTokens] when no extension
/// is installed, which is what lets the existing widget tests keep pumping bare
/// widgets with no theme wrapper. In production that same fallback would render
/// the wrong theme with no signal at all, so the app opts into the assert.
bool debugAssertTokensPresent = false;

/// Every colour, face and measurement either theme varies, keyed by **role**
/// rather than hue.
///
/// Roles, not hues, because the themes disagree about which states share a
/// colour. Mission Control paints "waiting" and "cascade held" the same amber;
/// LCARS gives them salmon and tan respectively. Likewise `working` is teal in
/// Mission Control but amber in LCARS, where amber is also `primary`. A palette
/// keyed by hue cannot express both mappings; one keyed by role can.
@immutable
class CommanderTokens extends ThemeExtension<CommanderTokens> {
  // ── Surfaces ───────────────────────────────────────────────────────────
  /// Deepest surface: live terminal output.
  final Color terminalBg;

  /// The app scaffold.
  final Color canvas;

  /// Raised strips — tab bars, status bars, diff gutters.
  final Color canvasRaised;

  /// Cards, inputs, icon buttons.
  final Color surface;

  /// A selected row or segment.
  final Color surfaceSelected;

  // ── Borders ────────────────────────────────────────────────────────────
  final Color border;
  final Color borderSubtle;
  final Color divider;

  // ── Text ramp ──────────────────────────────────────────────────────────
  final Color text;
  final Color textBright;
  final Color textMuted;
  final Color textFaint;
  final Color textDim;

  // ── Semantic roles ─────────────────────────────────────────────────────
  /// Buttons, selection, brand.
  final Color primary;

  /// A lighter [primary] for badges and secondary emphasis.
  final Color primarySoft;

  /// Agent working / connected / pushing. Teal in Mission Control, amber in
  /// LCARS (the deck's RUN blocks).
  final Color working;

  /// Navigation chrome. Mission Control has no separate nav hue and reuses
  /// [primary]; LCARS uses lilac.
  final Color nav;

  /// Informational / in-flight lifecycle (creating, merging).
  final Color info;

  /// Finished-but-unseen.
  final Color unread;

  /// Needs a human answer.
  final Color attention;

  /// Text on an [attention]-tinted fill — brighter than [attention] so small
  /// captions stay readable.
  final Color attentionOn;

  /// Cascade held. Shares amber with [attention] in Mission Control; distinct
  /// in LCARS.
  final Color held;

  final Color success;
  final Color danger;
  final Color idle;

  // ── Terminal / diff ────────────────────────────────────────────────────
  final Color terminalFg;
  final Color diffGutter;
  final Color diffGutterBg;

  /// The runtime `BrandMark` ramp, top → bottom. LCARS collapses it to a solid
  /// amber stroke. Distinct from the launcher-icon gradient in `BrandAssets`,
  /// which is baked into a static asset and never themed.
  final List<Color> brandGradient;

  // ── Type ───────────────────────────────────────────────────────────────
  /// UI face. Space Grotesk in Mission Control, condensed Antonio in LCARS.
  final String sans;

  /// Terminal, code, and metadata face. JetBrains Mono in **both** themes —
  /// the deck is explicit that real agent output stays monospace, so only the
  /// terminal's frame gets themed.
  final String mono;

  /// Whether chrome uppercases and letterspaces its labels (LCARS does).
  final bool uppercaseLabels;

  /// Which chrome implementation renders this theme's structure.
  final ChromeKind chrome;

  // ── Geometry ───────────────────────────────────────────────────────────
  final double cardRadius;

  /// Corner radius for inputs and buttons. Distinct from [cardRadius] because
  /// Mission Control uses 12 here against 13 for cards.
  final double controlRadius;

  final double pillRadius;

  /// The large radius on an LCARS elbow's single rounded corner. Zero in
  /// Mission Control, which has no elbows.
  final double elbowRadius;

  /// Width of the LCARS portrait rail. Zero in Mission Control.
  final double railWidth;

  /// Thickness of an LCARS panel's coloured top border. Zero in Mission
  /// Control, which uses an all-round [border] instead.
  final double panelTopBorder;

  /// The full per-tone paint table. Explicit rather than derived: LCARS' row
  /// fills are hand-tuned near-blacks from the deck (`#1C1010`, `#100E08`,
  /// `#0D0D14`…) that no single alpha-blend formula reproduces.
  final Map<SessionTone, ToneStyle> tones;

  const CommanderTokens({
    required this.terminalBg,
    required this.canvas,
    required this.canvasRaised,
    required this.surface,
    required this.surfaceSelected,
    required this.border,
    required this.borderSubtle,
    required this.divider,
    required this.text,
    required this.textBright,
    required this.textMuted,
    required this.textFaint,
    required this.textDim,
    required this.primary,
    required this.primarySoft,
    required this.working,
    required this.nav,
    required this.info,
    required this.unread,
    required this.attention,
    required this.attentionOn,
    required this.held,
    required this.success,
    required this.danger,
    required this.idle,
    required this.terminalFg,
    required this.diffGutter,
    required this.diffGutterBg,
    required this.brandGradient,
    required this.sans,
    required this.mono,
    required this.uppercaseLabels,
    required this.chrome,
    required this.cardRadius,
    required this.controlRadius,
    required this.pillRadius,
    required this.elbowRadius,
    required this.railWidth,
    required this.panelTopBorder,
    required this.tones,
  });

  /// How [tone] paints under this theme.
  ToneStyle toneStyle(SessionTone tone) => tones[tone]!;

  /// The active token set, falling back to [missionControlTokens] when no
  /// extension is installed — see [debugAssertTokensPresent] for why.
  static CommanderTokens of(BuildContext context) {
    final tokens = Theme.of(context).extension<CommanderTokens>();
    assert(
      tokens != null || !debugAssertTokensPresent,
      'No CommanderTokens in the theme — this subtree would silently render '
      'Mission Control colours. Ensure MaterialApp.theme comes from '
      'themeDataFor().',
    );
    return tokens ?? missionControlTokens;
  }

  /// An uppercase, letterspaced section eyebrow ("FILES CHANGED").
  TextStyle eyebrow({Color? color}) => TextStyle(
    fontFamily: mono,
    fontSize: 9.5,
    fontWeight: FontWeight.w600,
    letterSpacing: 1.4,
    color: color ?? textFaint,
  );

  /// The recurring small monospace metadata caption.
  TextStyle meta({
    double size = 11,
    FontWeight weight = FontWeight.w500,
    Color? color,
    double? height,
    double? letterSpacing,
  }) => TextStyle(
    fontFamily: mono,
    fontSize: size,
    fontWeight: weight,
    color: color ?? textMuted,
    height: height,
    letterSpacing: letterSpacing,
  );

  /// A screen or section title in the theme's display face.
  TextStyle display({
    double size = 23,
    FontWeight weight = FontWeight.w700,
    Color? color,
  }) => TextStyle(
    fontFamily: sans,
    fontSize: size,
    fontWeight: weight,
    // LCARS' condensed face wants positive tracking; Space Grotesk is tightened.
    letterSpacing: uppercaseLabels ? size * 0.04 : -0.3,
    color: color ?? (uppercaseLabels ? primary : text),
  );

  /// [label] cased for this theme — uppercased under LCARS, verbatim otherwise.
  String caseLabel(String label) =>
      uppercaseLabels ? label.toUpperCase() : label;

  @override
  CommanderTokens copyWith({
    Color? terminalBg,
    Color? canvas,
    Color? canvasRaised,
    Color? surface,
    Color? surfaceSelected,
    Color? border,
    Color? borderSubtle,
    Color? divider,
    Color? text,
    Color? textBright,
    Color? textMuted,
    Color? textFaint,
    Color? textDim,
    Color? primary,
    Color? primarySoft,
    Color? working,
    Color? nav,
    Color? info,
    Color? unread,
    Color? attention,
    Color? attentionOn,
    Color? held,
    Color? success,
    Color? danger,
    Color? idle,
    Color? terminalFg,
    Color? diffGutter,
    Color? diffGutterBg,
    List<Color>? brandGradient,
    String? sans,
    String? mono,
    bool? uppercaseLabels,
    ChromeKind? chrome,
    double? cardRadius,
    double? controlRadius,
    double? pillRadius,
    double? elbowRadius,
    double? railWidth,
    double? panelTopBorder,
    Map<SessionTone, ToneStyle>? tones,
  }) => CommanderTokens(
    terminalBg: terminalBg ?? this.terminalBg,
    canvas: canvas ?? this.canvas,
    canvasRaised: canvasRaised ?? this.canvasRaised,
    surface: surface ?? this.surface,
    surfaceSelected: surfaceSelected ?? this.surfaceSelected,
    border: border ?? this.border,
    borderSubtle: borderSubtle ?? this.borderSubtle,
    divider: divider ?? this.divider,
    text: text ?? this.text,
    textBright: textBright ?? this.textBright,
    textMuted: textMuted ?? this.textMuted,
    textFaint: textFaint ?? this.textFaint,
    textDim: textDim ?? this.textDim,
    primary: primary ?? this.primary,
    primarySoft: primarySoft ?? this.primarySoft,
    working: working ?? this.working,
    nav: nav ?? this.nav,
    info: info ?? this.info,
    unread: unread ?? this.unread,
    attention: attention ?? this.attention,
    attentionOn: attentionOn ?? this.attentionOn,
    held: held ?? this.held,
    success: success ?? this.success,
    danger: danger ?? this.danger,
    idle: idle ?? this.idle,
    terminalFg: terminalFg ?? this.terminalFg,
    diffGutter: diffGutter ?? this.diffGutter,
    diffGutterBg: diffGutterBg ?? this.diffGutterBg,
    brandGradient: brandGradient ?? this.brandGradient,
    sans: sans ?? this.sans,
    mono: mono ?? this.mono,
    uppercaseLabels: uppercaseLabels ?? this.uppercaseLabels,
    chrome: chrome ?? this.chrome,
    cardRadius: cardRadius ?? this.cardRadius,
    controlRadius: controlRadius ?? this.controlRadius,
    pillRadius: pillRadius ?? this.pillRadius,
    elbowRadius: elbowRadius ?? this.elbowRadius,
    railWidth: railWidth ?? this.railWidth,
    panelTopBorder: panelTopBorder ?? this.panelTopBorder,
    tones: tones ?? this.tones,
  );

  @override
  CommanderTokens lerp(ThemeExtension<CommanderTokens>? other, double t) {
    if (other is! CommanderTokens) return this;
    return CommanderTokens(
      terminalBg: Color.lerp(terminalBg, other.terminalBg, t)!,
      canvas: Color.lerp(canvas, other.canvas, t)!,
      canvasRaised: Color.lerp(canvasRaised, other.canvasRaised, t)!,
      surface: Color.lerp(surface, other.surface, t)!,
      surfaceSelected: Color.lerp(surfaceSelected, other.surfaceSelected, t)!,
      border: Color.lerp(border, other.border, t)!,
      borderSubtle: Color.lerp(borderSubtle, other.borderSubtle, t)!,
      divider: Color.lerp(divider, other.divider, t)!,
      text: Color.lerp(text, other.text, t)!,
      textBright: Color.lerp(textBright, other.textBright, t)!,
      textMuted: Color.lerp(textMuted, other.textMuted, t)!,
      textFaint: Color.lerp(textFaint, other.textFaint, t)!,
      textDim: Color.lerp(textDim, other.textDim, t)!,
      primary: Color.lerp(primary, other.primary, t)!,
      primarySoft: Color.lerp(primarySoft, other.primarySoft, t)!,
      working: Color.lerp(working, other.working, t)!,
      nav: Color.lerp(nav, other.nav, t)!,
      info: Color.lerp(info, other.info, t)!,
      unread: Color.lerp(unread, other.unread, t)!,
      attention: Color.lerp(attention, other.attention, t)!,
      attentionOn: Color.lerp(attentionOn, other.attentionOn, t)!,
      held: Color.lerp(held, other.held, t)!,
      success: Color.lerp(success, other.success, t)!,
      danger: Color.lerp(danger, other.danger, t)!,
      idle: Color.lerp(idle, other.idle, t)!,
      terminalFg: Color.lerp(terminalFg, other.terminalFg, t)!,
      diffGutter: Color.lerp(diffGutter, other.diffGutter, t)!,
      diffGutterBg: Color.lerp(diffGutterBg, other.diffGutterBg, t)!,
      brandGradient: [
        for (var i = 0; i < brandGradient.length; i++)
          Color.lerp(brandGradient[i], other.brandGradient[i], t)!,
      ],
      // Faces and flags snap at the midpoint — there is no meaningful
      // interpolation between two font families or between cased and uncased.
      sans: t < 0.5 ? sans : other.sans,
      mono: t < 0.5 ? mono : other.mono,
      uppercaseLabels: t < 0.5 ? uppercaseLabels : other.uppercaseLabels,
      // Structure cannot be interpolated, so it swaps at the midpoint.
      chrome: t < 0.5 ? chrome : other.chrome,
      cardRadius: lerpDouble(cardRadius, other.cardRadius, t)!,
      controlRadius: lerpDouble(controlRadius, other.controlRadius, t)!,
      pillRadius: lerpDouble(pillRadius, other.pillRadius, t)!,
      elbowRadius: lerpDouble(elbowRadius, other.elbowRadius, t)!,
      railWidth: lerpDouble(railWidth, other.railWidth, t)!,
      panelTopBorder: lerpDouble(panelTopBorder, other.panelTopBorder, t)!,
      tones: {
        for (final tone in SessionTone.values)
          tone: ToneStyle.lerp(toneStyle(tone), other.toneStyle(tone), t),
      },
    );
  }
}

/// The default theme: dark, terminal-DNA, violet/teal, Space Grotesk. These
/// values are the pre-theming `AppColors` palette exactly — `tokens_test.dart`
/// pins every one against a frozen reference so the migration off ~357 static
/// references cannot quietly retune a colour.
const missionControlTokens = CommanderTokens(
  terminalBg: Color(0xFF07090C),
  canvas: Color(0xFF0A0C10),
  canvasRaised: Color(0xFF0C0E13),
  surface: Color(0xFF12151B),
  surfaceSelected: Color(0xFF242938),
  border: Color(0xFF232838),
  borderSubtle: Color(0xFF1C1F28),
  divider: Color(0xFF14171D),
  text: Color(0xFFE7EBF2),
  textBright: Color(0xFFC9CDD6),
  textMuted: Color(0xFF8891A3),
  textFaint: Color(0xFF6B7488),
  textDim: Color(0xFF556072),
  primary: Color(0xFF7C6CFF),
  primarySoft: Color(0xFFA99DFF),
  working: Color(0xFF3FD6D0),
  // Mission Control never had a nav or info hue of its own; it reused the
  // violet accent and its lighter variant. Preserved deliberately — inventing
  // a distinction here would be a visible change during a neutral refactor.
  nav: Color(0xFF7C6CFF),
  info: Color(0xFFA99DFF),
  unread: Color(0xFF7C6CFF),
  attention: Color(0xFFF5B545),
  attentionOn: Color(0xFFF5C877),
  held: Color(0xFFF5B545),
  success: Color(0xFF5FD88A),
  danger: Color(0xFFFF8A9B),
  idle: Color(0xFF5B6478),
  terminalFg: Color(0xFFC8CCD4),
  diffGutter: Color(0xFF454B57),
  diffGutterBg: Color(0xFF0C0E13),
  brandGradient: [Color(0xFFB3A6FF), Color(0xFF7C9DFF), Color(0xFF3FD6D0)],
  sans: 'SpaceGrotesk',
  mono: 'JetBrainsMono',
  uppercaseLabels: false,
  chrome: ChromeKind.missionControl,
  cardRadius: 13,
  controlRadius: 12,
  pillRadius: 20,
  elbowRadius: 0,
  railWidth: 0,
  panelTopBorder: 0,
  tones: _mcTones,
);

/// Mission Control's tone table, reproducing the pre-theming
/// `StateDescriptor` colours one for one. Note `waiting` and `held` are
/// identical — that is not an oversight, it is what the old palette did, and
/// `sessionWantsAttention` depends on it.
const _mcTones = <SessionTone, ToneStyle>{
  SessionTone.working: ToneStyle(
    accent: Color(0xFF3FD6D0),
    onTint: Color(0xFF8891A3),
    tintedSurface: Color(0xFF12151B),
  ),
  SessionTone.waiting: ToneStyle(
    accent: Color(0xFFF5B545),
    onTint: Color(0xFFF5C877),
    // amber @ 9%, as `_GroupedTile` derived it by hand. Written as a literal
    // because a const cannot call `withValues`; 0x17/255 = 0.0902 against the
    // old 0.09, a difference of 2/10000 in alpha.
    tintedSurface: Color(0x17F5B545),
  ),
  SessionTone.held: ToneStyle(
    accent: Color(0xFFF5B545),
    onTint: Color(0xFFF5C877),
    tintedSurface: Color(0x17F5B545),
  ),
  SessionTone.unread: ToneStyle(
    accent: Color(0xFF7C6CFF),
    onTint: Color(0xFFA99DFF),
    tintedSurface: Color(0xFF12151B),
  ),
  SessionTone.idle: ToneStyle(
    accent: Color(0xFF5B6478),
    onTint: Color(0xFF8891A3),
    tintedSurface: Color(0xFF12151B),
  ),
  SessionTone.stopped: ToneStyle(
    accent: Color(0xFF5B6478),
    onTint: Color(0xFF8891A3),
    tintedSurface: Color(0xFF12151B),
  ),
  SessionTone.creating: ToneStyle(
    accent: Color(0xFFA99DFF),
    onTint: Color(0xFFA99DFF),
    tintedSurface: Color(0xFF12151B),
  ),
  SessionTone.merging: ToneStyle(
    accent: Color(0xFFA99DFF),
    onTint: Color(0xFFA99DFF),
    tintedSurface: Color(0xFF12151B),
  ),
  SessionTone.pushing: ToneStyle(
    accent: Color(0xFF3FD6D0),
    onTint: Color(0xFF8891A3),
    tintedSurface: Color(0xFF12151B),
  ),
};

/// The LCARS theme: black canvas, amber/lilac/periwinkle/salmon, condensed
/// uppercase Antonio. Values transcribed from the design deck's turn-4 frames.
const lcarsTokens = CommanderTokens(
  terminalBg: Color(0xFF080808),
  canvas: Color(0xFF000000),
  canvasRaised: Color(0xFF0D0B08),
  surface: Color(0xFF120F14),
  surfaceSelected: Color(0xFF241809),
  // The rail's inert filler blocks, brightest to darkest.
  border: Color(0xFF5C4A6B),
  borderSubtle: Color(0xFF3A2F45),
  divider: Color(0xFF241D2B),
  text: Color(0xFFFFCC99),
  textBright: Color(0xFFC9C2B6),
  textMuted: Color(0xFF8A7A6A),
  textFaint: Color(0xFF7A6A5A),
  textDim: Color(0xFF5C4A3A),
  primary: Color(0xFFF7A01D),
  primarySoft: Color(0xFFFFCC99),
  // RUN blocks are amber in the deck, so `working` collapses onto `primary`
  // here — the inverse of Mission Control, where working is its own teal.
  working: Color(0xFFF7A01D),
  nav: Color(0xFFCC99CC),
  info: Color(0xFF9C9CFF),
  unread: Color(0xFF9C9CFF),
  attention: Color(0xFFCC6666),
  attentionOn: Color(0xFFFFCC99),
  held: Color(0xFFC98F4A),
  success: Color(0xFF8FBF8F),
  danger: Color(0xFFCC4444),
  idle: Color(0xFF5C4A6B),
  terminalFg: Color(0xFFC9C2B6),
  diffGutter: Color(0xFF4A423A),
  diffGutterBg: Color(0xFF0D0B08),
  // Deck P1 strokes the chevron mark in solid amber rather than a ramp.
  brandGradient: [Color(0xFFF7A01D), Color(0xFFF7A01D), Color(0xFFF7A01D)],
  sans: 'Antonio',
  mono: 'JetBrainsMono',
  uppercaseLabels: true,
  chrome: ChromeKind.lcars,
  cardRadius: 0,
  controlRadius: 0,
  pillRadius: 11,
  elbowRadius: 32,
  railWidth: 62,
  panelTopBorder: 2,
  tones: _lcarsTones,
);

/// LCARS' tone table. The row fills are the deck's hand-picked near-blacks, not
/// an alpha blend — no single formula reproduces `#1C1010` from salmon *and*
/// `#100E08` from amber.
const _lcarsTones = <SessionTone, ToneStyle>{
  SessionTone.working: ToneStyle(
    accent: Color(0xFFF7A01D),
    onTint: Color(0xFFFFCC99),
    tintedSurface: Color(0xFF100E08),
  ),
  SessionTone.waiting: ToneStyle(
    accent: Color(0xFFCC6666),
    onTint: Color(0xFFFFCC99),
    tintedSurface: Color(0xFF1C1010),
  ),
  SessionTone.held: ToneStyle(
    accent: Color(0xFFC98F4A),
    onTint: Color(0xFFFFCC99),
    tintedSurface: Color(0xFF100E08),
  ),
  SessionTone.unread: ToneStyle(
    accent: Color(0xFF9C9CFF),
    onTint: Color(0xFFFFCC99),
    tintedSurface: Color(0xFF0D0D14),
  ),
  SessionTone.idle: ToneStyle(
    accent: Color(0xFF5C4A6B),
    onTint: Color(0xFFC9B8A8),
    tintedSurface: Color(0xFF120F14),
  ),
  SessionTone.stopped: ToneStyle(
    accent: Color(0xFF5C4A6B),
    onTint: Color(0xFFC9B8A8),
    tintedSurface: Color(0xFF120F14),
  ),
  SessionTone.creating: ToneStyle(
    accent: Color(0xFF9C9CFF),
    onTint: Color(0xFFFFCC99),
    tintedSurface: Color(0xFF0D0D14),
  ),
  SessionTone.merging: ToneStyle(
    accent: Color(0xFFCC99CC),
    onTint: Color(0xFFFFCC99),
    tintedSurface: Color(0xFF120F14),
  ),
  SessionTone.pushing: ToneStyle(
    accent: Color(0xFFCC99CC),
    onTint: Color(0xFFFFCC99),
    tintedSurface: Color(0xFF120F14),
  ),
};
