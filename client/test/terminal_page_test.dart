import 'dart:async';
import 'dart:convert';

import 'package:claude_commander_client/pages/terminal_page.dart';
import 'package:claude_commander_client/services/commander_api.dart';
import 'package:claude_commander_client/services/image_picker_service.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:xterm/xterm.dart';

import 'support/fake_commander_api.dart';
import 'support/fake_image_sources.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;

  setUp(() => api = FakeCommanderApi());

  // `viewInsets` models the soft keyboard: the platform reports the height it
  // covers. `viewPadding` is the (keyboard-invariant) system chrome inset.
  Widget wrap({
    EdgeInsets viewInsets = EdgeInsets.zero,
    EdgeInsets viewPadding = const EdgeInsets.only(bottom: 24),
    DateTime Function()? clock,
    Terminal Function()? terminalFactory,
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
          clock: clock,
          terminalFactory: terminalFactory,
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

  // Agent TUIs repaint a line by erasing it from column 0 — `CSI 1 K` with the
  // cursor already at column 0 — and `omp` emits that hundreds of times per
  // screenful. It used to throw a RangeError out of `Terminal.write`, which
  // abandoned the rest of that 8 KiB chunk: tmux only ever sends incremental
  // repaints, so the discarded bytes never came again and the pane stayed
  // garbled for the life of the attach. Guards the pinned `xterm` fork.
  testWidgets('output containing an erase-to-cursor at column 0 still renders '
      'the rest of its chunk', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pump();

    await emitAndPump(tester, output(utf8.encode('\x1b[1Kpost-erase')));

    final buffer = readTerminal(tester).buffer;
    final text = [
      for (var i = 0; i < buffer.lines.length; i++) buffer.lines[i].getText(),
    ].join();
    expect(text, contains('post-erase'));
  });

  // An emulator that throws mid-parse loses the rest of that chunk, and tmux
  // never resends it — so the pane is stale for the life of the attach. What
  // must not also happen is the throw escaping: one uncaught async error per
  // chunk, each dumping a stack trace on the UI thread, is what turned a
  // garbled `omp` pane into an unresponsive app.
  testWidgets('an emulator that throws leaves the attach up and says the pane '
      'is stale', (tester) async {
    await tester.pumpWidget(wrap(terminalFactory: _ThrowingTerminal.new));
    await tester.pump();
    await emitAndPump(tester, signal(TerminalEventKind.ready, 'sess'));

    expect(find.textContaining('stale'), findsNothing);

    await emitAndPump(tester, output(utf8.encode('anything')));

    // Reported once, not swallowed.
    expect(tester.takeException(), isA<StateError>());
    // Still attached, and the pane is flagged rather than silently wrong.
    expect(find.textContaining('attached: sess'), findsOneWidget);
    expect(find.textContaining('stale'), findsOneWidget);

    // A second failing chunk is counted, not re-reported: the flood is the
    // problem, so only the first one raises.
    await emitAndPump(tester, output(utf8.encode('more')));
    expect(tester.takeException(), isNull);
    expect(find.textContaining('stale'), findsOneWidget);
  });

  // The reconnect button must never be gated on the attach *looking* dead. If
  // the network path vanishes without a TCP FIN reaching us (Wi-Fi drop, cell
  // handoff) the socket goes half-open: no detach event ever arrives, so the UI
  // still reads "attached" while the pane is frozen. Disabling the button in
  // exactly that state is what turns a recoverable stall into a dead end.
  testWidgets(
    'the reconnect button stays enabled while the attach looks live',
    (tester) async {
      await tester.pumpWidget(wrap());
      await tester.pump();

      IconButton reconnect() => tester.widget<IconButton>(
        find.widgetWithIcon(IconButton, Icons.refresh),
      );
      expect(reconnect().onPressed, isNotNull);

      await emitAndPump(
        tester,
        signal(TerminalEventKind.detached, 'session ended'),
      );

      expect(reconnect().onPressed, isNotNull);
      expect(find.textContaining('detached: session ended'), findsOneWidget);
    },
  );

  // Reconnecting while the old attach may still be live must tear the old one
  // down first, or it becomes a zombie: the cdylib's registry entry keeps its
  // control sender alive, so the pump's `rx.recv()` never ends, and the pump only
  // notices Dart dropped the stream by failing to push an Output frame — which
  // never arrives on the half-open socket this button exists to escape. The
  // leaked pump holds the WS open and, while the network is fine, keeps answering
  // the server's pings, so the server keeps a `tmux attach-session` child alive.
  testWidgets('reconnecting detaches the previous attach before re-attaching', (
    tester,
  ) async {
    await tester.pumpWidget(wrap());
    await tester.pump();

    final firstId = api.lastCall('attachTerminal')!.args['attachId'] as String;

    // Note: no detached/error event — the attach still looks live, which is
    // exactly the half-open case.
    await tester.tap(find.widgetWithIcon(IconButton, Icons.refresh));
    await tester.pump();

    // Assert on ordering, not just that a detach happened: detaching after the
    // id is regenerated would tear down the *new* attach instead of the old one.
    final detachIndex = api.calls.indexWhere(
      (c) => c.method == 'terminalDetach' && c.args['attachId'] == firstId,
    );
    expect(
      detachIndex,
      isNonNegative,
      reason: 'the superseded attach must be detached, not abandoned',
    );
    final reattachIndex = api.calls.lastIndexWhere(
      (c) => c.method == 'attachTerminal',
    );
    expect(detachIndex, lessThan(reattachIndex));

    // And it really did re-attach, under a fresh id.
    expect(api.attachTerminalCount, 2);
    expect(api.calls[reattachIndex].args['attachId'], isNot(firstId));
  });

  // Android walks resumed → inactive → hidden → paused on the way out and back
  // in again on the way in; `handleAppLifecycleStateChanged` validates those
  // transitions, so the tests must walk them rather than jumping.
  Future<void> background(WidgetTester tester) async {
    for (final state in const [
      AppLifecycleState.inactive,
      AppLifecycleState.hidden,
      AppLifecycleState.paused,
    ]) {
      tester.binding.handleAppLifecycleStateChanged(state);
    }
    await tester.pump();
  }

  Future<void> foreground(WidgetTester tester) async {
    for (final state in const [
      AppLifecycleState.hidden,
      AppLifecycleState.inactive,
      AppLifecycleState.resumed,
    ]) {
      tester.binding.handleAppLifecycleStateChanged(state);
    }
    await tester.pump();
  }

  // A frozen background process can't answer the server's heartbeat pings, so
  // past the shared deadline the server has certainly killed the attach — even
  // though a dropped network path means no detach frame ever reached us. Coming
  // back to a permanently dead pane is the bug; re-attach without being asked.
  testWidgets('foregrounding after the heartbeat deadline re-attaches', (
    tester,
  ) async {
    var now = DateTime(2026, 8, 3, 12);
    await tester.pumpWidget(wrap(clock: () => now));
    await tester.pump(); // subscribe
    await tester.pump(); // let attachDeadAfter resolve
    expect(api.attachTerminalCount, 1);

    await background(tester);
    // Longer than the 60s the fake reports, so the attach is provably gone.
    now = now.add(const Duration(minutes: 5));
    await foreground(tester);

    expect(api.attachTerminalCount, 2);
  });

  // The other half of the contract: below the deadline the attach may well still
  // be live, and re-attaching spawns a fresh tmux attach child — which loses a
  // scrolled copy-mode view. A glance at a notification must not cost the user
  // their place in the scrollback.
  testWidgets('a brief background does not disturb a live attach', (
    tester,
  ) async {
    var now = DateTime(2026, 8, 3, 12);
    await tester.pumpWidget(wrap(clock: () => now));
    await tester.pump();
    await tester.pump();
    expect(api.attachTerminalCount, 1);

    await background(tester);
    now = now.add(const Duration(seconds: 5));
    await foreground(tester);

    expect(api.attachTerminalCount, 1);
  });

  // When the detach *did* reach us the clock is irrelevant — the pane is known
  // dead, so a resume of any length must re-attach it.
  testWidgets('foregrounding re-attaches an attach that already ended', (
    tester,
  ) async {
    var now = DateTime(2026, 8, 3, 12);
    await tester.pumpWidget(wrap(clock: () => now));
    await tester.pump();
    await tester.pump();

    await emitAndPump(tester, signal(TerminalEventKind.error, 'lost'));
    expect(api.attachTerminalCount, 1);

    await background(tester);
    now = now.add(const Duration(seconds: 1));
    await foreground(tester);

    expect(api.attachTerminalCount, 2);
  });

  // The attach handshake carries the size, so the server can open the PTY at it
  // *before* spawning `tmux attach-session`. Getting this wrong is not a
  // cosmetic first frame: tmux paints a whole screen into the socket the moment
  // the attach starts, and its later repaints are incremental with no
  // full-screen clear — so a paint that arrived at the wrong width is wrapped by
  // the emulator at its own width and never corrected, desynchronising the pane
  // for the life of the attach. Against a page that attached from `initState`
  // this is red: the terminal has not been laid out yet, so it hands over
  // xterm's 80x24 default instead of the view's real size — which is precisely
  // the server's own fallback geometry, so the bug would reproduce exactly.
  testWidgets('the attach announces the laid-out size, not xterm defaults', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1080, 2400);
    tester.view.devicePixelRatio = 2.75;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(wrap());
    await tester.pump(); // run the post-frame callback that opens the attach
    await tester.pump();

    final call = api.lastCall('attachTerminal')!;
    final laidOut = readTerminal(tester);
    expect(
      [call.args['cols'], call.args['rows']],
      [laidOut.viewWidth, laidOut.viewHeight],
      reason:
          'the handshake must carry the view size the emulator will render '
          'at, or tmux paints its first screen at a width we then re-wrap',
    );
    // Guard the specific regression: xterm's un-laid-out default is 80x24
    // (terminal.dart:116-118), the very geometry the server falls back to.
    expect(call.args['cols'], isNot(80));
  });

  // The reconnect button is the rescue path, and it installs a *fresh*
  // `Terminal` — which reports 80x24 until the re-keyed view lays out. If that
  // ordering ever slipped, reconnect would hand the server exactly the broken
  // geometry and silently restore the reported symptom ("reconnect doesn't fix
  // it"), while the initial-attach test above stayed green.
  testWidgets('a reconnect also announces the laid-out size', (tester) async {
    tester.view.physicalSize = const Size(1080, 2400);
    tester.view.devicePixelRatio = 2.75;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(wrap());
    await tester.pump();
    await tester.pump();
    final first = api.lastCall('attachTerminal')!;

    await tester.tap(find.widgetWithIcon(IconButton, Icons.refresh));
    await tester.pump();
    await tester.pump();

    expect(api.attachTerminalCount, 2, reason: 'precondition: it re-attached');
    final second = api.lastCall('attachTerminal')!;
    expect(second.args['attachId'], isNot(first.args['attachId']));
    final laidOut = readTerminal(tester);
    expect(
      [second.args['cols'], second.args['rows']],
      [laidOut.viewWidth, laidOut.viewHeight],
    );
    expect(second.args['cols'], isNot(80));
  });

  // The reconnect button is the only way out of a desynchronised pane: tmux's
  // stream carries no full-screen clear, so a grid that has stopped matching
  // tmux's screen model is never repaired by anything that arrives on it. A
  // fresh attach repaints the whole screen, but only onto a blank grid.
  testWidgets('an explicit reconnect clears the emulator', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pump();
    await tester.pump();

    await emitAndPump(tester, output(utf8.encode('stale-content-marker')));
    String bufferText() {
      final buffer = readTerminal(tester).buffer;
      return [
        for (var i = 0; i < buffer.lines.length; i++) buffer.lines[i].getText(),
      ].join();
    }

    expect(bufferText(), contains('stale-content-marker'));

    await tester.tap(find.widgetWithIcon(IconButton, Icons.refresh));
    await tester.pump();
    await tester.pump();

    expect(
      bufferText(),
      isNot(contains('stale-content-marker')),
      reason:
          'reconnect must start from a blank grid for the fresh attach\'s '
          'full-screen repaint to land on',
    );
  });

  // ...but the automatic resume path must NOT wipe the buffer: re-attaching
  // already costs the user their scrolled position, and doing it on every glance
  // at a notification would compound the loss this path exists to avoid.
  testWidgets('an automatic re-attach keeps the existing buffer', (
    tester,
  ) async {
    var now = DateTime(2026, 8, 3, 12);
    await tester.pumpWidget(wrap(clock: () => now));
    await tester.pump();
    await tester.pump();

    await emitAndPump(tester, output(utf8.encode('keep-me-marker')));

    await background(tester);
    now = now.add(const Duration(minutes: 5)); // past the heartbeat deadline
    await foreground(tester);
    await tester.pump();

    expect(api.attachTerminalCount, 2, reason: 'precondition: it re-attached');
    final buffer = readTerminal(tester).buffer;
    final text = [
      for (var i = 0; i < buffer.lines.length; i++) buffer.lines[i].getText(),
    ].join();
    expect(text, contains('keep-me-marker'));
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

  group('the on-screen Ctrl key', () {
    // The row can only carry a fixed handful of chords, so Ctrl arms the *next*
    // character instead of sending one of its own. It sits directly after Tab,
    // ahead of the preset ^C/^D chords it generalises.
    Finder ctrlKey() => find.text('Ctrl');

    /// The soft keyboard's path into the emulator: a typed character reaches
    /// `Terminal.onOutput` exactly as `textInput` does here.
    Future<void> type(WidgetTester tester, String text) async {
      readTerminal(tester).textInput(text);
      await tester.pump();
    }

    List<int>? lastInput() =>
        api.lastCall('terminalSendInput')?.args['bytes'] as List<int>?;

    testWidgets('sits between Tab and the preset chords', (tester) async {
      await tester.pumpWidget(wrap());
      await tester.pump();

      final tab = tester.getCenter(find.text('Tab')).dx;
      final ctrl = tester.getCenter(ctrlKey()).dx;
      final ctrlC = tester.getCenter(find.text('^C')).dx;

      expect(ctrl, greaterThan(tab));
      expect(ctrl, lessThan(ctrlC));
    });

    testWidgets('folds the next typed character into its control byte', (
      tester,
    ) async {
      await tester.pumpWidget(wrap());
      await tester.pump();

      await tester.tap(ctrlKey());
      await tester.pump();
      await type(tester, 'w');

      expect(lastInput(), [0x17]);
    });

    testWidgets('arms one keystroke only', (tester) async {
      await tester.pumpWidget(wrap());
      await tester.pump();

      await tester.tap(ctrlKey());
      await tester.pump();
      await type(tester, 'w');
      await type(tester, 'w');

      expect(lastInput(), utf8.encode('w'));
    });

    testWidgets('sends unmodified characters when it is not armed', (
      tester,
    ) async {
      await tester.pumpWidget(wrap());
      await tester.pump();

      await type(tester, 'w');

      expect(lastInput(), utf8.encode('w'));
    });

    testWidgets('a second tap disarms it', (tester) async {
      await tester.pumpWidget(wrap());
      await tester.pump();

      await tester.tap(ctrlKey());
      await tester.pump();
      await tester.tap(ctrlKey());
      await tester.pump();
      await type(tester, 'w');

      expect(lastInput(), utf8.encode('w'));
    });

    // A held arm has to be visible, or the next keystroke lands somewhere the
    // user did not expect with nothing on screen to explain why.
    testWidgets('shows that it is armed', (tester) async {
      await tester.pumpWidget(wrap());
      await tester.pump();

      Color? fill() => tester
          .widget<Material>(
            find.ancestor(of: ctrlKey(), matching: find.byType(Material)).first,
          )
          .color;
      final resting = fill();

      await tester.tap(ctrlKey());
      await tester.pump();

      expect(fill(), isNot(resting));
    });

    // A paste is not the keystroke the arm was set for: eating it would both
    // mangle the pasted text and leave the user's Ctrl silently spent.
    testWidgets('a paste does not consume the arm', (tester) async {
      await tester.pumpWidget(wrap());
      await tester.pump();

      await tester.tap(ctrlKey());
      await tester.pump();
      readTerminal(tester).paste('hello');
      await tester.pump();
      expect(lastInput(), utf8.encode('hello'));

      await type(tester, 'w');
      expect(lastInput(), [0x17]);
    });
  });

  group('image attach', () {
    late FakeImagePicker picker;
    late FakeClipboardImageReader clipboard;

    setUp(() {
      picker = FakeImagePicker();
      clipboard = FakeClipboardImageReader();
    });

    Widget wrapWithSources({AttachKind kind = AttachKind.agent}) => MaterialApp(
      home: TerminalPage(
        api: api,
        handle: testHandle,
        session: sessionInfo(),
        kind: kind,
        imagePicker: picker,
        clipboardImages: clipboard,
      ),
    );

    Finder attachButton() =>
        find.widgetWithIcon(IconButton, Icons.image_outlined);

    testWidgets('an agent attach offers the attach-image action', (
      tester,
    ) async {
      await tester.pumpWidget(wrapWithSources());
      await tester.pump();

      expect(attachButton(), findsOneWidget);
    });

    /// The server injects the path into the *agent* pane, so on a shell attach
    /// the action would type somewhere the user isn't looking.
    testWidgets('a shell attach hides the attach-image action', (tester) async {
      await tester.pumpWidget(wrapWithSources(kind: AttachKind.shell));
      await tester.pump();

      expect(attachButton(), findsNothing);
    });

    testWidgets('picking from the library uploads the picked bytes', (
      tester,
    ) async {
      picker.file = FakeImagePicker.fileOf(tinyPng);
      await tester.pumpWidget(wrapWithSources());
      await tester.pump();

      await tester.tap(attachButton());
      await tester.pumpAndSettle();
      await tester.tap(find.text('Choose file'));
      await tester.pumpAndSettle();

      expect(picker.picked, [ImagePickSource.gallery]);
      expect(api.lastPastedImage, tinyPng);
      expect(api.lastCall('pasteImage')!.args['id'], sessionInfo().id);
    });

    testWidgets('a cancelled pick uploads nothing', (tester) async {
      picker.file = null; // user backed out of the picker
      await tester.pumpWidget(wrapWithSources());
      await tester.pump();

      await tester.tap(attachButton());
      await tester.pumpAndSettle();
      await tester.tap(find.text('Choose file'));
      await tester.pumpAndSettle();

      expect(api.lastCall('pasteImage'), isNull);
    });

    /// Refused from the file's reported length, so a huge phone photo is never
    /// read into memory or sent.
    testWidgets('an oversized pick is refused before reading or uploading', (
      tester,
    ) async {
      api.imageMaxBytesResponse = 10 * 1024 * 1024;
      picker.file = FakeImagePicker.fileOf(
        tinyPng,
        reportedLength: 20 * 1024 * 1024,
      );
      await tester.pumpWidget(wrapWithSources());
      await tester.pump();

      await tester.tap(attachButton());
      await tester.pumpAndSettle();
      await tester.tap(find.text('Choose file'));
      await tester.pumpAndSettle();

      expect(api.lastCall('pasteImage'), isNull);
      expect(find.textContaining('the limit is'), findsOneWidget);
    });

    testWidgets('camera is offered only where the platform supports it', (
      tester,
    ) async {
      picker = FakeImagePicker(supportsCamera: true);
      await tester.pumpWidget(wrapWithSources());
      await tester.pump();

      await tester.tap(attachButton());
      await tester.pumpAndSettle();

      expect(find.text('Take photo'), findsOneWidget);
      expect(find.text('Photo library'), findsOneWidget);
      expect(find.text('Choose file'), findsNothing);
    });

    testWidgets('the clipboard option uploads a clipboard image', (
      tester,
    ) async {
      clipboard.image = tinyPng;
      await tester.pumpWidget(wrapWithSources());
      await tester.pump();

      await tester.tap(attachButton());
      await tester.pumpAndSettle();
      await tester.tap(find.text('Paste from clipboard'));
      await tester.pumpAndSettle();

      expect(api.lastPastedImage, tinyPng);
    });

    testWidgets('the clipboard option reports an empty clipboard', (
      tester,
    ) async {
      clipboard.image = null;
      await tester.pumpWidget(wrapWithSources());
      await tester.pump();

      await tester.tap(attachButton());
      await tester.pumpAndSettle();
      await tester.tap(find.text('Paste from clipboard'));
      await tester.pumpAndSettle();

      expect(api.lastCall('pasteImage'), isNull);
      expect(find.text('No image on the clipboard'), findsOneWidget);
    });

    testWidgets('a failed upload surfaces the error', (tester) async {
      clipboard.image = tinyPng;
      api.pasteImageError = Exception('not a recognised image');
      await tester.pumpWidget(wrapWithSources());
      await tester.pump();

      await tester.tap(attachButton());
      await tester.pumpAndSettle();
      await tester.tap(find.text('Paste from clipboard'));
      await tester.pumpAndSettle();

      expect(find.textContaining('not a recognised image'), findsOneWidget);
    });

    group('Ctrl+V', () {
      Future<void> pressCtrlV(WidgetTester tester) async {
        await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
        await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
        await tester.pumpAndSettle();
      }

      testWidgets('uploads a clipboard image instead of pasting text', (
        tester,
      ) async {
        clipboard.image = tinyPng;
        await tester.pumpWidget(wrapWithSources());
        await tester.pump();

        await pressCtrlV(tester);

        expect(api.lastPastedImage, tinyPng);
        // The image replaced the paste — no text was typed into the pane.
        expect(api.lastCall('terminalSendInput'), isNull);
      });

      /// With no clipboard image, Ctrl+V must still behave as xterm's own
      /// binding would — our `onKeyEvent` pre-empts it, so the fallback is ours
      /// to reproduce.
      testWidgets(
        'falls back to a text paste when the clipboard has no image',
        (tester) async {
          clipboard.image = null;
          const text = 'pasted text';
          tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
            SystemChannels.platform,
            (call) async => call.method == 'Clipboard.getData'
                ? <String, dynamic>{'text': text}
                : null,
          );
          addTearDown(
            () => tester.binding.defaultBinaryMessenger
                .setMockMethodCallHandler(SystemChannels.platform, null),
          );
          await tester.pumpWidget(wrapWithSources());
          await tester.pump();

          await pressCtrlV(tester);

          expect(api.lastCall('pasteImage'), isNull);
          expect(clipboard.readCount, 1);
          final sent = api.lastCall('terminalSendInput');
          expect(sent, isNotNull);
          expect(utf8.decode(sent!.args['bytes'] as List<int>), text);
        },
      );

      /// A shell attach has no image path, so Ctrl+V must be left entirely to
      /// xterm — we must not even read the clipboard.
      testWidgets('is not intercepted on a shell attach', (tester) async {
        clipboard.image = tinyPng;
        await tester.pumpWidget(wrapWithSources(kind: AttachKind.shell));
        await tester.pump();

        await pressCtrlV(tester);

        expect(clipboard.readCount, 0);
        expect(api.lastCall('pasteImage'), isNull);
      });

      /// Regression: `KeyRepeatEvent` is a sibling of `KeyDownEvent`, not a
      /// subclass, so matching only `KeyDownEvent` let every auto-repeat fall
      /// through to xterm — whose Ctrl+V activator defaults to
      /// `includeRepeats: true` — pasting clipboard *text* into the pane while
      /// our image upload was still in flight.
      testWidgets('a held key does not leak a text paste to xterm', (
        tester,
      ) async {
        clipboard.image = tinyPng;
        const text = 'should never be pasted';
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          (call) async => call.method == 'Clipboard.getData'
              ? <String, dynamic>{'text': text}
              : null,
        );
        addTearDown(
          () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
            SystemChannels.platform,
            null,
          ),
        );
        await tester.pumpWidget(wrapWithSources());
        await tester.pump();

        await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
        await tester.sendKeyDownEvent(LogicalKeyboardKey.keyV);
        await tester.sendKeyRepeatEvent(LogicalKeyboardKey.keyV);
        await tester.sendKeyRepeatEvent(LogicalKeyboardKey.keyV);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.keyV);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
        await tester.pumpAndSettle();

        expect(api.lastPastedImage, tinyPng);
        expect(
          api.lastCall('terminalSendInput'),
          isNull,
          reason: 'repeats must be swallowed, not passed to xterm',
        );
      });

      /// Regression: the in-flight flag used to be set only once the upload
      /// began, so a second press arriving during the clipboard read (a platform
      /// round trip) started a second upload and injected the path twice.
      testWidgets('two fast presses upload once', (tester) async {
        clipboard.image = tinyPng;
        // Hold the clipboard read open so the second press genuinely lands
        // mid-read, which is the window the real bug lived in.
        final gate = Completer<void>();
        clipboard.gate = gate;
        await tester.pumpWidget(wrapWithSources());
        await tester.pump();

        await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
        await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
        await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
        gate.complete();
        await tester.pumpAndSettle();

        expect(
          clipboard.readCount,
          1,
          reason: 'the second press must not even start a clipboard read',
        );
        expect(
          api.calls.where((c) => c.method == 'pasteImage').length,
          1,
          reason: 'a second press during the clipboard read must be dropped',
        );
      });

      /// Ctrl+V with Meta held previously reached the PTY as 0x16; intercepting
      /// it would silently steal that.
      testWidgets('is not intercepted when Meta is also held', (tester) async {
        clipboard.image = tinyPng;
        await tester.pumpWidget(wrapWithSources());
        await tester.pump();

        await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
        await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
        await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
        await tester.pumpAndSettle();

        expect(clipboard.readCount, 0);
        expect(api.lastCall('pasteImage'), isNull);
      });

      /// After the attach ends the view is frozen, so an upload would inject
      /// into a pane the user cannot see.
      testWidgets('is not intercepted after the attach has ended', (
        tester,
      ) async {
        clipboard.image = tinyPng;
        await tester.pumpWidget(wrapWithSources());
        await tester.pump();
        await emitAndPump(
          tester,
          signal(TerminalEventKind.detached, 'session ended'),
        );

        await pressCtrlV(tester);

        expect(clipboard.readCount, 0);
        expect(api.lastCall('pasteImage'), isNull);
      });
    });
  });
}

/// An emulator whose parser always fails. Stands in for a defect in the pinned
/// `xterm` fork: the real one has no known sequence that throws (the one it did
/// have — `CSI 1 K` at column 0 — is fixed and covered above), and a test for
/// the failure path cannot wait for the next one.
class _ThrowingTerminal extends Terminal {
  @override
  void write(String data) => throw StateError('parser failure');
}
