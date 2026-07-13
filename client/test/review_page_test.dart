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
    home: ReviewPage(api: api, handle: testHandle, session: sessionInfo()),
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

  testWidgets(
    'a mixed deletion+addition selection comments on the new side with '
    'only new-side snippet content',
    (tester) async {
      api.openReviewResponse = reviewSnapshot(
        files: [
          reviewFile(
            displayPath: 'lib/foo.dart',
            hunks: [
              hunk(
                lines: [
                  line(
                    ReviewLineOrigin.deletion,
                    'old gone line',
                    oldLineno: 4,
                  ),
                  line(
                    ReviewLineOrigin.addition,
                    'fresh new line',
                    newLineno: 4,
                  ),
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

      // Select the deletion row, then extend to the addition row: the selection
      // spans both a deletion and an addition (a "mixed" selection).
      InkWell rowOf(String content) => tester.widget<InkWell>(
        find.ancestor(of: find.text(content), matching: find.byType(InkWell)),
      );
      rowOf('old gone line').onTap!();
      await tester.pumpAndSettle();
      rowOf('fresh new line').onLongPress!();
      await tester.pumpAndSettle();

      await tester.tap(find.text('Comment on selection'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), 'mixed comment');
      await tester.tap(find.widgetWithText(FilledButton, 'Add'));
      await tester.pumpAndSettle();

      expect(api.countOf('createComment'), 1);
      final call = api.lastCall('createComment')!;
      expect(call.args['side'], 'new');
      // The snippet must contain only the new-side line, not the deletion text —
      // otherwise the server's reanchor (which searches the chosen side only)
      // drifts the comment the moment it's created.
      expect(call.args['snippet'], 'fresh new line');
      expect(call.args['lineStart'], 4);
      expect(call.args['lineEnd'], 4);
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

    await tester.tap(find.widgetWithText(FilledButton, 'Apply (1)'));
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

    await tester.tap(find.widgetWithText(FilledButton, 'Apply (1)'));
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
        home: ReviewPage(api: gate, handle: testHandle, session: sessionInfo()),
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

  testWidgets('adding a comment does not setState after the hunk is disposed', (
    tester,
  ) async {
    final gate = GatedCommentApi();
    gate.openReviewResponse = reviewSnapshot(
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
    await tester.pumpWidget(
      MaterialApp(
        home: ReviewPage(api: gate, handle: testHandle, session: sessionInfo()),
      ),
    );
    await tester.pumpAndSettle(); // first openReview resolves normally

    await tester.tap(find.text('lib/foo.dart'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('target line'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Comment on selection'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'boom');
    await tester.tap(find.widgetWithText(FilledButton, 'Add'));
    await tester.pumpAndSettle(); // dialog closes; createComment is in flight

    // Release createComment; the page's _addComment then calls _open(), whose
    // setState(_loading=true) swaps the diff list for a spinner and DISPOSES
    // this hunk's State while its re-open network call is still pending.
    gate.releaseCreate();
    await tester.pump(); // _open sets loading
    await tester.pump(); // rebuild → hunk subtree disposed

    // Release the re-open: _addComment returns and the hunk's _clear() runs
    // against the now-disposed State — which must not throw.
    gate.releaseReopen();
    await tester.pumpAndSettle();

    expect(tester.takeException(), isNull);
  });

  testWidgets('a snapshot refresh resets a now-out-of-range line selection', (
    tester,
  ) async {
    api.openReviewResponse = reviewSnapshot(
      contentHash: 'A',
      files: [
        reviewFile(
          displayPath: 'lib/foo.dart',
          hunks: [
            hunk(
              lines: [
                line(
                  ReviewLineOrigin.context,
                  'line one',
                  oldLineno: 1,
                  newLineno: 1,
                ),
                line(
                  ReviewLineOrigin.context,
                  'line two',
                  oldLineno: 2,
                  newLineno: 2,
                ),
                line(ReviewLineOrigin.addition, 'line three', newLineno: 3),
              ],
            ),
          ],
        ),
      ],
    );
    api.refreshReviewResponse = reviewSnapshot(
      contentHash: 'B',
      files: [
        reviewFile(
          displayPath: 'lib/foo.dart',
          hunks: [
            hunk(
              lines: [
                line(ReviewLineOrigin.addition, 'only line', newLineno: 1),
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

    // Select the last line of the 3-line hunk (index 2).
    await tester.tap(find.text('line three'));
    await tester.pumpAndSettle();
    expect(find.text('Comment on selection'), findsOneWidget);

    // Refresh to a snapshot whose hunk has a single line: the reused hunk State
    // must drop its stale index-2 selection rather than range-error on it.
    await tester.tap(find.widgetWithIcon(IconButton, Icons.refresh));
    await tester.pumpAndSettle();

    expect(tester.takeException(), isNull);
    expect(find.text('only line'), findsOneWidget);
    expect(find.text('Comment on selection'), findsNothing);
  });
}

/// A [FakeCommanderApi] that holds `createComment` and the *second* `openReview`
/// (the re-open triggered after staging a comment) behind completers, so a test
/// can interleave the disposal of the hunk subtree with the in-flight re-open.
class GatedCommentApi extends FakeCommanderApi {
  final Completer<void> _createGate = Completer<void>();
  final Completer<void> _reopenGate = Completer<void>();
  int _openCount = 0;

  void releaseCreate() => _createGate.complete();
  void releaseReopen() => _reopenGate.complete();

  @override
  Future<ReviewSnapshotDto> openReview({
    required String handle,
    required String sessionId,
  }) async {
    calls.add(RecordedCall('openReview', {'sessionId': sessionId}));
    _openCount++;
    if (_openCount > 1) await _reopenGate.future;
    final snap = openReviewResponse;
    if (snap == null) {
      throw StateError('openReviewResponse not set on GatedCommentApi');
    }
    return snap;
  }

  @override
  Future<String> createComment({
    required String handle,
    required String sessionId,
    required String file,
    required String side,
    required int lineStart,
    required int lineEnd,
    required String snippet,
    required String comment,
  }) async {
    calls.add(
      RecordedCall('createComment', {
        'file': file,
        'side': side,
        'lineStart': lineStart,
        'lineEnd': lineEnd,
        'snippet': snippet,
        'comment': comment,
      }),
    );
    await _createGate.future;
    return createCommentResponse;
  }
}

/// A [FakeCommanderApi] whose `toggleFileReviewed` blocks until released, so a
/// test can hold the flip in flight and exercise the `_toggling` guard.
class GatedReviewApi extends FakeCommanderApi {
  final Completer<bool> _gate = Completer<bool>();

  void releaseToggle(bool result) => _gate.complete(result);

  @override
  Future<bool> toggleFileReviewed({
    required String handle,
    required String sessionId,
    required String displayPath,
  }) {
    calls.add(RecordedCall('toggleFileReviewed', {'displayPath': displayPath}));
    return _gate.future;
  }
}
