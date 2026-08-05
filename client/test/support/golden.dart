import 'dart:io';

import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// Shared harness for the golden tests.
///
/// Goldens here guard the **chrome layer** — the two themes' structure, colour
/// and spacing, which no behavioural test asserts. They are not a general safety
/// net: they render at `devicePixelRatio` 1 in logical pixels, so an artifact
/// that only exists in a rasterised window at fractional DPI (see the LCARS
/// eyebrow's padding comment) renders identically here and passes. Anything a
/// widget test can assert *behaviourally* still should be.
///
/// Regenerate after an intended visual change:
///
/// ```sh
/// cd client && flutter test test/goldens --update-goldens
/// ```
///
/// and read the image diff before committing — an unexplained change is the
/// point of these tests.

/// The bundled faces, keyed by the family name `pubspec.yaml` declares.
const _fonts = {
  'SpaceGrotesk': 'assets/fonts/SpaceGrotesk-VF.ttf',
  'JetBrainsMono': 'assets/fonts/JetBrainsMono-VF.ttf',
  'Antonio': 'assets/fonts/Antonio-VF.ttf',
};

var _fontsLoaded = false;

/// Loads the app's real fonts into the test binding.
///
/// **Mandatory before any golden.** `flutter test` does not load a package's
/// bundled fonts: without this every glyph falls back to Ahem's filled boxes, so
/// the reference images pin a layout the app never renders and any real type
/// regression sails through.
Future<void> loadCommanderFonts() async {
  if (_fontsLoaded) return;
  for (final MapEntry(key: family, value: path) in _fonts.entries) {
    final bytes = File(path).readAsBytesSync();
    await (FontLoader(
      family,
    )..addFont(Future.value(ByteData.sublistView(bytes)))).load();
  }
  await _loadMaterialIcons();
  _fontsLoaded = true;
}

/// Loads `MaterialIcons` from the SDK, so an `Icon` is its glyph rather than a
/// notdef box.
///
/// Without it every icon renders as an identical square, which is worse than
/// cosmetic: the window bar's maximise and restore goldens came out *byte for
/// byte identical*, silently pinning nothing.
///
/// The SDK path is derived from `FLUTTER_ROOT`, which `flutter test` sets for the
/// test process. Absent (a bare `dart test`, say) this gives up rather than
/// throwing — the goldens will then differ, which is the honest signal.
Future<void> _loadMaterialIcons() async {
  final root = Platform.environment['FLUTTER_ROOT'];
  if (root == null) return;
  final file = File(
    '$root/bin/cache/artifacts/material_fonts/MaterialIcons-Regular.otf',
  );
  if (!file.existsSync()) return;
  await (FontLoader('MaterialIcons')
        ..addFont(Future.value(ByteData.sublistView(file.readAsBytesSync()))))
      .load();
}

/// Known gap in the reference images: the session state glyphs (`○ ● ◆ ◍ ⏸ ⑃ ⬆`,
/// see `theme/agent_glyphs.dart`) render as notdef boxes here, because **none of
/// the bundled faces contain them** — only JetBrains Mono has `●` and `◆`. The
/// app gets them from the platform's fallback chain, which is the one thing
/// bundling fonts was supposed to remove; a test binding has no such chain. So a
/// change to a glyph is not caught by these goldens, and the boxes in the images
/// are a standing reminder that the app's own assets cannot draw them.
///
/// The themes every golden is taken in, with the filename prefix each uses.
const goldenThemes = {
  'mission_control': missionControlTokens,
  'lcars': lcarsTokens,
};

/// Fixes the surface for one golden, restoring it afterwards.
///
/// `devicePixelRatio` 1 keeps a golden's pixels equal to its logical pixels, so
/// a reference image can be reasoned about in the same units as the widget code.
void useGoldenSurface(WidgetTester tester, Size size) {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
}

/// Frames [child] the way the app does: inside the theme's `ThemeData` and a
/// real `Scaffold`.
///
/// The `Scaffold` is not decoration. A `Text` with no `Material` ancestor picks
/// up `DefaultTextStyle.fallback`, which Flutter draws with a yellow double
/// underline — bake that into a reference image once and every later diff is
/// read against a lie.
Widget goldenFrame(CommanderTokens tokens, Widget child) => MaterialApp(
  debugShowCheckedModeBanner: false,
  theme: themeDataFor(tokens),
  home: Scaffold(backgroundColor: tokens.canvas, body: child),
);

/// Pumps [child] under [tokens] at [size] and settles it.
Future<void> pumpGolden(
  WidgetTester tester, {
  required CommanderTokens tokens,
  required Widget child,
  Size size = const Size(420, 200),
}) async {
  await loadCommanderFonts();
  useGoldenSurface(tester, size);
  await tester.pumpWidget(goldenFrame(tokens, child));
  await tester.pumpAndSettle();
}

/// Asserts the whole surface against `test/goldens/images/<name>.png`.
Future<void> expectGolden(WidgetTester tester, String name) => expectLater(
  find.byType(MaterialApp),
  matchesGoldenFile('images/$name.png'),
);
