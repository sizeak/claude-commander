import 'dart:ui' show Offset, Size;

import 'package:claude_commander_client/services/window_service.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:window_manager/window_manager.dart' as native;

/// The window id every event below carries. [translateWindowEvent] is fed by a
/// listener on one window, so the id is never consulted.
const _id = 1;

void main() {
  group('translateWindowEvent', () {
    WindowEvent? translate(
      native.WindowEvent event, {
      bool maximized = false,
    }) => translateWindowEvent(event, isMaximized: () => maximized);

    test('geometry changes map one to one', () {
      expect(
        translate(
          native.WindowMovedEvent(
            windowId: _id,
            newPosition: Offset.zero.toNative(),
          ),
        ),
        WindowEvent.moved,
      );
      expect(
        translate(
          native.WindowResizedEvent(
            windowId: _id,
            newSize: const Size(800, 600).toNative(),
          ),
        ),
        WindowEvent.resized,
      );
    });

    test('full screen enter and exit map one to one', () {
      expect(
        translate(const native.WindowEnteredFullScreenEvent(windowId: _id)),
        WindowEvent.enteredFullScreen,
      );
      expect(
        translate(const native.WindowExitedFullScreenEvent(windowId: _id)),
        WindowEvent.leftFullScreen,
      );
    });

    test('maximize maps to maximized', () {
      expect(
        translate(
          const native.WindowMaximizedEvent(windowId: _id),
          maximized: true,
        ),
        WindowEvent.maximized,
      );
    });

    // nativeapi has no "unmaximized" event: leaving maximize, leaving minimize
    // and (on some compositors) leaving full screen all arrive as one
    // `restored`. Inferring which from the previous event goes wrong whenever an
    // event was missed, so the window is asked instead.
    test('a restore that leaves the window unmaximized is an unmaximize', () {
      expect(
        translate(const native.WindowRestoredEvent(windowId: _id)),
        WindowEvent.unmaximized,
      );
    });

    test('a restore from minimize back to maximized stays maximized', () {
      expect(
        translate(
          const native.WindowRestoredEvent(windowId: _id),
          maximized: true,
        ),
        WindowEvent.maximized,
      );
    });

    test('the maximize probe is consulted only for a restore', () {
      var probed = 0;
      translateWindowEvent(
        const native.WindowMaximizedEvent(windowId: _id),
        isMaximized: () {
          probed++;
          return true;
        },
      );
      expect(probed, 0);
    });

    test('events the controller has no use for are dropped', () {
      for (final event in <native.WindowEvent>[
        const native.WindowFocusedEvent(windowId: _id),
        const native.WindowBlurredEvent(windowId: _id),
        const native.WindowMinimizedEvent(windowId: _id),
        const native.WindowCreatedEvent(windowId: _id),
        const native.WindowClosedEvent(windowId: _id),
      ]) {
        expect(translate(event), isNull, reason: '$event');
      }
    });
  });

  group('LaunchBoundsFix', () {
    const bounds = WindowBounds(x: 0, y: 0, width: 1000, height: 700);
    final t0 = DateTime(2026);

    WindowBounds? event(
      LaunchBoundsFix fix, {
      bool resize = true,
      bool visible = true,
      bool fullScreen = false,
      bool maximized = false,
      Duration at = Duration.zero,
    }) => fix.onEvent(
      isResize: resize,
      visible: visible,
      fullScreen: fullScreen,
      maximized: maximized,
      now: t0.add(at),
    );

    test('re-applies once, on the first resize after the window shows', () {
      final fix = LaunchBoundsFix()..arm(bounds);
      expect(event(fix), bounds);
      expect(event(fix), isNull, reason: 'one shot');
    });

    test('a resize while the window is still unmapped keeps it pending', () {
      // X11 can configure a realized, unmapped window; spending the fix there
      // would let the map's own resize bring the grown size back.
      final fix = LaunchBoundsFix()..arm(bounds);
      expect(event(fix, visible: false), isNull);
      expect(event(fix), bounds);
    });

    test('expires when no resize follows the show, so a later user resize '
        'is never snapped back', () {
      // A window that maps at exactly the asked size sends no resize at all.
      final fix = LaunchBoundsFix()..arm(bounds);
      expect(event(fix, resize: false), isNull); // e.g. focus, on show
      expect(
        event(
          fix,
          at: LaunchBoundsFix.defaultWindow + const Duration(seconds: 1),
        ),
        isNull,
      );
    });

    test('a resize inside the window after the show still re-applies', () {
      final fix = LaunchBoundsFix()..arm(bounds);
      expect(event(fix, resize: false), isNull);
      expect(event(fix, at: const Duration(milliseconds: 300)), bounds);
    });

    test('never fights a full-screen or maximized window', () {
      final full = LaunchBoundsFix()..arm(bounds);
      expect(event(full, fullScreen: true), isNull);
      expect(event(full), isNull, reason: 'spent, not deferred');
      final max = LaunchBoundsFix()..arm(bounds);
      expect(event(max, maximized: true), isNull);
    });

    test('does nothing unless armed', () {
      expect(event(LaunchBoundsFix()), isNull);
    });
  });
}
