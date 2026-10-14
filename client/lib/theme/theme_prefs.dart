import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/painting.dart';

import '../util/colour_hex.dart';
import 'theme_controller.dart';
import 'tokens.dart';

/// The token roles a theme preference may recolour — the semantic accents, not
/// surfaces or the text ramp, which are what make a preset legible and are left
/// to it. Applied through [CommanderTokens.copyWith].
///
/// [wire] is the persisted spelling, pinned by `theme_prefs_test.dart` for the
/// same reason [ThemeId.wire] is: renaming a constant must not drop a user's
/// overrides.
enum ThemeRole {
  primary('primary', 'Primary'),
  primarySoft('primarySoft', 'Primary soft'),
  working('working', 'Working'),
  nav('nav', 'Nav'),
  info('info', 'Info'),
  unread('unread', 'Unread'),
  attention('attention', 'Attention'),
  held('held', 'Held'),
  success('success', 'Success'),
  danger('danger', 'Danger'),
  idle('idle', 'Idle');

  final String wire;
  final String label;

  const ThemeRole(this.wire, this.label);

  /// This role's colour in [t].
  Color of(CommanderTokens t) => switch (this) {
    primary => t.primary,
    primarySoft => t.primarySoft,
    working => t.working,
    nav => t.nav,
    info => t.info,
    unread => t.unread,
    attention => t.attention,
    held => t.held,
    success => t.success,
    danger => t.danger,
    idle => t.idle,
  };

  static ThemeRole? tryWire(String wire) =>
      values.where((r) => r.wire == wire).firstOrNull;
}

/// The role each session tone's accent is drawn from, where a preset draws it
/// from one. [applyOverrides] repaints a tone only when the preset's accent for
/// it really *is* that role's colour, which is what keeps a theme-specific
/// mapping intact: LCARS paints `merging` in its nav lilac rather than its info
/// periwinkle, so an info override does not reach it.
const _toneRoles = <SessionTone, ThemeRole>{
  SessionTone.working: ThemeRole.working,
  SessionTone.pushing: ThemeRole.working,
  SessionTone.waiting: ThemeRole.attention,
  SessionTone.held: ThemeRole.held,
  SessionTone.unread: ThemeRole.unread,
  SessionTone.idle: ThemeRole.idle,
  SessionTone.stopped: ThemeRole.idle,
  SessionTone.creating: ThemeRole.info,
  SessionTone.merging: ThemeRole.info,
};

/// [base] with [overrides] applied. No overrides returns [base] itself, so an
/// un-customised theme stays the `const` preset (and identical across
/// rebuilds).
///
/// Only a tone's accent follows its role; its tinted row fill and on-tint text
/// stay the preset's, because LCARS' fills are hand-tuned near-blacks no
/// formula derives from an accent.
CommanderTokens applyOverrides(
  CommanderTokens base,
  Map<ThemeRole, Color> overrides,
) {
  if (overrides.isEmpty) return base;
  Color? o(ThemeRole r) => overrides[r];
  final tones = {
    for (final tone in SessionTone.values)
      tone: switch ((_toneRoles[tone], base.toneStyle(tone))) {
        (final role?, final style)
            when overrides.containsKey(role) && style.accent == role.of(base) =>
          ToneStyle(
            accent: overrides[role]!,
            onTint: style.onTint,
            tintedSurface: style.tintedSurface,
          ),
        (_, final style) => style,
      },
  };
  return base.copyWith(
    primary: o(ThemeRole.primary),
    primarySoft: o(ThemeRole.primarySoft),
    working: o(ThemeRole.working),
    nav: o(ThemeRole.nav),
    info: o(ThemeRole.info),
    unread: o(ThemeRole.unread),
    attention: o(ThemeRole.attention),
    held: o(ThemeRole.held),
    success: o(ThemeRole.success),
    danger: o(ThemeRole.danger),
    idle: o(ThemeRole.idle),
    tones: tones,
  );
}

/// One theme preference: an optional preset and per-role colour overrides.
///
/// Used for both the usual theme (`commander.theme`, where a null [themeId]
/// means Mission Control) and a workspace's (where a null [themeId] means
/// "inherit the usual theme"); see [resolveTheme].
@immutable
class ThemePref {
  final ThemeId? themeId;
  final Map<ThemeRole, Color> overrides;

  const ThemePref({this.themeId, this.overrides = const {}});

  bool get isEmpty => themeId == null && overrides.isEmpty;

  ThemePref withThemeId(ThemeId? id) =>
      ThemePref(themeId: id, overrides: overrides);

  /// This pref with [role] set to [color], or cleared when [color] is null.
  ThemePref withOverride(ThemeRole role, Color? color) {
    final next = Map.of(overrides);
    if (color == null) {
      next.remove(role);
    } else {
      next[role] = color;
    }
    return ThemePref(themeId: themeId, overrides: Map.unmodifiable(next));
  }

  /// `{"themeId": "lcars", "overrides": {"primary": "#rrggbb"}}`, with either
  /// half omitted when empty. Overrides are written in [ThemeRole] order so the
  /// stored text is stable.
  Map<String, Object?> toJson() => {
    if (themeId case final id?) 'themeId': id.wire,
    if (overrides.isNotEmpty)
      'overrides': {
        for (final r in ThemeRole.values)
          if (overrides[r] case final c?) r.wire: hexOf(c),
      },
  };

