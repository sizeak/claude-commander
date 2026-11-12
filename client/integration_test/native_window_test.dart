// Drives the real `NativeWindowService` against the desktop's real window
// manager — the one layer `flutter test` cannot reach, because unit tests load
// no native library and `test/support/fake_window_service.dart` stands in for
// it there.
//
// Not part of the e2e lane on purpose: that runs under xvfb with no window
// manager, where maximize and full screen are requests nothing answers. Run it
// by hand on a real session after touching `window_service.dart` or bumping
// `window_manager`/nativeapi:
//
//   cd client && flutter test integration_test/native_window_test.dart -d linux
//
// It opens a window on your desktop, maximizes it, takes it full screen and
// minimizes it last.
import 'dart:async';

import 'package:claude_commander_client/services/window_service.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the native window answers every WindowService call', (
    tester,
  ) async {
    final service = createWindowService()!;
    final events = <WindowEvent>[];
    final sub = service.events.listen(events.add);
    addTearDown(sub.cancel);

    await service.initialize();

    // What `WindowController.load` does at launch, before the runner has shown
    // the window (it maps on the first frame): apply geometry saved while
    // borderless, then hide the title bar. The size must come back as saved,
    // or every launch drifts.
    const saved = WindowBounds(x: 40, y: 40, width: 1000, height: 700);
    await service.setTitleBarHidden(true);
    await service.setBounds(saved);
    await tester.pumpWidget(const MaterialApp(home: Scaffold()));
    await _settle(tester);
    final restored = await service.getBounds();
    expect((restored.width, restored.height), (saved.width, saved.height));

    /// Waits for [event] to arrive, pumping frames meanwhile.
    Future<void> expectEvent(WindowEvent event) async {
      for (var i = 0; i < 40 && !events.contains(event); i++) {
        await _settle(tester, const Duration(milliseconds: 100));
      }
      expect(events, contains(event), reason: 'events so far: $events');
      events.clear();
    }

    await service.setTitle('native_window_test');

    // Size round-trips in both frame modes. Position is not asserted: Wayland
    // does not let a client place its own window.
    for (final hidden in [false, true]) {
      await service.setTitleBarHidden(hidden);
      await _settle(tester);
      const want = WindowBounds(x: 40, y: 40, width: 900, height: 600);
      await service.setBounds(want);
      await _settle(tester);
      final got = await service.getBounds();
      expect(
        (got.width, got.height),
        (want.width, want.height),
        reason: 'titleBarHidden=$hidden: $got',
      );
    }

    events.clear();

    await service.setMaximized(true);
    await expectEvent(WindowEvent.maximized);
    expect(await service.isMaximized(), isTrue);

    await service.setMaximized(false);
    await expectEvent(WindowEvent.unmaximized);
    expect(await service.isMaximized(), isFalse);

    await service.setFullScreen(true);
    await expectEvent(WindowEvent.enteredFullScreen);
    await service.setFullScreen(false);
    await expectEvent(WindowEvent.leftFullScreen);

    // Last: a client cannot un-minimize itself on Wayland.
    await service.minimize();
    await _settle(tester);
  });
}

Future<void> _settle(
  WidgetTester tester, [
  Duration wait = const Duration(milliseconds: 500),
]) async {
  await tester.runAsync(() => Future<void>.delayed(wait));
  await tester.pump();
}
