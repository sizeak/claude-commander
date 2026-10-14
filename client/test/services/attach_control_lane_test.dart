import 'dart:async';

import 'package:claude_commander_client/services/commander_api.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('AttachControlLane', () {
    test(
      'delivers calls in issue order even when they settle out of order',
      () async {
        final lane = AttachControlLane();
        final started = <int>[];
        final completers = <int, Completer<void>>{};

        Future<void> op(int i) {
          started.add(i);
          return (completers[i] = Completer<void>()).future;
        }

        // Issue three calls back-to-back, as a burst of keystrokes or a keyboard
        // animation's worth of resizes would.
        final calls = [
          lane.run('attach', () => op(1)),
          lane.run('attach', () => op(2)),
          lane.run('attach', () => op(3)),
        ];

        // Only the first has been handed to the bridge; the rest are queued.
        await pumpEventQueue();
        expect(started, [1]);

        completers[1]!.complete();
        await pumpEventQueue();
        expect(started, [1, 2]);

        completers[2]!.complete();
        await pumpEventQueue();
        expect(started, [1, 2, 3]);

        completers[3]!.complete();
        await Future.wait(calls);
      },
    );

    test('a failed call does not wedge the lane', () async {
      final lane = AttachControlLane();
      final started = <int>[];

      final failed = lane.run('attach', () async {
        started.add(1);
        throw StateError('bridge went away');
      });
      final next = lane.run('attach', () async => started.add(2));

      await expectLater(failed, throwsStateError);
      await next;
      expect(started, [1, 2]);
    });

    test('each attach has its own queue', () async {
      final lane = AttachControlLane();
      final started = <String>[];
      final block = Completer<void>();

      final blocked = lane.run('a', () async {
        started.add('a');
        await block.future;
      });
      await lane.run('b', () async => started.add('b'));

      // 'b' ran without waiting on the stalled 'a'.
      expect(started, ['a', 'b']);
      block.complete();
      await blocked;
    });

    // A reconnect abandons its attach id without detaching it, so the lane has
    // to forget drained queues by itself or it grows for the app's lifetime.
    test('forgets a queue once it drains', () async {
      final lane = AttachControlLane();

      await lane.run('attach-1', () async {});
      expect(lane.pendingAttaches, 0);

      // Including when the call failed.
      await expectLater(
        lane.run('attach-2', () async => throw StateError('boom')),
        throwsStateError,
      );
      expect(lane.pendingAttaches, 0);

      // A queue with work still in flight is retained.
      final block = Completer<void>();
      final inFlight = lane.run('attach-3', () => block.future);
      expect(lane.pendingAttaches, 1);
      block.complete();
      await inFlight;
      expect(lane.pendingAttaches, 0);
    });

    test('a queue still being appended to is not dropped mid-drain', () async {
      final lane = AttachControlLane();
      final started = <int>[];
      final first = Completer<void>();

      final a = lane.run('attach', () {
        started.add(1);
        return first.future;
      });
      final b = lane.run('attach', () async => started.add(2));

      first.complete();
      await Future.wait([a, b]);

      // The second call still ran after the first, and the drained queue is gone.
      expect(started, [1, 2]);
      expect(lane.pendingAttaches, 0);
    });
  });
}
