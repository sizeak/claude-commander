import 'package:flutter/painting.dart';

/// A workspace accent (`#rrggbb`, as `protocol::workspace` validates it) as a
/// [Color], or null when unset — or malformed, which a server that predates
/// the validation could still hold. Null means "no accent", never an error.
Color? parseWorkspaceColor(String? hex) {
  if (hex == null) return null;
  final m = RegExp(r'^#([0-9a-fA-F]{6})$').firstMatch(hex.trim());
  if (m == null) return null;
  return Color(0xFF000000 | int.parse(m.group(1)!, radix: 16));
}

/// [color] as the `#rrggbb` string a workspace definition stores.
String workspaceColorHex(Color color) {
  final rgb = color.toARGB32() & 0xFFFFFF;
  return '#${rgb.toRadixString(16).padLeft(6, '0')}';
}

/// The accents the Workspaces page offers. Picked to read on both themes'
/// dark canvases; the server accepts any `#rrggbb`, these are just the menu.
const workspacePalette = <String>[
  '#f7a01d',
  '#e4572e',
  '#cc99cc',
  '#6f9fd8',
  '#4fb286',
  '#e0c341',
  '#9a8cff',
  '#8a8f98',
];
