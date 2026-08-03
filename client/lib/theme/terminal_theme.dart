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
TerminalTheme terminalThemeFor(CommanderTokens t) => TerminalTheme(
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
