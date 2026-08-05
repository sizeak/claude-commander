import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:claude_commander_client/window/window_controller.dart';
import 'package:claude_commander_client/window/window_frame.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_window_service.dart';

void main() {
  /// Mirrors `main()`: the scope above the app, the frame inside the app's
  /// builder so it sits under the `Theme` and over every route.
  Future<void> pumpApp(
    WidgetTester tester, {
    required WindowController? controller,
  }) => tester.pumpWidget(
    WindowScope(
      controller: controller,
      child: MaterialApp(
        theme: themeDataFor(missionControlTokens),
        builder: (context, child) => WindowFrame(child: child!),
        home: const Scaffold(body: Text('page')),
      ),
    ),
  );

  ({WindowController controller, FakeWindowService window}) newController() {
    final window = FakeWindowService();
    return (
      controller: WindowController(store: InMemoryPrefStore(), service: window),
      window: window,
    );
  }

  testWidgets('draws the window bar over the app while borderless', (
    tester,
  ) async {
    final c = newController();
    await pumpApp(tester, controller: c.controller);

    expect(find.byType(ChromeWindowBar), findsOneWidget);
    expect(find.text('page'), findsOneWidget);
    // Above the page, not beside or under it.
    expect(
      tester.getTopLeft(find.byType(ChromeWindowBar)).dy,
      lessThan(tester.getTopLeft(find.text('page')).dy),
    );
  });

  testWidgets('no bar with the native title bar', (tester) async {
    final c = newController();
    await c.controller.setTitleBar(TitleBarMode.native);
    await pumpApp(tester, controller: c.controller);

    expect(find.byType(ChromeWindowBar), findsNothing);
  });

  testWidgets('no bar in fullscreen, even while borderless', (tester) async {
    final c = newController();
    await c.controller.setFullscreen(true);
    await pumpApp(tester, controller: c.controller);

    expect(find.byType(ChromeWindowBar), findsNothing);
  });

  testWidgets('follows the controller without a rebuild from above', (
    tester,
  ) async {
    final c = newController();
    await pumpApp(tester, controller: c.controller);
    expect(find.byType(ChromeWindowBar), findsOneWidget);

    await c.controller.setFullscreen(true);
    await tester.pump();

    expect(find.byType(ChromeWindowBar), findsNothing);
  });

  testWidgets('no controller means no bar at all', (tester) async {
    // Android: there is no window to manage, so there is no controller and the
    // frame is a pass-through. Nothing has to check a platform here.
    await pumpApp(tester, controller: null);

    expect(find.byType(ChromeWindowBar), findsNothing);
    expect(find.text('page'), findsOneWidget);
  });

  testWidgets('the bar has a full-window overlay for its tooltips', (
    tester,
  ) async {
    // The bar sits above the Navigator, so the app's own overlay is *below* it
    // and its controls' tooltips need one supplied here. Wrapping only the bar
    // gives them a 32px-tall overlay, which paints a tooltip and then clips it
    // to the bar — visible as a sliver of a label under the button.
    final c = newController();
    await pumpApp(tester, controller: c.controller);

    final overlay = find.ancestor(
      of: find.byType(ChromeWindowBar),
      matching: find.byType(Overlay),
    );
    expect(overlay, findsOneWidget);
    expect(tester.getSize(overlay), tester.getSize(find.byType(WindowFrame)));
  });

  testWidgets('a control tooltip is placed below the bar, inside the window', (
    tester,
  ) async {
    final c = newController();
    await pumpApp(tester, controller: c.controller);

    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    addTearDown(mouse.removePointer);
    await mouse.addPointer(location: Offset.zero);
    await mouse.moveTo(tester.getCenter(find.byTooltip('Maximise')));
    await tester.pump(const Duration(seconds: 1));

    final label = find.text('Maximise');
    expect(label, findsOneWidget);
    final rect = tester.getRect(label);
    final bar = tester.getRect(find.byType(ChromeWindowBar));
    // Below the bar and within the window: a tooltip laid out inside a
    // bar-height overlay is squeezed against the bar and clipped by it.
    expect(rect.top, greaterThanOrEqualTo(bar.bottom));
    expect(
      tester.getRect(find.byType(WindowFrame)).contains(rect.topLeft),
      isTrue,
    );
  });

  testWidgets('the bar drives the window through the controller', (
    tester,
  ) async {
    final c = newController();
    await pumpApp(tester, controller: c.controller);
    c.window.calls.clear();

    await tester.tap(find.byTooltip('Minimise'));
    await tester.pump();

    expect(c.window.calls, contains('minimize'));
  });

  group('F11', () {
    testWidgets('toggles fullscreen', (tester) async {
      final c = newController();
      await pumpApp(tester, controller: c.controller);

      await tester.sendKeyEvent(LogicalKeyboardKey.f11);
      await tester.pumpAndSettle();

      expect(c.controller.fullscreen, isTrue);
      expect(c.window.fullScreen, isTrue);
      expect(find.byType(ChromeWindowBar), findsNothing);

      await tester.sendKeyEvent(LogicalKeyboardKey.f11);
      await tester.pumpAndSettle();

      expect(c.controller.fullscreen, isFalse);
      expect(find.byType(ChromeWindowBar), findsOneWidget);
    });

    testWidgets('with shift toggles the title bar instead', (tester) async {
      final c = newController();
      await pumpApp(tester, controller: c.controller);

      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.f11);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.pumpAndSettle();

      expect(c.controller.titleBar, TitleBarMode.native);
      expect(c.controller.fullscreen, isFalse);
    });

    testWidgets('is not consumed when there is no window to manage', (
      tester,
    ) async {
      await pumpApp(tester, controller: null);

      // No controller, no handler: the key must fall through to the app rather
      // than being swallowed by a frame that cannot act on it.
      final handled = await tester.sendKeyEvent(LogicalKeyboardKey.f11);
      expect(handled, isFalse);
    });

    testWidgets('other keys are left alone for the terminal', (tester) async {
      // The frame's handler runs before the focus tree, ahead of the terminal's
      // key handling, so it must claim F11 and nothing else.
      final c = newController();
      await pumpApp(tester, controller: c.controller);

      final handled = await tester.sendKeyEvent(LogicalKeyboardKey.keyA);

      expect(handled, isFalse);
      expect(c.window.calls, isNot(contains('setFullScreen(true)')));
    });

    testWidgets('releases its handler when the frame goes away', (
      tester,
    ) async {
      final c = newController();
      await pumpApp(tester, controller: c.controller);
      await tester.pumpWidget(const SizedBox());

      final handled = await tester.sendKeyEvent(LogicalKeyboardKey.f11);

      expect(handled, isFalse);
      expect(c.controller.fullscreen, isFalse);
    });
  });
}