  /// Parses [json] leniently: an unknown theme is dropped (inherit), an unknown
  /// role or a malformed colour is skipped, anything not a map is empty. Never
  /// throws — the theme loads before the first frame.
  static ThemePref fromJson(Object? json) {
    if (json is! Map) return const ThemePref();
    final id = switch (json['themeId']) {
      final String wire => ThemeId.tryWire(wire),
      _ => null,
    };
    final overrides = <ThemeRole, Color>{};
    if (json['overrides'] case final Map raw) {
      for (final MapEntry(:key, :value) in raw.entries) {
        final role = key is String ? ThemeRole.tryWire(key) : null;
        final color = value is String ? parseHexColor(value) : null;
        if (role != null && color != null) overrides[role] = color;
      }
    }
    return ThemePref(themeId: id, overrides: Map.unmodifiable(overrides));
  }

  @override
  bool operator ==(Object other) =>
      other is ThemePref &&
      other.themeId == themeId &&
      mapEquals(other.overrides, overrides);

  @override
  int get hashCode => Object.hash(
    themeId,
    Object.hashAllUnordered([
      for (final e in overrides.entries) Object.hash(e.key, e.value),
    ]),
  );

  @override
  String toString() => 'ThemePref(${toJson()})';
}

/// What a workspace (or the usual theme) actually renders in: a preset and the
/// overrides that apply on top of it.
@immutable
class ResolvedTheme {
  final ThemeId id;
  final Map<ThemeRole, Color> overrides;

  const ResolvedTheme(this.id, this.overrides);

  CommanderTokens get tokens => applyOverrides(id.tokens, overrides);

  @override
  bool operator ==(Object other) =>
      other is ResolvedTheme &&
      other.id == id &&
      mapEquals(other.overrides, overrides);

  @override
  int get hashCode => ThemePref(themeId: id, overrides: overrides).hashCode;
}

/// The inheritance rules, shared with the TUI's `[workspace_themes]`:
///
/// * no [workspace] entry → the [usual] theme (preset + its overrides);
/// * an entry without a preset → the usual theme, then the entry's overrides
///   on top;
/// * an entry with a preset → that preset and **only** the entry's overrides —
///   the usual overrides were chosen against another palette and do not carry.
ResolvedTheme resolveTheme({
  required ThemePref usual,
  required ThemePref? workspace,
}) {
  final usualId = usual.themeId ?? ThemeId.missionControl;
  if (workspace == null) return ResolvedTheme(usualId, usual.overrides);
  if (workspace.themeId case final id?) {
    return ResolvedTheme(id, workspace.overrides);
  }
  return ResolvedTheme(usualId, {...usual.overrides, ...workspace.overrides});
}

/// Where a role's colour comes from, for the picker's per-role rows.
enum RoleSource {
  /// The preset's own value.
  preset,

  /// Inherited from an override on the usual theme.
  usual,

  /// Overridden in the scope being edited.
  overridden,
}

/// Where [role]'s colour comes from when editing [workspace] (null = editing
/// the usual theme itself).
RoleSource roleSource({
  required ThemePref usual,
  required ThemePref? workspace,
  required ThemeRole role,
}) {
  if (workspace == null) {
    return usual.overrides.containsKey(role)
        ? RoleSource.overridden
        : RoleSource.preset;
  }
  if (workspace.overrides.containsKey(role)) return RoleSource.overridden;
  if (workspace.themeId == null && usual.overrides.containsKey(role)) {
    return RoleSource.usual;
  }
  return RoleSource.preset;
}

/// The key Main's theme is stored under. Main has no name, and `main` is a
/// reserved workspace name on the wire (refused case-insensitively), so no
/// user workspace can collide with it. The same key the TUI uses for
/// `[workspace_themes.main]`.
const mainWorkspaceThemeKey = 'main';

/// The theme key for workspace [name] (null = Main).
String workspaceThemeKey(String? name) => name ?? mainWorkspaceThemeKey;

/// The usual theme as `commander.theme` stores it: the bare wire spelling when
/// there are no overrides — exactly what builds before overrides wrote, so one
/// of those still reads the choice — and the JSON form otherwise.
String encodeUsualTheme(ThemePref pref) {
  final id = pref.themeId ?? ThemeId.missionControl;
  if (pref.overrides.isEmpty) return id.wire;
  return jsonEncode(pref.withThemeId(id).toJson());
}

/// Reads [encodeUsualTheme]'s output, or a bare wire spelling. Anything
/// unreadable is Mission Control.
ThemePref decodeUsualTheme(String? stored) {
  if (stored != null && stored.trimLeft().startsWith('{')) {
    try {
      final pref = ThemePref.fromJson(jsonDecode(stored));
      return pref.withThemeId(pref.themeId ?? ThemeId.missionControl);
    } on FormatException {
      return const ThemePref(themeId: ThemeId.missionControl);
    }
  }
  return ThemePref(themeId: ThemeId.fromWire(stored));
}

/// Every workspace's theme, as one JSON object keyed by [workspaceThemeKey].
/// Empty entries are not written.
String encodeWorkspaceThemes(Map<String, ThemePref> prefs) => jsonEncode({
  for (final MapEntry(:key, :value) in prefs.entries)
    if (!value.isEmpty) key: value.toJson(),
});

/// Reads [encodeWorkspaceThemes]'s output leniently; entries that decode to
/// nothing are dropped.
Map<String, ThemePref> decodeWorkspaceThemes(String? stored) {
  if (stored == null) return {};
  Object? json;
  try {
    json = jsonDecode(stored);
  } on FormatException {
    return {};
  }
  if (json is! Map) return {};
  return {
    for (final MapEntry(:key, :value) in json.entries)
      if ((key, ThemePref.fromJson(value)) case (
        final String k,
        final p,
      ) when !p.isEmpty)
        k: p,
  };
}
