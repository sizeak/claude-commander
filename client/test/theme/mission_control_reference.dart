import 'dart:ui';

/// The **frozen** Mission Control palette, transcribed from `lib/theme/
/// app_colors.dart` as it stood before the theming work (commit `1ba8e7e`).
///
/// This exists so `tokens_test.dart` can assert that migrating ~357 static
/// `AppColors.` references onto [CommanderTokens] did not retune a single
/// colour. It is deliberately a **test fixture, not library code**: if it lived
/// in `lib/` and the token set were built from it, the parity test would be
/// tautological — it would compare a value to itself and pass no matter what.
///
/// Do not "fix" a value here to make a test pass. A mismatch means either the
/// migration changed a colour (revert it) or the change is intentional (then
/// change this file in the same commit, so the diff records the retune).
abstract final class McRef {
  // ── Backgrounds ────────────────────────────────────────────────────────
  static const bgTerminal = Color(0xFF07090C);
  static const bg = Color(0xFF0A0C10);
  static const bgRaised = Color(0xFF0C0E13);
  static const surface = Color(0xFF12151B);
  static const surfaceSel = Color(0xFF242938);

  // ── Borders / dividers ─────────────────────────────────────────────────
  static const border = Color(0xFF232838);
  static const borderSubtle = Color(0xFF1C1F28);
  static const divider = Color(0xFF14171D);

  // ── Text ramp ──────────────────────────────────────────────────────────
  static const text = Color(0xFFE7EBF2);
  static const textBright = Color(0xFFC9CDD6);
  static const textMuted = Color(0xFF8891A3);
  static const textFaint = Color(0xFF6B7488);
  static const textDim = Color(0xFF556072);

  // ── Semantic accents ───────────────────────────────────────────────────
  static const accent = Color(0xFF7C6CFF);
  static const accentSoft = Color(0xFFA99DFF);
  static const teal = Color(0xFF3FD6D0);
  static const green = Color(0xFF5FD88A);
  static const amber = Color(0xFFF5B545);
  static const amberText = Color(0xFFF5C877);
  static const red = Color(0xFFFF8A9B);
  static const idle = Color(0xFF5B6478);

  // ── Brand mark ─────────────────────────────────────────────────────────
  static const brandTop = Color(0xFFB3A6FF);
  static const brandMid = Color(0xFF7C9DFF);
  static const brandBottom = Color(0xFF3FD6D0);
  static const brandGradient = [brandTop, brandMid, brandBottom];
  static const brandTileTop = Color(0xFF2B2D3A);
  static const brandTileBottom = Color(0xFF1B1C26);

  // ── Terminal / diff ────────────────────────────────────────────────────
  static const terminalFg = Color(0xFFC8CCD4);
  static const diffGutter = Color(0xFF454B57);
  static const diffGutterBg = Color(0xFF0C0E13);

  // ── Fonts ──────────────────────────────────────────────────────────────
  static const sans = 'SpaceGrotesk';
  static const mono = 'JetBrainsMono';
}
