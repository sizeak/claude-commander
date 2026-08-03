import 'dart:typed_data';

import 'package:claude_commander_client/pages/terminal_page.dart';
import 'package:claude_commander_client/services/commander_api.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:xterm/xterm.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;

  setUp(() => api = FakeCommanderApi());

  // `viewInsets` models the soft keyboard: the platform reports the height it
  // covers. `viewPadding` is the (keyboard-invariant) system chrome inset.
  Widget wrap({
    EdgeInsets viewInsets = EdgeInsets.zero,
    EdgeInsets viewPadding = const EdgeInsets.only(bottom: 24),
  }) => MaterialApp(
    home: Builder(
      builder: (context) => MediaQuery(
        data: MediaQuery.of(context).copyWith(
          viewInsets: viewInsets,
          viewPadding: viewPadding,
          // Flutter collapses `padding` to zero on any edge the keyboard
          // covers; mirror that so SafeArea behaves as it does on device.
          padding: viewInsets.bottom > 0 ? EdgeInsets.zero : viewPadding,
        ),
        child: TerminalPage(
          api: api,
          handle: testHandle,
          session: sessionInfo(),
        ),
      ),
    ),
  );

  TerminalEvent output(List<int> bytes) => TerminalEvent(
    kind: TerminalEventKind.output,
    bytes: Uint8List.fromList(bytes),
    text: '',
  );

  TerminalEvent signal(TerminalEventKind kind, String text) =>
      TerminalEvent(kind: kind, bytes: Uint8List(0), text: text);

  Terminal readTerminal(WidgetTester tester) =>
      tester.widget<TerminalView>(find.byType(TerminalView)).terminal;

  // Deliver a stream event and settle it: one pump runs the microtask that
  // delivers the event (single-subscription streams deliver async) and any
  // resulting setState; a second pump rebuilds with the new state.
  Future<void> emitAndPump(WidgetTester tester, TerminalEvent event) async {
    api.emit(event);
    await tester.pump();
    await tester.pump();
  }

  testWidgets('a ready event updates the status line', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pump(); // let _connect subscribe

    await emitAndPump(
      tester,
      signal(TerminalEventKind.ready, 'my-tmux-session'),
    );

    expect(find.textContaining('attached: my-tmux-session'), findsOneWidget);
  });

  testWidgets('a multi-byte codepoint split across two output events renders '
      'as one glyph', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pump();

    // '✓' (U+2713) is UTF-8 [0xE2, 0x9C, 0x93]; split it across two frames so
    // the chunked decoder must buffer the partial tail.
    await emitAndPump(tester, output([0xE2, 0x9C]));
    await emitAndPump(tester, output([0x93]));

    // Scan the whole buffer (not just the viewport range, which depends on a
    // laid-out view size) for the reassembled glyph.
    final buffer = readTerminal(tester).buffer;
    final text = [
      for (var i = 0; i < buffer.lines.length; i++) buffer.lines[i].getText(),
    ].join();
    expect(text, contains('✓'));
  });

  testWidgets('a detached event enables the reconnect button', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pump();

    // While live, reconnect is disabled.
    IconButton reconnect() => tester.widget<IconButton>(
      find.widgetWithIcon(IconButton, Icons.refresh),
    );
    expect(reconnect().onPressed, isNull);

    await emitAndPump(
      tester,
      signal(TerminalEventKind.detached, 'session ended'),
    );

    expect(reconnect().onPressed, isNotNull);
    expect(find.textContaining('detached: session ended'), findsOneWidget);
  });

  testWidgets('a ready event re-announces the terminal size', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pump(); // subscribe
    await tester.pump(); // let any layout-driven onResize fire first

    final before = api.countOf('terminalResize');
    await emitAndPump(tester, signal(TerminalEventKind.ready, 'sess'));

    // The server spawns each attach at 80x24 and only learns our size from an
    // explicit Resize, so `ready` must (re-)announce it.
    expect(api.countOf('terminalResize'), greaterThan(before));
  });

  testWidgets('reconnect re-announces the terminal size on the new ready', (
    tester,
  ) async {
    await tester.pumpWidget(wrap());
    await tester.pump();
    await tester.pump();

    await emitAndPump(tester, signal(TerminalEventKind.ready, 'sess'));
    final afterFirstReady = api.countOf('terminalResize');

    await emitAndPump(tester, signal(TerminalEventKind.detached, 'bye'));
    await tester.tap(find.widgetWithIcon(IconButton, Icons.refresh));
    await tester.pump();
    await emitAndPump(tester, signal(TerminalEventKind.ready, 'sess2'));

    // The reconnected PTY starts at 80x24 again; the same-size Terminal won't
    // fire onResize, so the new ready must re-announce.
    expect(api.countOf('terminalResize'), greaterThan(afterFirstReady));
  });

  testWidgets('tapping reconnect re-subscribes via attachTerminal', (
    tester,
  ) async {
    await tester.pumpWidget(wrap());
    await tester.pump();
    expect(api.attachTerminalCount, 1);

    await emitAndPump(tester, signal(TerminalEventKind.detached, 'bye'));

    await tester.tap(find.widgetWithIcon(IconButton, Icons.refresh));
    await tester.pump();

    expect(api.attachTerminalCount, 2);
  });

  // Resizing the remote pane when the soft keyboard appears is destructive:
  // tmux does not compensate a scrolled copy-mode view for the lines the shrink
  // pushes into the history, so the text on screen slides forward by a viewport
  // height and never comes back. The page pans instead of resizing, so the PTY
  // never learns the keyboard exists.
  testWidgets('the soft keyboard appearing does not resize the remote pane', (
    tester,
  ) async {
    await tester.pumpWidget(wrap());
    await tester.pump(); // subscribe
    await tester.pump(); // let the layout-driven onResize settle

    final resizesBefore = api.countOf('terminalResize');
    final rowsBefore = readTerminal(tester).viewHeight;

    // The keyboard slides in. Flutter animates the inset, so step through it
    // rather than jumping straight to the final height.
    for (final inset in [80.0, 180.0, 260.0, 320.0]) {
      await tester.pumpWidget(wrap(viewInsets: EdgeInsets.only(bottom: inset)));
      await tester.pump();
    }

    expect(
      readTerminal(tester).viewHeight,
      rowsBefore,
      reason: 'the pane must keep its rows while the keyboard covers it',
    );
    expect(
      api.countOf('terminalResize'),
      resizesBefore,
      reason: 'no resize may be sent to the server for a keyboard show',
    );

    // ...and dismissing it is equally silent.
    await tester.pumpWidget(wrap());
    await tester.pump();

    expect(readTerminal(tester).viewHeight, rowsBefore);
    expect(api.countOf('terminalResize'), resizesBefore);
  });

  testWidgets('the keyboard pans the pane up so the newest rows stay visible', (
    tester,
  ) async {
    await tester.pumpWidget(wrap());
    await tester.pump();
    await tester.pump();

    final before = tester.getRect(find.byType(TerminalView));

    await tester.pumpWidget(
      wrap(viewInsets: const EdgeInsets.only(bottom: 320)),
    );
    await tester.pump();

    final after = tester.getRect(find.byType(TerminalView));

    // Same height (that's what keeps the PTY's rows stable)...
    expect(after.height, before.height);
    // ...but slid up, so its bottom rows — the prompt and newest output — sit
    // above the keyboard rather than behind it.
    expect(after.top, lessThan(before.top));
    expect(after.bottom, lessThan(before.bottom));
    // The modifier bar is the only chrome between the pane and the keyboard.
    // The test surface is 800x600, so the keyboard's top edge is at y=280; the
    // bar lands at 256, the 24px short of it being the maintained viewPadding.
    final bar = tester.getRect(find.byType(ListView));
    expect(after.bottom, lessThanOrEqualTo(bar.top));
    expect(bar.bottom, lessThanOrEqualTo(600 - 320));
  });

  // Regression: the pinned height must not be computed from space that the
  // keyboard has already taken. A landscape phone's keyboard can be taller than
  // the whole pane, and an earlier version of this fix collapsed to a zero-height
  // pane there — which made the pinned height track the keyboard again (so the
  // resize, and the copy-mode jump, came back) and overflowed the layout.
  testWidgets('a keyboard taller than the pane still does not resize it', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 360);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(wrap());
    await tester.pump();
    await tester.pump();

    final resizesBefore = api.countOf('terminalResize');
    final rowsBefore = readTerminal(tester).viewHeight;

    // 250px of keyboard against a ~250px pane: more than the Column can give up.
    await tester.pumpWidget(
      wrap(viewInsets: const EdgeInsets.only(bottom: 250)),
    );
    await tester.pump();

    expect(readTerminal(tester).viewHeight, rowsBefore);
    expect(api.countOf('terminalResize'), resizesBefore);
    expect(tester.takeException(), isNull, reason: 'no layout overflow');
  });

  testWidgets('defaults to an agent-pane attach', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pump();

    expect(api.lastCall('attachTerminal')!.args['kind'], AttachKind.agent);
  });

  testWidgets('a shell page attaches to the paired shell pane', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: TerminalPage(
          api: api,
          handle: testHandle,
          session: sessionInfo(),
          kind: AttachKind.shell,
        ),
      ),
    );
    await tester.pump();

    expect(api.lastCall('attachTerminal')!.args['kind'], AttachKind.shell);
  });
}
