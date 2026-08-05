import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/services/window_service.dart';
import 'package:claude_commander_client/window/window_controller.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_window_service.dart';

/// A controller over a fake window, with a debounce short enough to await but
/// long enough that a burst of events emitted synchronously all land inside it.
({
  WindowController controller,
  FakeWindowService window,
  InMemoryPrefStore store,
})
_controller({Map<String, String>? prefs}) {
  final window = FakeWindowService();
  final store = InMemoryPrefStore(prefs);
  return (
    controller: WindowController(
      store: store,
      service: window,
      geometryDebounce: const Duration(milliseconds: 30),
    ),
    window: window,
    store: store,
  );
}

/// Long enough for the 30ms debounce above to have fired.
Future<void> _settle() =>
    Future<void>.delayed(const Duration(milliseconds: 90));

void main() {
  group('TitleBarMode.fromWire', () {
    test('round-trips every mode through its persisted spelling', () {
      for (final mode in TitleBarMode.values) {
        expect(TitleBarMode.fromWire(mode.wire), mode);
      }
    });

    test('defaults to borderless for absent or unknown values', () {
      // The desktop default is the app's own themed bar: a GTK3 window on
      // Wayland is always client-side decorated, so the "native" frame is a
      // GNOME-style header bar even in a KDE session.
      expect(TitleBarMode.fromWire(null), TitleBarMode.borderless);
      expect(TitleBarMode.fromWire(''), TitleBarMode.borderless);
      expect(TitleBarMode.fromWire('gnomish'), TitleBarMode.borderless);
    });

    test('the wire spellings are stable and not the Dart names', () {
      expect(TitleBarMode.native.wire, 'native');
      expect(TitleBarMode.borderless.wire, 'borderless');
    });
  });

  group('WindowBounds', () {
    test('round-trips through its persisted form', () {
      const b = WindowBounds(x: 12, y: 34, width: 1280, height: 720);
      expect(b.wire, '12,34,1280,720');
      expect(WindowBounds.fromWire(b.wire), b);
    });

    test('rejects malformed, short and absent values rather than throwing', () {
      // A hand-edited or truncated preferences file must cost the remembered
      // geometry, not the launch.
      expect(WindowBounds.fromWire(null), isNull);
      expect(WindowBounds.fromWire(''), isNull);
      expect(WindowBounds.fromWire('12,34,1280'), isNull);
      expect(WindowBounds.fromWire('12,34,1280,wide'), isNull);
    });

    test('rejects a zero or negative size', () {
      // A 0x0 window is unrecoverable without a titlebar to drag, so a bad
      // saved size must be ignored rather than applied.
      expect(WindowBounds.fromWire('0,0,0,0'), isNull);
      expect(WindowBounds.fromWire('0,0,1280,-4'), isNull);
    });
  });

  group('WindowController.load', () {
    test('applies the borderless default to a first-run window', () async {
      final c = _controller();
      await c.controller.load();

      expect(c.controller.titleBar, TitleBarMode.borderless);
      expect(c.controller.fullscreen, isFalse);
      expect(c.window.titleBarHidden, isTrue);
      expect(c.window.title, WindowController.windowTitle);
    });

    test('restores a persisted frame, fullscreen and geometry', () async {
      final c = _controller(
        prefs: {
          WindowController.titleBarPrefKey: 'native',
          WindowController.fullscreenPrefKey: 'true',
          WindowController.boundsPrefKey: '40,50,1000,800',
        },
      );
      await c.controller.load();

      expect(c.controller.titleBar, TitleBarMode.native);
      expect(c.controller.fullscreen, isTrue);
      expect(c.window.titleBarHidden, isFalse);
      expect(c.window.fullScreen, isTrue);
      expect(c.window.bounds.wire, '40,50,1000,800');
    });

    test('sets the geometry before going fullscreen', () async {
      // Otherwise the un-fullscreened size is whatever the runner defaulted to:
      // GTK remembers the pre-fullscreen geometry, so it has to be right first.
      final c = _controller(
        prefs: {
          WindowController.fullscreenPrefKey: 'true',
          WindowController.boundsPrefKey: '40,50,1000,800',
        },
      );
      await c.controller.load();

      expect(
        c.window.calls.indexOf('setBounds(40,50,1000,800)'),
        lessThan(c.window.calls.indexOf('setFullScreen(true)')),
      );
    });

    test('a throwing store leaves the defaults and does not throw', () async {
      final c = WindowController(
        store: _ThrowingPrefStore(),
        service: FakeWindowService(),
      );
      await c.load();
      expect(c.titleBar, TitleBarMode.borderless);
      expect(c.fullscreen, isFalse);
    });

    test('a throwing window service does not stop the launch', () async {
      // main() awaits load() before runApp, so a window that refuses a call must
      // not turn a cosmetic preference into a failure to start.
      final window = FakeWindowService()
        ..failingMethod = 'setTitleBarHidden(true)';
      final c = WindowController(store: InMemoryPrefStore(), service: window);
      await c.load();
      expect(c.titleBar, TitleBarMode.borderless);
    });
  });

  group('WindowController toggles', () {
    test('setFullscreen drives the window, notifies and persists', () async {
      final c = _controller();
      await c.controller.load();
      var notifications = 0;
      c.controller.addListener(() => notifications++);

      await c.controller.setFullscreen(true);

      expect(c.controller.fullscreen, isTrue);
      expect(c.window.fullScreen, isTrue);
      expect(notifications, 1);
      expect(await c.store.read(WindowController.fullscreenPrefKey), 'true');
    });

    test('setting the current value is a no-op', () async {
      final c = _controller();
      await c.controller.load();
      var notifications = 0;
      c.controller.addListener(() => notifications++);

      await c.controller.setFullscreen(false);
      await c.controller.setTitleBar(TitleBarMode.borderless);

      expect(notifications, 0);
    });

    test('setTitleBar drives the window, notifies and persists', () async {
      final c = _controller();
      await c.controller.load();

      await c.controller.setTitleBar(TitleBarMode.native);

      expect(c.window.titleBarHidden, isFalse);
      expect(await c.store.read(WindowController.titleBarPrefKey), 'native');
    });

    test(
      'the two toggles compose: leaving fullscreen keeps the frame',
      () async {
        // Why these are separate persisted values rather than one three-valued
        // mode: fullscreen has to return to whichever frame you were in.
        final c = _controller(
          prefs: {WindowController.titleBarPrefKey: 'native'},
        );
        await c.controller.load();

        await c.controller.toggleFullscreen();
        expect(c.controller.showWindowBar, isFalse);
        await c.controller.toggleFullscreen();

        expect(c.controller.fullscreen, isFalse);
        expect(c.controller.titleBar, TitleBarMode.native);
      },
    );

    test('showWindowBar is borderless and not fullscreen', () async {
      final c = _controller();
      await c.controller.load();
      expect(c.controller.showWindowBar, isTrue);

      await c.controller.setFullscreen(true);
      // GTK drops the titlebar in fullscreen, so an app-drawn drag bar there
      // would be chrome you cannot use.
      expect(c.controller.showWindowBar, isFalse);

      await c.controller.setFullscreen(false);
      await c.controller.setTitleBar(TitleBarMode.native);
      expect(c.controller.showWindowBar, isFalse);
    });
  });

  group('WindowController geometry', () {
    test('persists the window bounds after a move', () async {
      final c = _controller();
      await c.controller.load();
      c.window.bounds = const WindowBounds(x: 5, y: 6, width: 900, height: 500);

      c.window.emit(WindowEvent.moved);
      await _settle();

      expect(await c.store.read(WindowController.boundsPrefKey), '5,6,900,500');
    });

    test('coalesces a burst of resize events into one read', () async {
      final c = _controller();
      await c.controller.load();
      c.window.calls.clear();

      for (var i = 0; i < 5; i++) {
        c.window.emit(WindowEvent.resized);
      }
      await _settle();

      expect(c.window.calls.where((call) => call == 'getBounds'), hasLength(1));
    });

    test('never persists bounds while fullscreen', () async {
      // The bounds would be the screen, and restoring them on the next launch
      // would give a window with no way back to a usable size.
      final c = _controller();
      await c.controller.load();
      await c.controller.setFullscreen(true);

      c.window.emit(WindowEvent.resized);
      await _settle();

      expect(await c.store.read(WindowController.boundsPrefKey), isNull);
    });

    test('never persists bounds while maximized', () async {
      // Same trap one step smaller: saving the maximized size means unmaximizing
      // on the next launch lands you at screen size with no floating geometry to
      // return to.
      final c = _controller();
      await c.controller.load();
      c.window.emit(WindowEvent.maximized);
      await _settle();

      c.window.emit(WindowEvent.resized);
      await _settle();

      expect(await c.store.read(WindowController.boundsPrefKey), isNull);
      expect(c.controller.maximized, isTrue);
    });

    test('resumes persisting once unmaximized', () async {
      final c = _controller();
      await c.controller.load();
      c.window.emit(WindowEvent.maximized);
      await _settle();
      c.window.emit(WindowEvent.unmaximized);
      c.window.bounds = const WindowBounds(x: 1, y: 2, width: 800, height: 600);
      c.window.emit(WindowEvent.resized);
      await _settle();

      expect(c.controller.maximized, isFalse);
      expect(await c.store.read(WindowController.boundsPrefKey), '1,2,800,600');
    });

    test('a pending save after dispose does not fire', () async {
      final c = _controller();
      await c.controller.load();
      c.window.emit(WindowEvent.moved);
      c.controller.dispose();
      await _settle();

      expect(await c.store.read(WindowController.boundsPrefKey), isNull);
    });
  });

  group('WindowController external changes', () {
    test('follows a fullscreen change made by the desktop itself', () async {
      // KWin has its own fullscreen shortcut. If we ignored it our window bar
      // would be the one part of the UI disagreeing with the actual window.
      final c = _controller();
      await c.controller.load();
      var notifications = 0;
      c.controller.addListener(() => notifications++);

      c.window.emit(WindowEvent.enteredFullScreen);
      await _settle();

      expect(c.controller.fullscreen, isTrue);
      expect(c.controller.showWindowBar, isFalse);
      expect(notifications, 1);
      // Still persisted, so the next launch matches what the user left behind.
      expect(await c.store.read(WindowController.fullscreenPrefKey), 'true');
    });

    test('an external change already matching our state is quiet', () async {
      final c = _controller();
      await c.controller.load();
      var notifications = 0;
      c.controller.addListener(() => notifications++);

      c.window.emit(WindowEvent.leftFullScreen);
      await _settle();

      expect(notifications, 0);
    });
  });

  group('WindowController window-bar actions', () {
    test('forwards drag, minimize and close to the window', () async {
      final c = _controller();
      await c.controller.load();
      c.window.calls.clear();

      await c.controller.startDragging();
      await c.controller.minimize();
      await c.controller.close();

      expect(c.window.calls, ['startDragging', 'minimize', 'close']);
    });

    test('toggleMaximize flips the window and the reported state', () async {
      final c = _controller();
      await c.controller.load();

      await c.controller.toggleMaximize();
      expect(c.window.maximized, isTrue);
      expect(c.controller.maximized, isTrue);

      await c.controller.toggleMaximize();
      expect(c.window.maximized, isFalse);
      expect(c.controller.maximized, isFalse);
    });
  });
}

/// A [PrefStore] whose every call fails, standing in for a corrupt or
/// unreadable preferences backend.
class _ThrowingPrefStore implements PrefStore {
  @override
  Future<String?> read(String key) async => throw StateError('unreadable');

  @override
  Future<void> write(String key, String value) async =>
      throw StateError('unwritable');
}
