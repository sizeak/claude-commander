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
}
