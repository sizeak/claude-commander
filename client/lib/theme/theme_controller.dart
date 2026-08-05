import 'package:flutter/widgets.dart';

import '../services/pref_store.dart';
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
}

/// Owns the selected theme and persists it to the device.
///
/// [load] must complete **before** `runApp`, because the deck requires the very
/// first screen — connect, on a cold start with no servers — to already be
/// themed. Restoring the theme a frame late would flash Mission Control.
class ThemeController extends ChangeNotifier {
  static const prefKey = 'commander.theme';

  final PrefStore _store;
  ThemeId _id;

  ThemeController({required PrefStore store, ThemeId? initial})
    : _store = store,
      _id = initial ?? ThemeId.missionControl;

  ThemeId get id => _id;

  CommanderTokens get tokens => _id.tokens;

  /// Reads the stored choice. Safe to call once at startup; a store failure
  /// leaves the default in place rather than blocking launch.
  Future<void> load() async {
    // Caught, not propagated: `main()` awaits this before `runApp`, so a throwing
    // store — a corrupt preferences file on the Linux backend, say — would turn a
    // cosmetic preference into a failure to launch. The default is a fine answer.
    String? stored;
    try {
      stored = await _store.read(prefKey);
    } catch (_) {
      return;
    }
    final resolved = ThemeId.fromWire(stored);
    if (resolved == _id) return;
    _id = resolved;
    notifyListeners();
  }

  /// Selects [id], notifying listeners immediately and persisting in the
  /// background — the deck promises switching is instant, so the repaint must
  /// not wait on a disk write.
  Future<void> select(ThemeId id) async {
    if (id == _id) return;
    _id = id;
    notifyListeners();
    // Same reasoning: the theme has already been applied, so a failed write costs
    // persistence across relaunch, not this session.
    try {
      await _store.write(prefKey, id.wire);
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
