import 'package:flutter/material.dart';

import 'tokens.dart';

/// Builds the app-wide [ThemeData] from a token set, and installs [t] as a theme
/// extension so `CommanderTokens.of(context)` resolves anywhere below it.
///
/// This layer exists because chrome widgets alone cannot theme the app. Material
/// paints a long tail of surfaces from `ThemeData`'s component themes that no
/// page ever constructs by hand: dialogs, popup menus, snackbars, text-field
/// decoration and caret, `RefreshIndicator`, `CircularProgressIndicator`.
/// Without this, selecting LCARS would produce violet, rounded, Space-Grotesk
/// dialogs floating inside amber elbow chrome.
///
/// Note that shape comes from the tokens too, not only colour: Mission Control's
/// [CommanderTokens.cardRadius] is 13 where LCARS is 0, its panels being
/// hard-edged with a single coloured top border instead of an all-round one.
///
/// **This is a faithful port of the former `AppTheme.dark()`.** Feeding it
/// [missionControlTokens] must reproduce that theme exactly — Phase 1 of the
/// theming work is meant to be invisible, so no component theme is added here
/// that the old one lacked unless it provably resolves to the same Material
/// default (see [TextSelectionThemeData] below). Extra component themes that
/// LCARS wants — bottom sheets, dialog type scales — land with LCARS itself,
/// where their effect on Mission Control can be judged deliberately rather than
/// slipped in under a "no visible change" phase.
ThemeData themeDataFor(CommanderTokens t) {
  final scheme = ColorScheme.dark(
    primary: t.primary,
    onPrimary: t.canvas,
    primaryContainer: t.surfaceSelected,
    onPrimaryContainer: t.text,
    secondary: t.working,
    onSecondary: t.canvas,
    tertiary: t.primarySoft,
    onTertiary: t.canvas,
    surface: t.canvas,
    onSurface: t.text,
    surfaceContainerLowest: t.terminalBg,
    surfaceContainerLow: t.canvasRaised,
    surfaceContainer: t.surface,
    surfaceContainerHigh: t.surface,
    surfaceContainerHighest: t.surfaceSelected,
    onSurfaceVariant: t.textMuted,
    outline: t.border,
    outlineVariant: t.borderSubtle,
    error: t.danger,
    onError: t.canvas,
  );

  final base = ThemeData(
    useMaterial3: true,
    brightness: Brightness.dark,
    colorScheme: scheme,
    scaffoldBackgroundColor: t.canvas,
    canvasColor: t.canvas,
    fontFamily: t.sans,
    splashFactory: InkSparkle.splashFactory,
    // The face reaches dialogs, sheets and menus through here, so those need no
    // per-component text styles to pick up Antonio under LCARS.
    extensions: [t],
  );

  OutlineInputBorder inputBorder(Color color) => OutlineInputBorder(
    borderRadius: BorderRadius.circular(t.controlRadius),
    borderSide: BorderSide(color: color),
  );

  return base.copyWith(
    textTheme: _textTheme(base.textTheme, t),
    dividerTheme: DividerThemeData(
      color: t.borderSubtle,
      thickness: 1,
      space: 1,
    ),
    dividerColor: t.borderSubtle,
    appBarTheme: AppBarTheme(
      backgroundColor: t.canvas,
      foregroundColor: t.text,
      elevation: 0,
      scrolledUnderElevation: 0,
      centerTitle: false,
      titleTextStyle: TextStyle(
        fontFamily: t.sans,
        fontSize: 18,
        fontWeight: FontWeight.w700,
        letterSpacing: t.uppercaseLabels ? 0.7 : -0.3,
        color: t.text,
      ),
    ),
    cardTheme: CardThemeData(
      color: t.surface,
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(t.cardRadius),
        side: BorderSide(color: t.border),
      ),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: t.surface,
      hintStyle: TextStyle(color: t.textFaint),
      labelStyle: TextStyle(color: t.textMuted),
      contentPadding: const EdgeInsets.symmetric(horizontal: 14, vertical: 13),
      border: inputBorder(t.border),
      enabledBorder: inputBorder(t.border),
      focusedBorder: inputBorder(t.primary),
      errorBorder: inputBorder(t.danger),
      focusedErrorBorder: inputBorder(t.danger),
    ),
    // Not part of ColorScheme, and the old theme left it unset. The values below
    // are exactly Material 3's own defaults (`colorScheme.primary`, and primary
    // at 40% for the selection), so this is a no-op for Mission Control — it is
    // stated explicitly only so LCARS gets an amber caret rather than inheriting
    // a violet one from a stale ColorScheme assumption.
    textSelectionTheme: TextSelectionThemeData(
      cursorColor: t.primary,
      selectionColor: t.primary.withValues(alpha: 0.4),
      selectionHandleColor: t.primary,
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        backgroundColor: t.primary,
        foregroundColor: t.canvas,
        disabledBackgroundColor: t.surface,
        disabledForegroundColor: t.textFaint,
        textStyle: TextStyle(
          fontFamily: t.sans,
          fontWeight: FontWeight.w700,
          fontSize: 14,
        ),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(t.controlRadius),
        ),
        padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        foregroundColor: t.textBright,
        side: BorderSide(color: t.border),
        backgroundColor: t.surface,
        textStyle: TextStyle(
          fontFamily: t.sans,
          fontWeight: FontWeight.w600,
          fontSize: 14,
        ),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(t.controlRadius),
        ),
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 13),
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(foregroundColor: t.primarySoft),
    ),
    iconTheme: IconThemeData(color: t.textBright),
    listTileTheme: ListTileThemeData(iconColor: t.textMuted, textColor: t.text),
    snackBarTheme: SnackBarThemeData(
      backgroundColor: t.surfaceSelected,
      contentTextStyle: TextStyle(color: t.text),
      behavior: SnackBarBehavior.floating,
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: t.surface,
      surfaceTintColor: Colors.transparent,
    ),
    popupMenuTheme: PopupMenuThemeData(
      color: t.surface,
      surfaceTintColor: Colors.transparent,
    ),
    progressIndicatorTheme: ProgressIndicatorThemeData(color: t.primary),
    dropdownMenuTheme: DropdownMenuThemeData(
      menuStyle: MenuStyle(
        backgroundColor: WidgetStatePropertyAll(t.surface),
        surfaceTintColor: const WidgetStatePropertyAll(Colors.transparent),
      ),
    ),
  );
}

/// The text theme, preserving the former `AppTheme._textTheme` exactly for
/// Mission Control — including the per-level letter spacing (-0.3 on
/// `titleLarge`, -0.2 on `titleMedium`, none on `titleSmall`), which a single
/// shared display() helper would have flattened.
TextTheme _textTheme(TextTheme base, CommanderTokens t) {
  // LCARS' condensed face wants positive tracking where Space Grotesk is
  // tightened; flip the sign rather than carry two sets of literals.
  double track(double mc) => t.uppercaseLabels ? -mc * 2 : mc;
  return base
      .apply(fontFamily: t.sans, bodyColor: t.text, displayColor: t.text)
      .copyWith(
        titleLarge: TextStyle(
          fontWeight: FontWeight.w700,
          letterSpacing: track(-0.3),
          color: t.text,
        ),
        titleMedium: TextStyle(
          fontWeight: FontWeight.w700,
          letterSpacing: track(-0.2),
          color: t.text,
        ),
        titleSmall: TextStyle(fontWeight: FontWeight.w600, color: t.text),
        bodyMedium: TextStyle(color: t.textBright, height: 1.45),
        bodySmall: TextStyle(color: t.textMuted),
        labelLarge: const TextStyle(fontWeight: FontWeight.w600),
      );
}
