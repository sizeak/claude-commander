import 'package:flutter/painting.dart';

import '../theme/tokens.dart';

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

/// One colour the Workspaces page offers: a theme token's role [name] and its
/// `#rrggbb` [hex].
typedef WorkspaceSwatch = ({String name, String hex});

/// The accents the Workspaces page offers: the semantic roles of the active
/// theme [t], in a fixed order, with any colour a theme reuses for several
/// roles listed once under its first name (Mission Control's `nav` is its
/// `primary`, LCARS's `working` is its `primary`). The server accepts any
/// `#rrggbb`; these are just the menu, and they follow the theme so a picked
/// accent sits in the palette the rest of the app is drawn in.
List<WorkspaceSwatch> themeSwatches(CommanderTokens t) {
  final roles = <(String, Color)>[
    ('Primary', t.primary),
    ('Primary soft', t.primarySoft),
    ('Working', t.working),
    ('Nav', t.nav),
    ('Info', t.info),
    ('Unread', t.unread),
    ('Attention', t.attention),
    ('Held', t.held),
    ('Success', t.success),
    ('Danger', t.danger),
    ('Idle', t.idle),
  ];
  final seen = <String>{};
  return [
    for (final (name, color) in roles)
      if (seen.add(workspaceColorHex(color)))
        (name: name, hex: workspaceColorHex(color)),
  ];
}

/// What the user typed or pasted into the colour field, in the shape the wire
/// rule (`protocol::workspace::validate_workspace_color`) checks: trimmed,
/// `#`-prefixed when the `#` was left off, lowercase. Only the shape is
/// adjusted here; whether it is a colour is still the rule's call.
String normalizeWorkspaceColorInput(String raw) {
  final c = raw.trim().toLowerCase();
  return c.isEmpty || c.startsWith('#') ? c : '#$c';
}
