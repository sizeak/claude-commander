import 'dart:async';

import 'package:claude_commander_client/pages/session_detail_page.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;

  setUp(() => api = FakeCommanderApi());

  Widget wrap(SessionInfo session) => MaterialApp(
    home: SessionDetailPage(api: api, config: testConfig, session: session),
  );

  // The page polls on a periodic Timer, so pumpAndSettle never quiets. Pump a
  // couple of frames to let the first poll's Future resolve, then stop.
  Future<void> settleFirstPoll(WidgetTester tester) async {
    await tester.pump(); // build
    await tester.pump(const Duration(milliseconds: 10)); // first poll resolves
  }

  testWidgets('renders detail from the fake', (tester) async {
    api.getSessionDetailResponse = sessionDetail(
      info: sessionInfo(title: 'Detail me', status: SessionStatus.running),
      diffStat: '3 files changed',
      paneContent: 'hello world',
    );
    await tester.pumpWidget(wrap(sessionInfo(title: 'Detail me')));
    await settleFirstPoll(tester);

    expect(find.text('Detail me'), findsWidgets);
    expect(find.text('3 files changed'), findsOneWidget);
    expect(find.text('hello world'), findsOneWidget);
  });

  testWidgets('a deleted session shows a gone state and stops polling', (
    tester,
  ) async {
    // getSessionDetail returns null (404 → session gone by design).
    api.getSessionDetailResponse = null;
    await tester.pumpWidget(wrap(sessionInfo(title: 'Gone one')));
    await settleFirstPoll(tester);

    expect(find.textContaining('no longer exists'), findsOneWidget);
    // The lifecycle actions are gone (or disabled) — no live controls.
    expect(find.widgetWithText(FilledButton, 'Kill'), findsNothing);
    expect(find.widgetWithText(FilledButton, 'Restart'), findsNothing);
    expect(find.widgetWithText(FilledButton, 'Delete'), findsNothing);

    // The poll timer is cancelled: no further getSessionDetail calls fire.
    final callsSoFar = api.countOf('getSessionDetail');
    await tester.pump(const Duration(seconds: 5));
    expect(api.countOf('getSessionDetail'), callsSoFar);
  });

  Future<void> confirmAction(
    WidgetTester tester, {
    required String button,
    required String confirmLabel,
  }) async {
    await tester.tap(find.widgetWithText(FilledButton, button));
    await tester.pump();
    // The confirm dialog is up.
    expect(find.byType(AlertDialog), findsOneWidget);
    // The dialog's confirm button sits above the page's action button of the
    // same label, so target the dialog one via the AlertDialog subtree.
    await tester.tap(
      find.descendant(
        of: find.byType(AlertDialog),
        matching: find.widgetWithText(FilledButton, confirmLabel),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));
  }

  testWidgets('kill opens a confirm dialog then calls killSession', (
    tester,
  ) async {
    final info = sessionInfo(status: SessionStatus.running);
    api.getSessionDetailResponse = sessionDetail(info: info);
    await tester.pumpWidget(wrap(info));
    await settleFirstPoll(tester);

    await confirmAction(tester, button: 'Kill', confirmLabel: 'Kill');
    expect(api.countOf('killSession'), 1);
  });

  testWidgets('restart opens a confirm dialog then calls restartSession', (
    tester,
  ) async {
    final info = sessionInfo(status: SessionStatus.running);
    api.getSessionDetailResponse = sessionDetail(info: info);
    await tester.pumpWidget(wrap(info));
    await settleFirstPoll(tester);

    await confirmAction(tester, button: 'Restart', confirmLabel: 'Restart');
    expect(api.countOf('restartSession'), 1);
  });

  testWidgets('delete confirms, calls deleteSession, and pops', (tester) async {
    final info = sessionInfo(status: SessionStatus.running);
    api.getSessionDetailResponse = sessionDetail(info: info);
    // Push the detail page as a second route (over a placeholder home) so the
    // delete-pop has an underlying route to return to.
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () => Navigator.of(context).push(
                  MaterialPageRoute(
                    builder: (_) => SessionDetailPage(
                      api: api,
                      config: testConfig,
                      session: info,
                    ),
                  ),
                ),
                child: const Text('open'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open'));
    await settleFirstPoll(tester);

    await confirmAction(tester, button: 'Delete', confirmLabel: 'Delete');
    expect(api.countOf('deleteSession'), 1);
    // popOnSuccess: true → the page is gone, back to the placeholder home.
    await tester.pumpAndSettle();
    expect(find.byType(SessionDetailPage), findsNothing);
    expect(find.text('open'), findsOneWidget);
  });

  testWidgets('the busy guard disables the actions while one is in flight', (
    tester,
  ) async {
    // Gate killSession on a completer so the action stays in flight and _busy
    // stays set while we inspect the buttons.
    final gate = GatedCommanderApi();
    gate.getSessionDetailResponse = sessionDetail(
      info: sessionInfo(status: SessionStatus.running),
    );
    final info = sessionInfo(status: SessionStatus.running);
    await tester.pumpWidget(
      MaterialApp(
        home: SessionDetailPage(api: gate, config: testConfig, session: info),
      ),
    );
    await settleFirstPoll(tester);

    // Confirm kill; the completer is not yet complete, so the action hangs.
    await tester.tap(find.widgetWithText(FilledButton, 'Kill'));
    await tester.pump();
    await tester.tap(
      find.descendant(
        of: find.byType(AlertDialog),
        matching: find.widgetWithText(FilledButton, 'Kill'),
      ),
    );
    await tester.pump();

    // _busy is set → Restart/Delete are disabled (onPressed null).
    final restart = tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, 'Restart'),
    );
    final delete = tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, 'Delete'),
    );
    expect(restart.onPressed, isNull);
    expect(delete.onPressed, isNull);
    expect(gate.countOf('killSession'), 1);

    // Release the in-flight call so the widget can settle cleanly.
    gate.releaseKill();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));
  });
}

/// A [FakeCommanderApi] whose `killSession` blocks on a completer, so a test can
/// hold the action in flight and observe the `_busy` gate.
class GatedCommanderApi extends FakeCommanderApi {
  final _killGate = Completer<void>();

  void releaseKill() => _killGate.complete();

  @override
  Future<void> killSession({
    required String baseUrl,
    required String token,
    required String id,
  }) async {
    calls.add(RecordedCall('killSession', {'id': id}));
    await _killGate.future;
  }
}
