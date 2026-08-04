// Full-stack e2e: drives the REAL app (RustCommanderApi over the frb bridge)
// against a REAL, hermetic claude-commander-server, on the Linux desktop target.
// Launch via client/tool/e2e.sh, which boots the server over throwaway XDG state
// and passes its address/token/repo through --dart-define. Running this file with
// a plain `flutter test` (no server) fails at the connect step by design.
//
// One continuous journey (not several tests): the detail page polls on a 2s
// Timer and the terminal streams events, so the integration-test binding can
// otherwise trip its between-test frame/`inTest` assertions on desktop. A single
// test that ends on the timer-free session list keeps the teardown clean while
// still covering every happy path in sequence.
//
// Pumping idiom: `pumpAndSettle` never settles on the polling/streaming pages, so
// every wait uses `pumpUntil`, which pumps frames until a condition holds or a
// real-time deadline passes (real time advances because network + PTY I/O
// complete between pumps). Terminal output is read from the xterm `Terminal`
// buffer via the public `TerminalView.terminal` field (glyphs are canvas-painted,
// so `find.text` can't see them); diff rows are ordinary `Text` widgets.

import 'package:claude_commander_client/main.dart';
import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/services/commander_api.dart';
import 'package:claude_commander_client/src/rust/frb_generated.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:xterm/xterm.dart';

