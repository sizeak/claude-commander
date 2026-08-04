import 'dart:io';
import 'dart:ui';

import 'package:claude_commander_client/theme/brand_assets.dart';
import 'package:flutter_test/flutter_test.dart';

/// The launcher art under `tool/icon/` is hand-authored SVG, so nothing links
/// it to [BrandAssets] — a palette retune silently leaves the Android icon on
/// the old colours (it did, once). These tests pin the SVG masters to those
/// values so the home-screen icon stays the mark the app was designed around.
///
/// The launcher art is deliberately **not** themed: it is rendered to a static
/// Android resource at build time, so it cannot follow a runtime theme choice.
/// The in-app `BrandMark` is themed separately via `CommanderTokens`.
///
/// After changing a colour here, re-render the PNGs and regenerate the Android
/// resources (see `tool/icon/README.md`).
void main() {
  String read(String name) => File('tool/icon/$name').readAsStringSync();

  group('launcher icon art', () {
    test('the full icon uses the brand tile and gradient', () {
      final svg = read('icon_full.svg');
      for (final color in [BrandAssets.tileTop, BrandAssets.tileBottom]) {
        expect(svg, contains(_hex(color)), reason: 'tile gradient stop');
      }
      for (final color in BrandAssets.chevronGradient) {
        expect(svg, contains(_hex(color)), reason: 'chevron gradient stop');
      }
    });

    test('the adaptive background is the brand tile gradient', () {
      final svg = read('icon_background.svg');
      for (final color in [BrandAssets.tileTop, BrandAssets.tileBottom]) {
        expect(svg, contains(_hex(color)));
      }
    });

    test('the adaptive foreground chevrons use the brand gradient', () {
      final svg = read('icon_foreground.svg');
      for (final color in BrandAssets.chevronGradient) {
        expect(svg, contains(_hex(color)));
      }
    });

    test('every layer draws the same three chevrons', () {
      const chevrons = [
        'M 300 470 L 512 288 L 724 470',
        'M 300 620 L 512 438 L 724 620',
        'M 300 770 L 512 588 L 724 770',
      ];
      for (final name in ['icon_full.svg', 'icon_foreground.svg']) {
        final svg = read(name);
        for (final path in chevrons) {
          expect(svg, contains(path), reason: '$name is missing $path');
        }
      }
    });
  });
}

/// `#RRGGBB` as written in the SVG masters.
String _hex(Color color) =>
    '#${(color.toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0').toUpperCase()}';
