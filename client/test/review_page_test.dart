import 'dart:async';

import 'package:claude_commander_client/pages/review_page.dart';
import 'package:claude_commander_client/src/rust/api/review.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_commander_api.dart';
import 'support/fixtures.dart';

void main() {
  late FakeCommanderApi api;

  setUp(() => api = FakeCommanderApi());

  Widget wrap() => MaterialApp(
    home: ReviewPage(api: api, config: testConfig, session: sessionInfo()),
  );

  testWidgets('a snapshot renders file cards and diff rows', (tester) async {
    api.openReviewResponse = reviewSnapshot(
      files: [
        reviewFile(
          displayPath: 'lib/foo.dart',
          hunks: [
            hunk(
              lines: [
                line(
                  ReviewLineOrigin.context,
                  'unchanged here',
                  oldLineno: 1,
                  newLineno: 1,
                ),
                line(ReviewLineOrigin.addition, 'brand new line', newLineno: 2),
              ],
            ),
          ],
        ),
      ],
    );
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.text('lib/foo.dart'), findsOneWidget);
    // The card starts collapsed; expand it to reveal the diff rows.
    await tester.tap(find.text('lib/foo.dart'));
    await tester.pumpAndSettle();
    expect(find.text('unchanged here'), findsOneWidget);
    expect(find.text('brand new line'), findsOneWidget);
  });

  testWidgets(
    'selecting a range then confirming the dialog calls createComment',
    (tester) async {
      api.openReviewResponse = reviewSnapshot(
        files: [
          reviewFile(
            displayPath: 'lib/foo.dart',
            hunks: [
              hunk(
                lines: [
                  line(ReviewLineOrigin.addition, 'target line', newLineno: 5),
                ],
              ),
            ],
          ),
        ],
      );
      await tester.pumpWidget(wrap());
      await tester.pumpAndSettle();
      await tester.tap(find.text('lib/foo.dart'));
      await tester.pumpAndSettle();

      // Tap the diff line to select it, then the "Comment on selection" action.
      await tester.tap(find.text('target line'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Comment on selection'));
      await tester.pumpAndSettle();

      // The comment dialog is up; type and confirm.
      expect(find.byType(AlertDialog), findsOneWidget);
      await tester.enterText(find.byType(TextField), 'needs a test');
      await tester.tap(find.widgetWithText(FilledButton, 'Add'));
      await tester.pumpAndSettle();

      expect(api.countOf('createComment'), 1);
      final call = api.lastCall('createComment')!;
      expect(call.args['comment'], 'needs a test');
      expect(call.args['side'], 'new');
      expect(call.args['lineStart'], 5);
    },
  );

  testWidgets('delete calls deleteComment', (tester) async {
    api.openReviewResponse = reviewSnapshot(
      comments: [comment(id: 'c-9', text: 'delete me')],
    );
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    expect(find.text('delete me'), findsOneWidget);
    await tester.tap(find.widgetWithIcon(IconButton, Icons.delete_outline));
    await tester.pumpAndSettle();

    expect(api.countOf('deleteComment'), 1);
    expect(api.lastCall('deleteComment')!.args['commentId'], 'c-9');
  });

  testWidgets('a blocked apply result renders differently from applied', (
    tester,
  ) async {
    api.openReviewResponse = reviewSnapshot(
      comments: [comment(status: ReviewCommentStatus.staged)],
    );
    api.applyCommentsResponse = const ApplyResult(
      kind: ApplyResultKind.blocked,
      driftedIds: ['c-1', 'c-2'],
      count: 0,
    );
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.widgetWithText(FloatingActionButton, 'Apply (1)'));
    await tester.pumpAndSettle();

    // Blocked → the drift count is surfaced, not an "Applied N" message.
    expect(find.textContaining('Blocked: 2 drifted'), findsOneWidget);
    expect(find.textContaining('Applied'), findsNothing);
  });

  testWidgets('an applied apply result surfaces the applied count', (
    tester,
  ) async {
    api.openReviewResponse = reviewSnapshot(
      comments: [comment(status: ReviewCommentStatus.staged)],
    );
    api.applyCommentsResponse = const ApplyResult(
      kind: ApplyResultKind.applied,
      driftedIds: [],
      count: 3,
    );
    await tester.pumpWidget(wrap());
    await tester.pumpAndSettle();

    await tester.tap(find.widgetWithText(FloatingActionButton, 'Apply (1)'));
    await tester.pumpAndSettle();

    expect(find.textContaining('Applied 3 comment'), findsOneWidget);
  });

  testWidgets('the reviewed checkbox is optimistic and guarded against '
      'double-tap', (tester) async {
    // Gate toggleFileReviewed so the second tap arrives while the first is in
    // flight; the _toggling guard must swallow it.
    final gate = GatedReviewApi();
    gate.openReviewResponse = reviewSnapshot(
      files: [reviewFile(displayPath: 'lib/foo.dart')],
    );
    await tester.pumpWidget(
      MaterialApp(
        home: ReviewPage(api: gate, config: testConfig, session: sessionInfo()),
      ),
    );
    await tester.pumpAndSettle();

    final checkbox = find.byType(Checkbox);
    await tester.tap(checkbox);
    await tester.pump();
    // Second rapid tap while the first flip is in flight.
    await tester.tap(checkbox);
    await tester.pump();

    expect(gate.countOf('toggleFileReviewed'), 1);

    // Complete the flip; the optimistic set flips the checkbox to true.
    gate.releaseToggle(true);
    await tester.pumpAndSettle();
    final cb = tester.widget<Checkbox>(checkbox);
    expect(cb.value, isTrue);
  });
}

/// A [FakeCommanderApi] whose `toggleFileReviewed` blocks until released, so a
/// test can hold the flip in flight and exercise the `_toggling` guard.
class GatedReviewApi extends FakeCommanderApi {
  final Completer<bool> _gate = Completer<bool>();

  void releaseToggle(bool result) => _gate.complete(result);

  @override
  Future<bool> toggleFileReviewed({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String displayPath,
  }) {
    calls.add(RecordedCall('toggleFileReviewed', {'displayPath': displayPath}));
    return _gate.future;
  }
}