const _baseUrl = String.fromEnvironment('CC_E2E_BASE_URL');
const _token = String.fromEnvironment('CC_E2E_TOKEN');
const _repo = String.fromEnvironment('CC_E2E_REPO');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(() async {
    await RustLib.init();
    expect(
      _baseUrl.isNotEmpty && _token.isNotEmpty && _repo.isNotEmpty,
      isTrue,
      reason:
          'CC_E2E_BASE_URL / CC_E2E_TOKEN / CC_E2E_REPO must be passed via '
          '--dart-define (run through client/tool/e2e.sh)',
    );
  });

  // Pump frames until [cond] holds or [timeout] (real time) elapses.
  Future<void> pumpUntil(
    WidgetTester tester,
    bool Function() cond, {
    Duration timeout = const Duration(seconds: 25),
    String reason = 'condition',
  }) async {
    final deadline = DateTime.now().add(timeout);
    while (DateTime.now().isBefore(deadline)) {
      if (cond()) return;
      await tester.pump(const Duration(milliseconds: 120));
    }
    if (!cond()) {
      final visible = tester
          .widgetList<Text>(find.byType(Text))
          .map((t) => t.data)
          .where((s) => s != null)
          .toList();
      throw TestFailure('pumpUntil timed out: $reason\nvisible text: $visible');
    }
  }

  Future<void> waitFor(
    WidgetTester tester,
    Finder finder, {
    Duration timeout = const Duration(seconds: 25),
  }) => pumpUntil(
    tester,
    () => finder.evaluate().isNotEmpty,
    timeout: timeout,
    reason: 'finding $finder',
  );

  String terminalText(WidgetTester tester) {
    final term = tester.widget<TerminalView>(find.byType(TerminalView)).terminal;
    return [
      for (var i = 0; i < term.buffer.lines.length; i++)
        term.buffer.lines[i].getText(),
    ].join('\n');
  }

  void typeInTerminal(WidgetTester tester, String text) {
    tester
        .widget<TerminalView>(find.byType(TerminalView))
        .terminal
        .textInput(text);
  }

  // Pop the current route deterministically. `tester.pageBack()` finds the
  // "Back" button by tooltip, which is ambiguous when two routes' app bars are
  // briefly onstage during a transition; popping the top Scaffold's Navigator
  // avoids that.
  Future<void> goBack(WidgetTester tester) async {
    Navigator.of(tester.element(find.byType(Scaffold).last)).pop();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));
  }

  // Tap a detail-page lifecycle action (by icon) then confirm its dialog.
  Future<void> confirmAction(
    WidgetTester tester,
    IconData actionIcon,
    String confirmLabel,
  ) async {
    await tester.tap(find.widgetWithIcon(FilledButton, actionIcon));
    await waitFor(tester, find.byType(AlertDialog));
    await tester.tap(
      find.descendant(
        of: find.byType(AlertDialog),
        matching: find.widgetWithText(FilledButton, confirmLabel),
      ),
    );
    await pumpUntil(
      tester,
      () => find.byType(AlertDialog).evaluate().isEmpty,
      reason: 'dialog "$confirmLabel" dismissed',
    );
  }

  testWidgets('full journey: connect, create, terminal + rejoin, review, '
      'lifecycle', (tester) async {
    // ---- connect (with auth) ----
    await tester.pumpWidget(
      CommanderApp(
        api: const RustCommanderApi(),
        workspace: WorkspaceStore(
          api: const RustCommanderApi(),
          listStore: InMemoryServerListStore(),
        ),
      ),
    );
    await waitFor(tester, find.text('Add server'));
    // Focus each field before entering text: under headless xvfb the field
    // isn't auto-focused as on a real display, so a bare enterText can no-op and
    // leave the prefilled default URL. Tap → enterText → pump makes it stick.
    // Fields, in order: Name (0), Server URL (1), Bearer token (2).
    final urlField = find.byType(TextFormField).at(1);
    final tokenField = find.byType(TextFormField).at(2);
    await tester.tap(urlField);
    await tester.pump();
    await tester.enterText(urlField, _baseUrl);
    await tester.pump();
    await tester.tap(tokenField);
    await tester.pump();
    await tester.enterText(tokenField, _token);
    await tester.pump();
    // Guard: the URL must actually be the e2e server before we connect.
    expect(
      tester.widget<TextField>(find.descendant(
        of: urlField,
        matching: find.byType(TextField),
      )).controller?.text,
      _baseUrl,
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Add server'));
    await waitFor(tester, find.text('Sessions'));
    await waitFor(tester, find.text('No sessions')); // fresh hermetic server

    // ---- create a bash session ----
    await tester.tap(find.byIcon(Icons.add));
    await waitFor(tester, find.text('New session'));
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Project path (on the server)'),
      _repo,
    );
    await tester.enterText(find.widgetWithText(TextFormField, 'Title'), 'e2e-journey');
    await tester.enterText(
      find.widgetWithText(TextFormField, 'Program (optional)'),
      'bash',
    );
    await tester.tap(find.widgetWithText(FilledButton, 'Create session'));
    // Wait for the actual list tile to render — not the create page's Title
    // field, whose 'e2e-journey' text transiently matches during the pop
    // transition. The list has exactly one ListTile; the create page has none.
    await waitFor(tester, find.byType(ListTile));
    expect(find.widgetWithText(ListTile, 'e2e-journey'), findsOneWidget);

    // ---- open detail ---- (tap the ListTile itself, not its title Text, so the
    // tile's InkWell reliably receives the gesture)
    await tester.tap(find.byType(ListTile));
    await tester.pump();
    await waitFor(tester, find.byIcon(Icons.rate_review));

    // ---- terminal: do work + write a file for the later review ----
    await tester.tap(find.byIcon(Icons.terminal).first);
    await waitFor(tester, find.byType(TerminalView));
    await pumpUntil(
      tester,
      () => find.textContaining('attached:').evaluate().isNotEmpty,
      reason: 'terminal attached',
    );
    typeInTerminal(tester, 'echo cc_e2e_marker\n');
    await pumpUntil(
      tester,
      () => terminalText(tester).contains('cc_e2e_marker'),
      reason: 'PTY echoes the marker',
    );
    typeInTerminal(tester, "printf 'hello e2e\\n' > cc_e2e_file.txt\n");
    typeInTerminal(tester, 'echo cc_wrote_file\n');
    await pumpUntil(
      tester,
      () => terminalText(tester).contains('cc_wrote_file'),
      reason: 'file-write command completes',
    );

    // ---- re-join: leave and re-attach; the pane replays prior output ----
    await goBack(tester);
    await waitFor(tester, find.byIcon(Icons.terminal));
    await tester.tap(find.byIcon(Icons.terminal).first);
    await waitFor(tester, find.byType(TerminalView));
    await pumpUntil(
      tester,
      () => terminalText(tester).contains('cc_e2e_marker'),
      reason: 're-attach replays prior output (join existing session)',
    );
    await goBack(tester);
    await waitFor(tester, find.byIcon(Icons.rate_review));

    // ---- review: the diff of the file written over the terminal renders, and
    // marking the file reviewed round-trips to the server. Comment create + apply
    // are covered deterministically elsewhere — by the L2 cdylib↔server test
    // `review_round_trip` (real server create_comment/apply_comments/
    // toggle_file_reviewed) and the L3 review widget test (real UI line-selection
    // → createComment/delete/apply). Driving the thin diff row's line-selection
    // gesture is unreliable under the live desktop test binding, so it's left to
    // those layers.
    await tester.tap(find.byIcon(Icons.rate_review));
    await waitFor(tester, find.text('cc_e2e_file.txt'));
    await tester.tap(find.text('cc_e2e_file.txt')); // expand the file card
    await waitFor(tester, find.text('hello e2e')); // the added line renders

    // mark the file reviewed — a real toggle_file_reviewed round-trip
    await tester.tap(find.byType(Checkbox).first);
    await pumpUntil(
      tester,
      () => tester.widget<Checkbox>(find.byType(Checkbox).first).value == true,
      reason: 'file marked reviewed (server round-trip)',
    );
    await goBack(tester);
    await waitFor(tester, find.byIcon(Icons.rate_review)); // back on detail

    // ---- lifecycle: kill → restart → delete ----
    await confirmAction(tester, Icons.stop, 'Kill');
    await waitFor(tester, find.text('Session killed'));
    await confirmAction(tester, Icons.restart_alt, 'Restart');
    await waitFor(tester, find.text('Session restarted'));
    await confirmAction(tester, Icons.delete_outline, 'Delete');

    // delete pops back to the (timer-free) list; the session is gone.
    await waitFor(tester, find.text('Sessions'));
    await pumpUntil(
      tester,
      () => find.text('e2e-journey').evaluate().isEmpty,
      reason: 'deleted session disappears from the list',
    );
    await tester.pumpAndSettle();
  });
}
