import 'dart:async';

import 'package:flutter/widgets.dart';

import '../services/pref_store.dart';
import '../services/window_service.dart';

/// Which frame the window wears.
enum TitleBarMode {
  /// The runner's GTK title bar.
  native('native', 'Native'),

  /// No native title bar; the active theme's own window bar instead.
  borderless('borderless', 'Hidden');

  /// The persisted spelling. Stable and decoupled from the Dart name — renaming
  /// the constant must not silently reset the frame, the same rule
  /// `#[serde(alias)]` enforces on the Rust side (see CLAUDE.md § Migrations).
  final String wire;

  /// Shown as the Settings row's caption.
  final String label;

  const TitleBarMode(this.wire, this.label);

  /// Parses a persisted [wire] value, falling back to [borderless].
  ///
  /// Borderless is the desktop default deliberately. GTK3 has no
  /// `xdg-decoration` support, so a GTK3 window on Wayland is *always*
  /// client-side decorated and KWin will never draw its own title bar for this
  /// app — "native" therefore means a GNOME-style header bar even in a KDE
  /// session. The app's own themed bar is the only frame that can match the
  /// desktop it is running on.
  static TitleBarMode fromWire(String? wire) =>
      values.firstWhere((m) => m.wire == wire, orElse: () => borderless);
}

/// Owns the desktop window's frame, fullscreen state and remembered geometry,
/// and persists all three to the device.
///
/// Exists only where [createWindowService] returned a service, so there is no
/// no-op path to reason about: no window, no controller (see [WindowScope]).
///
/// [load] must complete **before** `runApp`. The runner shows the window on its
/// first Flutter frame, so a frame restored late is a visible jump from the
/// default 1280x720 to wherever the user left the window.
class WindowController extends ChangeNotifier {
  static const titleBarPrefKey = 'commander.window.titlebar';
  static const fullscreenPrefKey = 'commander.window.fullscreen';
  static const boundsPrefKey = 'commander.window.bounds';

  /// The window's name in the taskbar and the alt-tab switcher, replacing the
  /// Flutter template's `claude_commander_client`.
  static const windowTitle = 'Claude Commander';

  /// How long to wait for a drag or resize to settle before reading the window's
  /// geometry back. Linux only emits the continuous `move`/`resize` events, so
  /// without this a single drag would mean hundreds of reads and writes.
  static const defaultGeometryDebounce = Duration(milliseconds: 400);

  final PrefStore _store;
  final WindowService _service;
  final Duration _geometryDebounce;

  TitleBarMode _titleBar = TitleBarMode.borderless;
  bool _fullscreen = false;
  bool _maximized = false;

  Timer? _geometryTimer;
  StreamSubscription<WindowEvent>? _events;

  WindowController({
    required PrefStore store,
    required WindowService service,
    Duration geometryDebounce = defaultGeometryDebounce,
  }) : _store = store,
       _service = service,
       _geometryDebounce = geometryDebounce;

  TitleBarMode get titleBar => _titleBar;
  bool get fullscreen => _fullscreen;
  bool get maximized => _maximized;

  /// Whether the app should draw its own window bar.
  ///
  /// Not simply "is borderless": GTK drops the title bar in fullscreen anyway, so
  /// a bar there would be chrome that cannot be dragged and a strip of screen
  /// given up for nothing.
  bool get showWindowBar =>
      _titleBar == TitleBarMode.borderless && !_fullscreen;

  /// Reads the stored preferences and applies them to the window. Call once,
  /// before `runApp`.
  Future<void> load() async {
    await _guard(_service.initialize);

    _titleBar = TitleBarMode.fromWire(await _read(titleBarPrefKey));
    _fullscreen = await _read(fullscreenPrefKey) == 'true';
    final bounds = WindowBounds.fromWire(await _read(boundsPrefKey));

    await _guard(() async {
      await _service.setTitle(windowTitle);
      // Geometry first: GTK remembers the pre-fullscreen size, so setting it
      // after would leave the un-fullscreened window at the runner's default.
      if (bounds != null) await _service.setBounds(bounds);
      await _service.setTitleBarHidden(_titleBar == TitleBarMode.borderless);
      if (_fullscreen) await _service.setFullScreen(true);
    });

    // Seeded from the window rather than assumed false: the desktop can restore
    // a session's windows maximized, and a bar that opened with the wrong glyph
    // would stay wrong until the user happened to maximize it again.
    _maximized = await _guard(_service.isMaximized) ?? false;

    _events = _service.events.listen(_onWindowEvent);
  }

