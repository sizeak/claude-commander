import 'dart:async';
import 'dart:ui' show Rect;

import 'package:flutter/foundation.dart';
import 'package:window_manager/window_manager.dart';

/// A change to the window that did **not** come from us.
///
/// The desktop drives the window too — dragging an edge, KWin's own fullscreen
/// and maximise shortcuts — so [WindowController] listens rather than assuming it
/// is the only writer. Without this the app-drawn window bar would be the one
/// part of the UI able to disagree with the actual window.
enum WindowEvent {
  moved,
  resized,
  maximized,
  unmaximized,
  enteredFullScreen,
  leftFullScreen,
}

/// The window's position and size in logical pixels.
///
/// Whole pixels: this is a remembered window placement, and a persisted
/// `1280.0000001` would be noise in a preferences file no one can hand-edit.
@immutable
class WindowBounds {
  final int x;
  final int y;
  final int width;
  final int height;

  const WindowBounds({
    required this.x,
    required this.y,
    required this.width,
    required this.height,
  });

  factory WindowBounds.fromRect(Rect rect) => WindowBounds(
    x: rect.left.round(),
    y: rect.top.round(),
    width: rect.width.round(),
    height: rect.height.round(),
  );

  Rect get rect => Rect.fromLTWH(
    x.toDouble(),
    y.toDouble(),
    width.toDouble(),
    height.toDouble(),
  );

  /// The persisted form, `"x,y,w,h"`.
  String get wire => '$x,$y,$width,$height';

  /// Parses [wire], returning null for anything absent, malformed or unusable.
  ///
  /// Null rather than a throw or a clamp: a truncated preferences file must cost
  /// the remembered geometry, not the launch. A zero or negative size is refused
  /// for a sharper reason — borderless has no titlebar to grab, so a window
  /// restored at 0x0 could not be recovered.
  static WindowBounds? fromWire(String? wire) {
    if (wire == null) return null;
    final parts = wire.split(',');
    if (parts.length != 4) return null;
    final values = [for (final p in parts) int.tryParse(p.trim())];
    if (values.any((v) => v == null)) return null;
    final [x!, y!, width!, height!] = values;
    if (width <= 0 || height <= 0) return null;
    return WindowBounds(x: x, y: y, width: width, height: height);
  }

  @override
  bool operator ==(Object other) =>
      other is WindowBounds &&
      other.x == x &&
      other.y == y &&
      other.width == width &&
      other.height == height;

  @override
  int get hashCode => Object.hash(x, y, width, height);

  @override
  String toString() => 'WindowBounds($wire)';
}

/// The desktop window, as much of it as this app drives.
///
/// A seam over `window_manager` for two reasons. It keeps the plugin — which
/// declares only linux/macos/windows, so any call on Android throws
/// `MissingPluginException` — behind one gate, and it makes [WindowController]
/// testable without a platform channel.
abstract class WindowService {
  /// Window changes the desktop made itself.
  Stream<WindowEvent> get events;

  /// Prepares the plugin. Must complete before any other call.
  Future<void> initialize();

  Future<void> setTitle(String title);

  /// Hides or restores the *native* title bar.
  Future<void> setTitleBarHidden(bool hidden);

  Future<void> setFullScreen(bool value);

  Future<WindowBounds> getBounds();
  Future<void> setBounds(WindowBounds bounds);

  Future<bool> isMaximized();
  Future<void> setMaximized(bool value);
  Future<void> minimize();

  /// Begins a window drag, for a bar the app drew itself.
  Future<void> startDragging();

  /// Closes the window, which quits the app.
  Future<void> close();
}

/// The window for this platform, or **null where there is no window to manage**.
///
/// Null rather than a silent no-op implementation, so the absence is structural:
/// on Android there is no service, therefore no [WindowController], therefore no
/// window bar and no F11 handler. Nothing has to remember to check a flag.
WindowService? createWindowService() => switch (defaultTargetPlatform) {
  TargetPlatform.linux ||
  TargetPlatform.macOS ||
  TargetPlatform.windows => WindowManagerService(),
  _ => null,
};

/// The real window, via `window_manager`.
///
/// Uses `setTitleBarStyle(hidden)` and deliberately **never** `setAsFrameless`.
/// On the runner's header-bar path the former hides the header *widget* and
/// leaves the window GTK-decorated, so its invisible client-side resize border
/// survives and borderless needs no resize grips of its own; frameless forces
/// `decorated=false` and takes resizing with it.
class WindowManagerService with WindowListener implements WindowService {
  final _events = StreamController<WindowEvent>.broadcast();

  @override
  Stream<WindowEvent> get events => _events.stream;

  @override
  Future<void> initialize() async {
    await windowManager.ensureInitialized();
    windowManager.addListener(this);
  }

  @override
  Future<void> setTitle(String title) => windowManager.setTitle(title);

  @override
  Future<void> setTitleBarHidden(bool hidden) => windowManager.setTitleBarStyle(
    hidden ? TitleBarStyle.hidden : TitleBarStyle.normal,
  );

  @override
  Future<void> setFullScreen(bool value) => windowManager.setFullScreen(value);

  @override
  Future<WindowBounds> getBounds() async =>
      WindowBounds.fromRect(await windowManager.getBounds());

  @override
  Future<void> setBounds(WindowBounds bounds) =>
      windowManager.setBounds(bounds.rect);

  @override
  Future<bool> isMaximized() => windowManager.isMaximized();

  @override
  Future<void> setMaximized(bool value) =>
      value ? windowManager.maximize() : windowManager.unmaximize();

  @override
  Future<void> minimize() => windowManager.minimize();

  @override
  Future<void> startDragging() => windowManager.startDragging();

  @override
  Future<void> close() => windowManager.close();

  // ── WindowListener ─────────────────────────────────────────────────────────
  // Linux emits the continuous `move`/`resize` pair; the `moved`/`resized`
  // settled variants are macOS/Windows only, which is why the controller
  // debounces rather than relying on a drag-finished event.

  @override
  void onWindowMove() => _events.add(WindowEvent.moved);

  @override
  void onWindowResize() => _events.add(WindowEvent.resized);

  @override
  void onWindowMaximize() => _events.add(WindowEvent.maximized);

  @override
  void onWindowUnmaximize() => _events.add(WindowEvent.unmaximized);

  @override
  void onWindowEnterFullScreen() => _events.add(WindowEvent.enteredFullScreen);

  @override
  void onWindowLeaveFullScreen() => _events.add(WindowEvent.leftFullScreen);
}
