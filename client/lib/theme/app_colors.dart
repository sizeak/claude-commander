import 'package:flutter/painting.dart';

/// The dark, terminal-DNA colour palette for the Commander client, taken from
/// the "Commander Mobile" design deck. Every screen pulls its colours from here
/// (directly or via [ThemeData] built in `app_theme.dart`) so the palette stays
/// consistent and is the single place to retune.
///
/// The ramp runs from the near-black backgrounds through raised surfaces to the
/// text scale, then the semantic accents used for agent state, PR/CI, and diffs.
abstract final class AppColors {
  // ── Backgrounds ────────────────────────────────────────────────────────
  /// Deepest surface: the live terminal / deepest part of the workspace.
  static const bgTerminal = Color(0xFF07090C);

  /// The app scaffold and the Fleet rail.
  static const bg = Color(0xFF0A0C10);

  /// Slightly raised strips — bottom tab bar, status bars.
  static const bgRaised = Color(0xFF0C0E13);

  /// Cards, inputs, icon buttons — the primary elevated surface.
  static const surface = Color(0xFF12151B);

  /// The selected pill inside a segmented control.
  static const surfaceSel = Color(0xFF242938);

  // ── Borders / dividers ─────────────────────────────────────────────────
  /// Card and input borders.
  static const border = Color(0xFF232838);

  /// Structural separators — rail edge, section rules, footers.
  static const borderSubtle = Color(0xFF1C1F28);

  /// The faintest divider, between dense list rows.
  static const divider = Color(0xFF14171D);

  // ── Text ramp ──────────────────────────────────────────────────────────
  static const text = Color(0xFFE7EBF2);
  static const textBright = Color(0xFFC9CDD6);
  static const textMuted = Color(0xFF8891A3);
  static const textFaint = Color(0xFF6B7488);
  static const textDim = Color(0xFF556072);

  // ── Semantic accents ───────────────────────────────────────────────────
  /// Primary accent (violet): buttons, selection, brand.
  static const accent = Color(0xFF7C6CFF);

  /// Lighter accent: PR badge text, timeline nodes.
  static const accentSoft = Color(0xFFA99DFF);

  /// Working / connected / filenames / restart.
  static const teal = Color(0xFF3FD6D0);

  /// Success / CI pass / added diff lines / REVIEW badge.
  static const green = Color(0xFF5FD88A);

  /// Waiting / cascade-paused (fill + brighter text variant).
  static const amber = Color(0xFFF5B545);
  static const amberText = Color(0xFFF5C877);

  /// Error / removed diff lines / delete / unreachable.
  static const red = Color(0xFFFF8A9B);

  /// Idle / stopped glyph, disabled state.
  static const idle = Color(0xFF5B6478);

  // ── Brand mark gradient (top → bottom) ─────────────────────────────────
  static const brandTop = Color(0xFFB3A6FF);
  static const brandMid = Color(0xFF7C9DFF);
  static const brandBottom = Color(0xFF3FD6D0);
  static const brandGradient = [brandTop, brandMid, brandBottom];

  // ── Terminal / diff ────────────────────────────────────────────────────
  /// Foreground for terminal + pane-snapshot monospace text (the deck's
  /// `#c8ccd4`), a touch softer than [textBright].
  static const terminalFg = Color(0xFFC8CCD4);
  static const diffGutter = Color(0xFF454B57);
  static const diffGutterBg = Color(0xFF0C0E13);
}

/// Bundled font family names (declared in `pubspec.yaml`). Space Grotesk is the
/// UI face; JetBrains Mono is used for terminal text, code, badges, and all the
/// small mono metadata captions throughout the deck.
abstract final class AppFonts {
  static const sans = 'SpaceGrotesk';
  static const mono = 'JetBrainsMono';
}
