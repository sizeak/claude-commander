import 'dart:async';
import 'dart:ui' show Rect, Size;

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show WidgetsBinding;
import 'package:window_manager/window_manager.dart' as native;

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
///
/// The **position** half only takes effect on X11: Wayland does not let a client
/// place its own window, and nativeapi positions through `gtk_window_move`
/// (cnativeapi 0.5.10, `window_linux.cpp:1956-1962`), a no-op there and a restored
/// window keeps its size but lands wherever the compositor puts it. Worth knowing
/// before debugging "the position isn't restored".
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
/// A seam over `window_manager` for two reasons. It keeps the desktop-only
/// native window API behind one gate — `window_manager` supports only
/// linux/macos/windows — and it makes [WindowController] testable without the
/// native library, which `flutter test` never loads.
abstract class WindowService {
  /// Window changes the desktop made itself.
  Stream<WindowEvent> get events;

  /// Resolves the window. Must complete before any other call.
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
  TargetPlatform.windows => NativeWindowService(),
  _ => null,
};

/// The real window, via `window_manager`'s native API (nativeapi).
///
/// Uses `titleBarStyle = hidden` and deliberately **never** `setFrameless`. On
/// Linux, where the runner gave the window a GTK header bar — Wayland, and X11
/// under GNOME — hiding the title bar hides that header *widget* and keeps the
/// window GTK-decorated, so its invisible client-side resize border survives and
/// borderless needs no resize grips of its own; `setFrameless(true)` also calls
/// `gtk_window_set_decorated(FALSE)` and takes resizing with it (cnativeapi
/// 0.5.10, `cxx_impl/src/platform/linux/window_linux.cpp:2055-2068` for
/// frameless, `:2103-2120` for the header-bar branch).
///
/// Off that path the two converge: with no header bar, hiding the title bar
/// falls through to `gtk_window_set_decorated(false)` itself (same file,
/// `:2106-2112`), so borderless on X11 outside GNOME *does* lose the resize
/// border. Not accommodated on purpose — X11 is deprecated and out of scope —
/// but do not read the paragraph above as universal.
class NativeWindowService implements WindowService {
  final _events = StreamController<WindowEvent>.broadcast();
  native.Window? _window;
  final _launchBounds = LaunchBoundsFix();

  /// The window [initialize] resolved. Throws before then, which
  /// [WindowController]'s failure containment turns into a no-op.
  native.Window get _target =>
      _window ?? (throw StateError('NativeWindowService is not initialized'));

  @override
  Stream<WindowEvent> get events => _events.stream;

  /// Resolves the app's window once and listens to it.
  ///
  /// Resolved once rather than per call because `getCurrent()` is "the focused
  /// window, falling back to the first visible one" (cnativeapi 0.5.10,
  /// `window_manager_linux.cpp:894-901`): asked later, it could answer some
  /// other toplevel. Asked now, before the first frame, it answers the runner's
  /// realized-but-unshown window, which that same comment documents as the
  /// supported case.
  @override
  Future<void> initialize() async {
    if (_window != null) return;
    final window =
        native.WindowManager.instance.getCurrent() ??
        (throw StateError('nativeapi found no window'));
    _window = window;
    window.addListener((event) {
      // `armed` only spares the FFI reads below once the fix is spent;
      // `onResize` is the real guard.
      if (event is native.WindowResizedEvent && _launchBounds.armed) {
        final relaunch = _launchBounds.onResize(
          size: event.newSize.toSize(),
          visible: window.isVisible,
          fullScreen: window.isFullScreen,
          maximized: window.isMaximized,
          now: DateTime.now(),
        );
        if (relaunch != null) window.bounds = relaunch.rect.toNative();
      }
      final translated = translateWindowEvent(
        event,
        isMaximized: () => window.isMaximized,
      );
      if (translated != null) _events.add(translated);
    });
  }

  // The native API is synchronous (FFI); the seam stays async so the fake and
  // the controller's failure containment are unchanged.

  @override
  Future<void> setTitle(String title) async => _target.title = title;

  @override
  Future<void> setTitleBarHidden(bool hidden) async => _target.titleBarStyle =
      hidden ? native.TitleBarStyle.hidden : native.TitleBarStyle.normal;

  @override
  Future<void> setFullScreen(bool value) async => _target.isFullScreen = value;

  @override
  Future<WindowBounds> getBounds() async =>
      WindowBounds.fromRect(_target.bounds.toRect());

  @override
  Future<void> setBounds(WindowBounds bounds) async {
    final window = _target;
    // Still applied now, so the first frame is near the right size; the
    // re-apply after mapping makes it exact. See [LaunchBoundsFix].
    window.bounds = bounds.rect.toNative();
    if (defaultTargetPlatform != TargetPlatform.linux || window.isVisible) {
      return;
    }
    if (!_launchBounds.armed) {
      // The runner shows the window on the first frame, so that is when the
      // deadline starts (see [LaunchBoundsFix.markShown]).
      WidgetsBinding.instance.waitUntilFirstFrameRasterized.then(
        (_) => _launchBounds.markShown(DateTime.now()),
      );
    }
    _launchBounds.arm(bounds);
  }