  Future<void> setTitleBar(TitleBarMode mode) async {
    if (mode == _titleBar) return;
    _titleBar = mode;
    notifyListeners();
    await _guard(
      () => _service.setTitleBarHidden(mode == TitleBarMode.borderless),
    );
    await _write(titleBarPrefKey, mode.wire);
  }

  Future<void> setFullscreen(bool value) async {
    if (value == _fullscreen) return;
    _fullscreen = value;
    notifyListeners();
    await _guard(() => _service.setFullScreen(value));
    await _write(fullscreenPrefKey, '$value');
  }

  Future<void> toggleFullscreen() => setFullscreen(!_fullscreen);

  Future<void> toggleTitleBar() => setTitleBar(
    _titleBar == TitleBarMode.borderless
        ? TitleBarMode.native
        : TitleBarMode.borderless,
  );

  // ── Window-bar actions ─────────────────────────────────────────────────────

  Future<void> startDragging() => _guard(_service.startDragging);

  Future<void> minimize() => _guard(_service.minimize);

  Future<void> toggleMaximize() async {
    final target = !_maximized;
    // Set optimistically rather than waiting for the maximize/unmaximize event,
    // so the bar's glyph flips with the click. The event confirms it.
    _maximized = target;
    notifyListeners();
    try {
      await _service.setMaximized(target);
    } catch (_) {
      // The optimistic flip was a lie and no event will arrive to correct it —
      // the maximize/unmaximize feed only reports changes that *happened*. Ask
      // the window what is actually true rather than leaving the wrong glyph up.
      final actual = await _guard(_service.isMaximized);
      if (actual == null || actual == _maximized) return;
      _maximized = actual;
      notifyListeners();
    }
  }

  Future<void> close() => _guard(_service.close);

  // ── The desktop's own changes ──────────────────────────────────────────────

  void _onWindowEvent(WindowEvent event) {
    switch (event) {
      case WindowEvent.moved || WindowEvent.resized:
        _scheduleGeometrySave();
      case WindowEvent.maximized || WindowEvent.unmaximized:
        final maximized = event == WindowEvent.maximized;
        if (maximized == _maximized) return;
        _maximized = maximized;
        notifyListeners();
      case WindowEvent.enteredFullScreen || WindowEvent.leftFullScreen:
        final fullscreen = event == WindowEvent.enteredFullScreen;
        if (fullscreen == _fullscreen) return;
        _fullscreen = fullscreen;
        notifyListeners();
        // Persist it too: a fullscreen entered from the desktop's own shortcut is
        // still the state the user left the app in.
        _write(fullscreenPrefKey, '$fullscreen');
    }
  }

  void _scheduleGeometrySave() {
    _geometryTimer?.cancel();
    _geometryTimer = Timer(_geometryDebounce, _saveGeometry);
  }

  Future<void> _saveGeometry() async {
    // Neither of these is a placement worth restoring: fullscreen bounds are the
    // screen, and maximized bounds mean the next launch un-maximizes to screen
    // size with no floating geometry to return to.
    if (_fullscreen || _maximized) return;
    final bounds = await _guard(_service.getBounds);
    if (bounds == null) return;
    await _write(boundsPrefKey, bounds.wire);
  }

  // ── Failure containment ────────────────────────────────────────────────────
  // `main()` awaits load() before runApp, so neither a refusing window nor an
  // unreadable preferences backend may turn a cosmetic preference into a failure
  // to launch. Both are caught, in both directions.

  Future<T?> _guard<T>(Future<T> Function() action) async {
    try {
      return await action();
    } catch (_) {
      return null;
    }
  }

  Future<String?> _read(String key) => _guard<String?>(() => _store.read(key));

  Future<void> _write(String key, String value) =>
      _guard(() => _store.write(key, value));

  @override
  void dispose() {
    _geometryTimer?.cancel();
    _events?.cancel();
    super.dispose();
  }
}

/// Exposes the [WindowController] to the widget tree, above the `MaterialApp` so
/// both the window frame and the Settings screen can reach it.
///
/// A **null** controller is the honest answer on Android and anywhere else with
/// no window to manage: `WindowScope.of(context) == null` is what hides the
/// window bar and the Settings section, rather than a platform check repeated at
/// each site.
class WindowScope extends InheritedWidget {
  final WindowController? controller;

  const WindowScope({
    super.key,
    required this.controller,
    required super.child,
  });

  static WindowController? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<WindowScope>()?.controller;

  @override
  bool updateShouldNotify(WindowScope oldWidget) =>
      controller != oldWidget.controller;
}
