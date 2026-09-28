import 'package:flutter/painting.dart';

/// Colours baked into the **launcher art**, which is not themed.
///
/// Deliberately separate from [CommanderTokens]: the icon under `tool/icon/` is
/// hand-authored SVG rendered to PNG at build time and installed as an Android
/// resource, so it cannot respond to a runtime theme choice — a phone's home
/// screen shows one icon regardless of which theme is active in the app.
///
/// `test/theme/launcher_icon_test.dart` pins the SVG masters to these values, so
/// a palette retune can't silently leave the home-screen icon behind (it did,
/// once). The in-app `BrandMark` is a different thing and *is* themed — see
/// `CommanderTokens.brandGradient`.
///
/// After changing anything here, re-render the PNGs and regenerate the Android
/// resources; see `tool/icon/README.md`.
abstract final class BrandAssets {
  /// The chevron ramp, top → bottom.
  static const chevronTop = Color(0xFFB3A6FF);
  static const chevronMid = Color(0xFF7C9DFF);
  static const chevronBottom = Color(0xFF3FD6D0);
  static const chevronGradient = [chevronTop, chevronMid, chevronBottom];

  /// The slate tile the chevrons sit on (top-left → bottom-right).
  static const tileTop = Color(0xFF2B2D3A);
  static const tileBottom = Color(0xFF1B1C26);
}
