import 'package:flutter/material.dart';

import 'app_colors.dart';

/// Builds the app-wide dark theme from the [AppColors] tokens. Space Grotesk is
/// the default (sans) family; use [mono] for the JetBrains Mono metadata/code
/// captions that recur across every screen.
abstract final class AppTheme {
  /// The single dark theme. There is no light theme — the design is dark-only.
  static ThemeData dark() {
    const scheme = ColorScheme.dark(
      primary: AppColors.accent,
      onPrimary: AppColors.bg,
      primaryContainer: AppColors.surfaceSel,
      onPrimaryContainer: AppColors.text,
      secondary: AppColors.teal,
      onSecondary: AppColors.bg,
      tertiary: AppColors.accentSoft,
      onTertiary: AppColors.bg,
      surface: AppColors.bg,
      onSurface: AppColors.text,
      surfaceContainerLowest: AppColors.bgTerminal,
      surfaceContainerLow: AppColors.bgRaised,
      surfaceContainer: AppColors.surface,
      surfaceContainerHigh: AppColors.surface,
      surfaceContainerHighest: AppColors.surfaceSel,
      onSurfaceVariant: AppColors.textMuted,
      outline: AppColors.border,
      outlineVariant: AppColors.borderSubtle,
      error: AppColors.red,
      onError: AppColors.bg,
    );

    final base = ThemeData(
      useMaterial3: true,
      brightness: Brightness.dark,
      colorScheme: scheme,
      scaffoldBackgroundColor: AppColors.bg,
      canvasColor: AppColors.bg,
      fontFamily: AppFonts.sans,
      splashFactory: InkSparkle.splashFactory,
    );

    return base.copyWith(
      textTheme: _textTheme(base.textTheme),
      dividerTheme: const DividerThemeData(
        color: AppColors.borderSubtle,
        thickness: 1,
        space: 1,
      ),
      dividerColor: AppColors.borderSubtle,
      appBarTheme: const AppBarTheme(
        backgroundColor: AppColors.bg,
        foregroundColor: AppColors.text,
        elevation: 0,
        scrolledUnderElevation: 0,
        centerTitle: false,
        titleTextStyle: TextStyle(
          fontFamily: AppFonts.sans,
          fontSize: 18,
          fontWeight: FontWeight.w700,
          letterSpacing: -0.3,
          color: AppColors.text,
        ),
      ),
      cardTheme: const CardThemeData(
        color: AppColors.surface,
        elevation: 0,
        margin: EdgeInsets.zero,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.all(Radius.circular(13)),
          side: BorderSide(color: AppColors.border),
        ),
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: AppColors.surface,
        hintStyle: const TextStyle(color: AppColors.textFaint),
        labelStyle: const TextStyle(color: AppColors.textMuted),
        contentPadding: const EdgeInsets.symmetric(horizontal: 14, vertical: 13),
        border: _inputBorder(AppColors.border),
        enabledBorder: _inputBorder(AppColors.border),
        focusedBorder: _inputBorder(AppColors.accent),
        errorBorder: _inputBorder(AppColors.red),
        focusedErrorBorder: _inputBorder(AppColors.red),
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          backgroundColor: AppColors.accent,
          foregroundColor: AppColors.bg,
          disabledBackgroundColor: AppColors.surface,
          disabledForegroundColor: AppColors.textFaint,
          textStyle: const TextStyle(
            fontFamily: AppFonts.sans,
            fontWeight: FontWeight.w700,
            fontSize: 14,
          ),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
        ),
      ),
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          foregroundColor: AppColors.textBright,
          side: const BorderSide(color: AppColors.border),
          backgroundColor: AppColors.surface,
          textStyle: const TextStyle(
            fontFamily: AppFonts.sans,
            fontWeight: FontWeight.w600,
            fontSize: 14,
          ),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 13),
        ),
      ),
      textButtonTheme: TextButtonThemeData(
        style: TextButton.styleFrom(foregroundColor: AppColors.accentSoft),
      ),
      iconTheme: const IconThemeData(color: AppColors.textBright),
      listTileTheme: const ListTileThemeData(
        iconColor: AppColors.textMuted,
        textColor: AppColors.text,
      ),
      snackBarTheme: const SnackBarThemeData(
        backgroundColor: AppColors.surfaceSel,
        contentTextStyle: TextStyle(color: AppColors.text),
        behavior: SnackBarBehavior.floating,
      ),
      dialogTheme: const DialogThemeData(
        backgroundColor: AppColors.surface,
        surfaceTintColor: Colors.transparent,
      ),
      popupMenuTheme: const PopupMenuThemeData(
        color: AppColors.surface,
        surfaceTintColor: Colors.transparent,
      ),
      progressIndicatorTheme: const ProgressIndicatorThemeData(
        color: AppColors.accent,
      ),
      dropdownMenuTheme: const DropdownMenuThemeData(
        menuStyle: MenuStyle(
          backgroundColor: WidgetStatePropertyAll(AppColors.surface),
          surfaceTintColor: WidgetStatePropertyAll(Colors.transparent),
        ),
      ),
    );
  }

  static OutlineInputBorder _inputBorder(Color color) => OutlineInputBorder(
    borderRadius: BorderRadius.circular(12),
    borderSide: BorderSide(color: color),
  );

  static TextTheme _textTheme(TextTheme base) => base
      .apply(
        fontFamily: AppFonts.sans,
        bodyColor: AppColors.text,
        displayColor: AppColors.text,
      )
      .copyWith(
        titleLarge: const TextStyle(
          fontWeight: FontWeight.w700,
          letterSpacing: -0.3,
          color: AppColors.text,
        ),
        titleMedium: const TextStyle(
          fontWeight: FontWeight.w700,
          letterSpacing: -0.2,
          color: AppColors.text,
        ),
        titleSmall: const TextStyle(
          fontWeight: FontWeight.w600,
          color: AppColors.text,
        ),
        bodyMedium: const TextStyle(color: AppColors.textBright, height: 1.45),
        bodySmall: const TextStyle(color: AppColors.textMuted),
        labelLarge: const TextStyle(fontWeight: FontWeight.w600),
      );

  /// A JetBrains Mono text style — the deck's metadata / code / badge face.
  /// Defaults mirror the recurring "mono meta" caption (11px, muted).
  static TextStyle mono({
    double size = 11,
    FontWeight weight = FontWeight.w500,
    Color color = AppColors.textMuted,
    double? height,
    double? letterSpacing,
  }) => TextStyle(
    fontFamily: AppFonts.mono,
    fontSize: size,
    fontWeight: weight,
    color: color,
    height: height,
    letterSpacing: letterSpacing,
  );

  /// An uppercase mono section eyebrow, e.g. "FILES CHANGED", "THIS SESSION".
  static TextStyle eyebrow({Color color = AppColors.textFaint}) => mono(
    size: 9.5,
    weight: FontWeight.w600,
    color: color,
    letterSpacing: 1.4,
  );
}
