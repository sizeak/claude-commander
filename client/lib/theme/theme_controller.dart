import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../services/pref_store.dart';
import 'theme_prefs.dart';
import 'tokens.dart';

/// The themes a user can pick.
enum ThemeId {
  /// The default: dark, violet/teal, Space Grotesk.
  missionControl('mission_control', 'Mission Control', missionControlTokens),

  /// Opt-in: black, amber/lilac, condensed Antonio, elbow chrome.
  lcars('lcars', 'LCARS', lcarsTokens);

  /// The persisted spelling. Stable and decoupled from the Dart name — renaming
  /// the enum constant must not silently reset every user's theme, the same rule
  /// `#[serde(alias)]` enforces on the Rust side (see CLAUDE.md § Migrations).
  final String wire;

  /// Shown in the picker.
  final String label;

  final CommanderTokens tokens;

  const ThemeId(this.wire, this.label, this.tokens);

  /// Parses a persisted [wire] value, falling back to [missionControl] for
  /// anything absent or unrecognised — a preferences file written by a newer
  /// build naming a theme this one lacks must not crash the app on launch.
  static ThemeId fromWire(String? wire) =>
      values.firstWhere((id) => id.wire == wire, orElse: () => missionControl);

  /// Parses a persisted [wire] value, or null for anything unrecognised — for a
  /// workspace's theme, where "absent" means "inherit the usual theme" rather
  /// than Mission Control.
  static ThemeId? tryWire(String? wire) =>
      values.where((id) => id.wire == wire).firstOrNull;
}

/// Owns the device's theme preferences — the usual theme and each workspace's
/// — persists them, and resolves the one the app renders in.
///
/// [load] must complete **before** `runApp`, because the deck requires the very
/// first screen — connect, on a cold start with no servers — to already be
/// themed. Restoring the theme a frame late would flash Mission Control.
///
/// **Per-workspace themes are per device.** They live in [workspacesPrefKey],
/// keyed by workspace name (Main under [mainWorkspaceThemeKey]); none of it goes
/// to a server. So a rename made on this device moves the key
/// ([renameWorkspace]), but a rename made elsewhere — another device, the TUI —
/// leaves the old key orphaned and the renamed workspace on the usual theme
/// until it is themed again here. Accepted: the alternative is putting device
/// cosmetics on the wire.
///
/// The app renders [tokens]: the theme resolved (by [resolveTheme]) for
/// [activeWorkspaceKey], which `CommanderApp` keeps in step with the fleet's
/// active workspace. Listeners hear about every preference change, but a
/// [setActiveWorkspace] that resolves to the same theme is silent, so the
/// fleet's frequent notifications never restart the theme crossfade.
class ThemeController extends ChangeNotifier {
  /// The usual theme: a bare [ThemeId.wire], or JSON once it carries overrides
  /// (see [encodeUsualTheme]).
  static const prefKey = 'commander.theme';

  /// Every workspace's theme, one JSON object ([encodeWorkspaceThemes]).
  static const workspacesPrefKey = 'commander.theme.workspaces';

  final PrefStore _store;
  ThemePref _usual;
  Map<String, ThemePref> _workspaces = const {};
  String? _activeKey;
  // Resolved eagerly, not `late`: [_refresh] compares against the previous
  // value, and a lazy one would first be computed *after* the change it is meant
  // to detect.
  ResolvedTheme _resolved;
  CommanderTokens _tokens;

  ThemeController({required PrefStore store, ThemeId? initial})
    : this._(store, ThemePref(themeId: initial ?? ThemeId.missionControl));

  ThemeController._(this._store, this._usual)
    : _resolved = resolveTheme(usual: _usual, workspace: null),
      _tokens = resolveTheme(usual: _usual, workspace: null).tokens;

  /// The preset the app is rendering in.
  ThemeId get id => _resolved.id;

  /// The tokens the app renders with. The same object until the resolved theme
  /// changes.
  CommanderTokens get tokens => _tokens;

  /// The usual theme, which every workspace without its own inherits.
  ThemePref get usual => _usual;

  /// The theme stored for workspace [key] ([workspaceThemeKey]), or null when
  /// it inherits the usual theme outright.
  ThemePref? workspaceTheme(String key) => _workspaces[key];

  /// The workspace whose theme the app renders in, or null for the usual theme.
  String? get activeWorkspaceKey => _activeKey;

  /// The theme workspace [key] resolves to (null = the usual theme).
  ResolvedTheme resolvedFor(String? key) => _resolve(key);

  /// [resolvedFor]'s tokens — the active workspace's are [tokens] itself.
  CommanderTokens tokensFor(String? key) {
    final r = _resolve(key);
    return r == _resolved ? _tokens : r.tokens;
  }

  ResolvedTheme _resolve(String? key) => resolveTheme(
    usual: _usual,
    workspace: key == null ? null : _workspaces[key],
  );

