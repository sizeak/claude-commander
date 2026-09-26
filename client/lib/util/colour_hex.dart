import 'package:flutter/painting.dart';

/// A `#rrggbb` string as an opaque [Color], or null when unset or malformed.
/// Null means "no colour", never an error — a preference written by a newer or
/// corrupt build must not throw while the theme loads.
Color? parseHexColor(String? hex) {
  if (hex == null) return null;
  final m = RegExp(r'^#([0-9a-fA-F]{6})$').firstMatch(hex.trim());
  if (m == null) return null;
  return Color(0xFF000000 | int.parse(m.group(1)!, radix: 16));
}

/// [color] as a lower-case `#rrggbb` string (alpha dropped).
String hexOf(Color color) {
  final rgb = color.toARGB32() & 0xFFFFFF;
  return '#${rgb.toRadixString(16).padLeft(6, '0')}';
}

/// What the user typed or pasted into a colour field, trimmed, `#`-prefixed
/// when the `#` was left off, and lowercase. Only the shape is adjusted here;
/// whether it is a colour is [hexColorError]'s call.
String normalizeHexInput(String raw) {
  final c = raw.trim().toLowerCase();
  return c.isEmpty || c.startsWith('#') ? c : '#$c';
}

/// Why [hex] (already [normalizeHexInput]-ed) is not a colour, or null when it
/// is one.
String? hexColorError(String hex) =>
    parseHexColor(hex) == null ? '"$hex" is not a #rrggbb colour' : null;
