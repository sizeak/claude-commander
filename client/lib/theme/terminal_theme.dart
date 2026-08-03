import 'package:xterm/xterm.dart';

import 'tokens.dart';

/// The xterm widget's palette, derived from [CommanderTokens].
///
/// This is a **second** theme object that `ThemeData` cannot reach: `xterm`
/// takes its own [TerminalTheme] rather than reading the Material theme. It
/// used to be a top-level `const _terminalTheme` in `terminal_page.dart` built
/// from the old static palette, which meant a themed app would still have shown
/// a violet cursor on an amber LCARS screen.
///
/// The ANSI ramp is nudged toward the theme's semantic roles so agent output
/// reads in the same colour language as the surrounding chrome. Only the
/// *colours* are themed — the face stays [CommanderTokens.mono] in every theme,
/// because this renders real terminal output and must stay monospace.
///
/// **The result is memoised, and that is load-bearing rather than an
/// optimisation.** `xterm`'s render object does
///
/// ```dart
/// set theme(TerminalTheme value) {
///   if (value == _theme) return;
///   _colorPalette = PaletteBuilder(value).build();  // 256 Colors
///   _paragraphCache.clear();                        // every shaped glyph
/// }
/// ```
///
/// and `TerminalTheme` overrides neither `==` nor `hashCode`, so that guard is
/// an *identity* check. Returning a fresh instance per `build` would therefore
/// rebuild the palette and discard the whole text-shaping cache on every
/// rebuild — and the terminal's status bar ticks its throughput readout roughly
/// once a second, so the live terminal would re-shape every visible line at
/// 1 Hz. The old code avoided this by accident, being a top-level `const`.
///
/// A single-entry memo rather than a map: during a theme switch `AnimatedTheme`
/// interpolates fresh [CommanderTokens] every frame, and a growing cache keyed
/// on those would leak one `TerminalTheme` per frame forever. Missing the memo
/// mid-transition is correct anyway — the terminal genuinely is recolouring.
CommanderTokens? _memoKey;
TerminalTheme? _memoValue;

TerminalTheme terminalThemeFor(CommanderTokens t) {
  final cached = _memoValue;
  if (cached != null && identical(_memoKey, t)) return cached;
  final built = _buildTerminalTheme(t);
  _memoKey = t;
  _memoValue = built;
  return built;
}

TerminalTheme _buildTerminalTheme(CommanderTokens t) => TerminalTheme(
  cursor: t.primary,
  selection: t.primary.withValues(alpha: 0.25),
  foreground: t.terminalFg,
  background: t.terminalBg,
  // Darkest ANSI slot — not pure black, so a "black" glyph stays visible.
  black: t.borderSubtle,
  red: t.danger,
  green: t.success,
  yellow: t.attention,
  blue: t.info,
  magenta: t.primary,
  cyan: t.working,
  white: t.textBright,
  brightBlack: t.textDim,
  brightRed: t.danger,
  brightGreen: t.success,
  brightYellow: t.attentionOn,
  brightBlue: t.info,
  brightMagenta: t.info,
  brightCyan: t.working,
  brightWhite: t.text,
  searchHitBackground: t.attention.withValues(alpha: 0.4),
  searchHitBackgroundCurrent: t.attention,
  searchHitForeground: t.canvas,
);
