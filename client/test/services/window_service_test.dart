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

    // One `restored` covers both unmaximize and unminimize; see translateWindowEvent.
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
    const asked = Size(1000, 700);
    const grown = Size(1090, 790);
    final t0 = DateTime(2026);

    WindowBounds? resize(
      LaunchBoundsFix fix, {
      Size size = grown,
      bool visible = true,
      bool fullScreen = false,
      bool maximized = false,
      Duration at = Duration.zero,
    }) => fix.onResize(
      size: size,
      visible: visible,
      fullScreen: fullScreen,
      maximized: maximized,
      now: t0.add(at),
    );

    LaunchBoundsFix shown() => LaunchBoundsFix()
      ..arm(bounds)
      ..markShown(t0);

    test('re-applies a resize that misses the armed size', () {
      expect(resize(shown()), bounds);
    });

    test('ignores a resize that already matches, so its own echo is inert', () {
      expect(resize(shown(), size: asked), isNull);
    });

    test('leaves alone any resize that is not growth in both dimensions', () {
      // The drift it exists for always grows both. Anything else is the user
      // dragging an edge, a compositor clamping to a smaller screen, or a
      // tile — not to be fought, even inside the deadline.
      final fix = shown();
      expect(resize(fix, size: const Size(800, 600)), isNull, reason: 'clamp');
      expect(resize(fix, size: const Size(1200, 700)), isNull, reason: 'edge');
      expect(resize(fix, size: const Size(1000, 760)), isNull, reason: 'edge');
      expect(resize(fix, size: const Size(1200, 500)), isNull, reason: 'tile');
      expect(fix.armed, isTrue);
    });

    test('keeps correcting until the deadline, not just once', () {
      // Why every resize, not one: see the LaunchBoundsFix doc.
      final fix = shown();
      expect(resize(fix), bounds);
      expect(resize(fix, size: asked), isNull);
      expect(resize(fix, at: const Duration(milliseconds: 400)), bounds);
    });

    test('expires, so a user resize after launch is never snapped back', () {
      final fix = shown();
      expect(
        resize(fix, at: LaunchBoundsFix.deadline + const Duration(seconds: 1)),
        isNull,
      );
      expect(fix.armed, isFalse);
    });

    test('the deadline runs from the first frame even when no other event '
        'arrived, so the first event minutes later is a user resize', () {
      // A window that maps at exactly the asked size and gets no focus event
      // sends nothing at all until the user acts.
      final fix = shown();
      expect(resize(fix, at: const Duration(minutes: 3)), isNull);
    });

    test('a resize before the window shows is ignored and keeps it armed', () {
      final fix = LaunchBoundsFix()..arm(bounds);
      expect(resize(fix, visible: false), isNull);
      expect(fix.armed, isTrue);
    });

    test('a visible resize before the first-frame mark starts the clock', () {
      final fix = LaunchBoundsFix()..arm(bounds);
      expect(resize(fix), bounds);
      expect(
        resize(fix, at: LaunchBoundsFix.deadline + const Duration(seconds: 1)),
        isNull,
      );
    });

    test('never resizes a full-screen or maximized window, but stays armed '
        'for the rest of the window', () {
      final fix = shown();
      expect(resize(fix, fullScreen: true), isNull);
      expect(resize(fix, maximized: true), isNull);
      expect(fix.armed, isTrue);
    });

    test('does nothing unless armed', () {
      final fix = LaunchBoundsFix()..markShown(t0);
      expect(fix.armed, isFalse);
      expect(resize(fix), isNull);
    });
  });
}