  @override
  Future<bool> isMaximized() async => _target.isMaximized;

  @override
  Future<void> setMaximized(bool value) async =>
      value ? _target.maximize() : _target.unmaximize();

  @override
  Future<void> minimize() async => _target.minimize();

  @override
  Future<void> startDragging() async => _target.startDragging();

  @override
  Future<void> close() async => _target.close();
}

/// Re-applies bounds set before the window was first shown, once it has been.
///
/// nativeapi 0.5.10 on Linux sizes a not-yet-shown window wrongly while the
/// title bar is hidden: it maps 90px wider and taller than asked, while the same
/// call after it is shown is exact. Pinned by
/// `integration_test/native_window_test.dart`. The cause appears to be the
/// gutter nativeapi reserves for its own hidden-title-bar shadow, which it adds
/// after map (`window_shadow_linux.h`, `AddGutter`, reached from `Decide` on the
/// compositor's first state event or a 300ms timeout) — unconfirmed beyond the
/// matching 45px a side. Left alone it compounds: the map's own resize events
/// make [WindowController] save the grown size, so every launch would open 90px
/// bigger than the last.
///
/// For [deadline] after the first frame (when the runner shows the window),
/// every resize that leaves the window larger than the armed size in *both*
/// dimensions is answered with the armed bounds again — every one, not just the
/// first, because the gutter can land after an earlier map-time configure.
/// Anything else is left alone even inside the deadline: a resize to the armed
/// size is the fix's own echo, and one that shrinks or grows a single dimension
/// is the user dragging an edge, a compositor clamping the window to a smaller
/// screen, or a tile. Full screen and maximized are left alone (they are the
/// screen's size) without disarming. Once [deadline] has passed it disarms.
///
/// Pure, so `flutter test` covers it; [NativeWindowService] feeds it each native
/// resize with the window's state.
///
/// A workaround, not a fix: neither `hasShadow = false` nor `setFrameless`
/// avoids the gutter, which nativeapi keeps reserved while its shadow is
/// disabled. Remove it once a cnativeapi release maps a pre-show `bounds`
/// exactly — `integration_test/native_window_test.dart` is the check.
class LaunchBoundsFix {
  static const deadline = Duration(seconds: 2);

  WindowBounds? _bounds;
  DateTime? _shownAt;

  bool get armed => _bounds != null;

  void arm(WindowBounds bounds) {
    _bounds = bounds;
    _shownAt = null;
  }

  /// Starts the deadline. Called once the first frame — and so the window — is
  /// up; a visible resize that arrives first starts it instead.
  void markShown(DateTime now) => _shownAt ??= now;

  /// The bounds to apply in answer to a resize to [size], or null.
  WindowBounds? onResize({
    required Size size,
    required bool visible,
    required bool fullScreen,
    required bool maximized,
    required DateTime now,
  }) {
    final bounds = _bounds;
    if (bounds == null || !visible) return null;
    final shownAt = _shownAt ??= now;
    if (now.difference(shownAt) > deadline) {
      _bounds = null;
      return null;
    }
    if (fullScreen || maximized) return null;
    // Only growth in both dimensions is the drift; see the class doc.
    final grown =
        size.width.round() > bounds.width &&
        size.height.round() > bounds.height;
    return grown ? bounds : null;
  }
}

/// Translates one nativeapi window event into the [WindowEvent] the controller
/// consumes, or null for an event it has no use for.
///
/// nativeapi has no "unmaximized" event: leaving maximize and leaving minimize
/// both arrive as one `WindowRestoredEvent` (window_manager 0.6.0's own legacy
/// bridge has to disambiguate them, `lib/src/legacy/window_manager.dart:778-787`).
/// Rather than infer which from the events seen before — wrong whenever one was
/// missed — a restore asks the window, via [isMaximized], what it now is. A
/// restore that changes nothing is harmless: the controller drops an event that
/// matches its state. The probe reads the window when Dart *receives* the event
/// (`NativeCallable.listener` delivers asynchronously), not when it happened —
/// fine here, since the controller wants the window's current state anyway.
///
/// Linux reports only the continuous move/resize stream, one event per change,
/// with no settled "drag finished" event, which is why the controller debounces.
WindowEvent? translateWindowEvent(
  native.WindowEvent event, {
  required bool Function() isMaximized,
}) => switch (event) {
  native.WindowMovedEvent() => WindowEvent.moved,
  native.WindowResizedEvent() => WindowEvent.resized,
  native.WindowMaximizedEvent() => WindowEvent.maximized,
  native.WindowRestoredEvent() =>
    isMaximized() ? WindowEvent.maximized : WindowEvent.unmaximized,
  native.WindowEnteredFullScreenEvent() => WindowEvent.enteredFullScreen,
  native.WindowExitedFullScreenEvent() => WindowEvent.leftFullScreen,
  _ => null,
};