  /// Re-resolves the active theme, replacing [tokens] only when it changed.
  /// Returns whether it did.
  bool _refresh() {
    final next = _resolve(_activeKey);
    if (next == _resolved) return false;
    _resolved = next;
    _tokens = next.tokens;
    return true;
  }

  /// Reads the stored choices. Safe to call once at startup; a store failure
  /// leaves the defaults in place rather than blocking launch.
  Future<void> load() async {
    // Caught, not propagated: `main()` awaits this before `runApp`, so a throwing
    // store — a corrupt preferences file on the Linux backend, say — would turn a
    // cosmetic preference into a failure to launch. The default is a fine answer.
    String? stored;
    String? workspaces;
    try {
      stored = await _store.read(prefKey);
      workspaces = await _store.read(workspacesPrefKey);
    } catch (_) {
      return;
    }
    _usual = decodeUsualTheme(stored);
    _workspaces = decodeWorkspaceThemes(workspaces);
    if (_refresh()) notifyListeners();
  }

  /// Render workspace [key] ([workspaceThemeKey]; null = the usual theme).
  /// Notifies only when that changes what the app looks like.
  void setActiveWorkspace(String? key) {
    if (key == _activeKey) return;
    _activeKey = key;
    if (_refresh()) notifyListeners();
  }

  /// Selects [id] as the usual theme, notifying listeners immediately and
  /// persisting in the background — the deck promises switching is instant, so
  /// the repaint must not wait on a disk write. Its overrides are kept.
  Future<void> select(ThemeId id) => selectFor(null, id);

  /// Selects [id] for workspace [key], or for the usual theme when [key] is
  /// null. A workspace given a preset stops inheriting the usual overrides.
  Future<void> selectFor(String? key, ThemeId id) {
    if (key == null) {
      if (_usual.themeId == id) return Future.value();
      return _setUsual(_usual.withThemeId(id));
    }
    final current = _workspaces[key] ?? const ThemePref();
    if (current.themeId == id) return Future.value();
    return _setWorkspace(key, current.withThemeId(id));
  }

  /// Overrides [role] with [color] in workspace [key]'s theme (null = the usual
  /// theme), or clears the override when [color] is null.
  Future<void> setOverride(String? key, ThemeRole role, Color? color) {
    if (key == null) return _setUsual(_usual.withOverride(role, color));
    final current = _workspaces[key] ?? const ThemePref();
    return _setWorkspace(key, current.withOverride(role, color));
  }

  /// Drops workspace [key]'s theme, so it follows the usual theme again.
  Future<void> resetWorkspace(String key) {
    if (!_workspaces.containsKey(key)) return Future.value();
    return _setWorkspaces({..._workspaces}..remove(key));
  }

  /// A workspace was renamed on this device: its theme moves with it (and the
  /// active key too, so the app does not flash the usual theme while the fleet
  /// catches up). Main is never renamed by name — its key is fixed.
  Future<void> renameWorkspace(String from, String to) {
    if (from == to || from == mainWorkspaceThemeKey) return Future.value();
    if (_activeKey == from) _activeKey = to;
    final pref = _workspaces[from];
    if (pref == null) return Future.value();
    final next = {..._workspaces}
      ..remove(from)
      ..[to] = pref;
    return _setWorkspaces(next);
  }

  /// A workspace was deleted: its theme goes too. Main cannot be deleted, so
  /// its key is left alone.
  Future<void> forgetWorkspace(String name) =>
      name == mainWorkspaceThemeKey ? Future.value() : resetWorkspace(name);

  Future<void> _setUsual(ThemePref pref) async {
    if (pref == _usual) return;
    _usual = pref;
    _refresh();
    notifyListeners();
    // Same reasoning as [load]: the theme has already been applied, so a failed
    // write costs persistence across relaunch, not this session.
    try {
      await _store.write(prefKey, encodeUsualTheme(pref));
    } catch (_) {}
  }

  Future<void> _setWorkspace(String key, ThemePref pref) => _setWorkspaces(
    pref.isEmpty
        ? ({..._workspaces}..remove(key))
        : {..._workspaces, key: pref},
  );

  Future<void> _setWorkspaces(Map<String, ThemePref> next) async {
    if (mapEquals(next, _workspaces)) return;
    _workspaces = Map.unmodifiable(next);
    _refresh();
    notifyListeners();
    try {
      await _store.write(workspacesPrefKey, encodeWorkspaceThemes(next));
    } catch (_) {}
  }
}

/// Exposes the [ThemeController] to the widget tree, placed above the
/// `MaterialApp` so pushed routes (the Settings screen and its theme picker) can
/// reach it. Reading the *tokens* does not go through here — that is
/// `CommanderTokens.of(context)`, resolved from the theme extension. This scope
/// is only for the code that needs to *change* the theme.
class ThemeScope extends InheritedWidget {
  final ThemeController? controller;

  const ThemeScope({super.key, required this.controller, required super.child});

  static ThemeController? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<ThemeScope>()?.controller;

  @override
  bool updateShouldNotify(ThemeScope oldWidget) =>
      controller != oldWidget.controller;
}
