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

  Widget wrap() => MaterialApp(
    home: TerminalPage(api: api, handle: testHandle, session: sessionInfo()),
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
