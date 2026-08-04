import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/util/activity_feed.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:uuid/uuid.dart';

import '../support/fake_commander_api.dart';
import '../support/fixtures.dart';

/// Spin up a [CommanderStore] backed by a fake carrying [sessions] and their
/// [states], connected so its snapshot getters are populated. Registers its own
/// teardown. Mirrors the seam used by `commander_store_test.dart`.
Future<CommanderStore> connectedStore(
  List<SessionInfo> sessions, {
  Map<String, AgentState> states = const {},
  ServerConfig config = testConfig,
}) async {
  final api = FakeCommanderApi();
  api.listSessionsResponse = sessions;
  api.agentStatesResponse = AgentStatesSnapshotDto(
    states: [
      for (final e in states.entries)
        AgentStateEntryDto(
          sessionId: SessionId(field0: UuidValue.fromString(e.key)),
          state: e.value,
        ),
    ],
    commanderRunning: true,
  );
  final store = CommanderStore(api: api, config: config);
  addTearDown(store.dispose);
  await store.connect();
  return store;
}

/// A distinct UUID per small integer, so tests can mint several sessions.
String id(int n) =>
    '${n.toString().padLeft(8, '0')}-2222-3333-4444-555555555555';

void main() {
  test('empty stores yield an empty feed', () async {
    expect(buildActivityFeed(const []), isEmpty);
    final store = await connectedStore(const []);
    expect(buildActivityFeed([store]), isEmpty);
  });

  test('a waiting session ranks first as a needsYou event', () async {
    // The working session is more recently attached, yet the actionable waiting
    // session must still float to the top.
    final waiting = sessionInfo(
      id: id(1),
      title: 'Payment webhook retry',
      lastAttachedAt: DateTime.utc(2026, 1, 1, 9),
    );
    final working = sessionInfo(
      id: id(2),
      title: 'Fix auth bypass',
      lastAttachedAt: DateTime.utc(2026, 1, 1, 12),
    );
    final store = await connectedStore(
      [working, waiting],
      states: {id(1): AgentState.waitingForInput, id(2): AgentState.working},
    );

    final feed = buildActivityFeed([store]);
    expect(feed.first.kind, ActivityKind.needsYou);
    expect(feed.first.title, 'Payment webhook retry');
    expect(feed.first.actionable, isTrue);
    expect(feed.first.sessionId, id(1));
    expect(feed.map((e) => e.kind), contains(ActivityKind.working));
  });

  test('a cascade-paused session is an actionable paused event', () async {
    final paused = sessionInfo(
      id: id(1),
      title: 'Onboarding wizard',
      status: SessionStatus.cascadePaused,
    );
    final store = await connectedStore([paused]);

    final feed = buildActivityFeed([store]);
    expect(feed.single.kind, ActivityKind.paused);
    expect(feed.single.actionable, isTrue);
  });

  test('merged and ready PRs produce PR events', () async {
    final merged = SessionInfo(
      id: id(1),
      sessionId: SessionId(field0: UuidValue.fromString(id(1))),
      title: 'Refactor session store',
      branch: 'refactor',
      status: SessionStatus.running,
      program: 'claude',
      projectId: ProjectId(field0: UuidValue.fromString(id(1))),
      projectName: 'genio',
      prNumber: 42,
      prUrl: null,
      prState: PrState.merged,
      prDraft: false,
      prLabels: const [],
      reviewDecision: null,
      prReviewers: const [],
      createdAt: DateTime.utc(2026, 1, 1),
      unread: false,
      stackParentSessionId: null,
      prBaseBranch: null,
      prMerged: true,
      currentSection: null,
      sectionOverride: null,
      enteredSectionAt: null,
      lastAttachedAt: DateTime.utc(2026, 1, 1, 10),
      worktreePath: '/tmp/w',
      tmuxSessionName: 'cc-1',
      keepAlive: false,
    );
    final ready = SessionInfo(
      id: id(2),
      sessionId: SessionId(field0: UuidValue.fromString(id(2))),
      title: 'Add rate limiter',
      branch: 'rate',
      status: SessionStatus.running,
      program: 'claude',
      projectId: ProjectId(field0: UuidValue.fromString(id(2))),
      projectName: 'genio',
      prNumber: 7,
      prUrl: null,
      prState: PrState.open,
      prDraft: false,
      prLabels: const [],
      reviewDecision: ReviewDecision.reviewRequired,
      prReviewers: const [],
      createdAt: DateTime.utc(2026, 1, 1),
      unread: false,
      stackParentSessionId: null,
      prBaseBranch: null,
      prMerged: false,
      currentSection: null,
      sectionOverride: null,
      enteredSectionAt: null,
      lastAttachedAt: DateTime.utc(2026, 1, 1, 11),
      worktreePath: '/tmp/w',
      tmuxSessionName: 'cc-2',
      keepAlive: false,
    );
    final store = await connectedStore(
      [merged, ready],
      states: {id(1): AgentState.idle, id(2): AgentState.idle},
    );

    final kinds = buildActivityFeed([store]).map((e) => e.kind).toList();
    expect(kinds, contains(ActivityKind.prMerged));
    expect(kinds, contains(ActivityKind.prReady));

    final mergedEvent = buildActivityFeed([
      store,
    ]).firstWhere((e) => e.kind == ActivityKind.prMerged);
    expect(mergedEvent.description, 'PR #42 merged');
  });

  test('filters partition the feed', () async {
    final waiting = sessionInfo(id: id(1), title: 'waiting');
    final open = SessionInfo(
      id: id(2),
      sessionId: SessionId(field0: UuidValue.fromString(id(2))),
      title: 'has pr',
      branch: 'b',
      status: SessionStatus.running,
      program: 'claude',
      projectId: ProjectId(field0: UuidValue.fromString(id(2))),
      projectName: 'genio',
      prNumber: 9,
      prUrl: null,
      prState: PrState.open,
      prDraft: false,
      prLabels: const [],
      reviewDecision: ReviewDecision.reviewRequired,
      prReviewers: const [],
      createdAt: DateTime.utc(2026, 1, 1),
      unread: false,
      stackParentSessionId: null,
      prBaseBranch: null,
      prMerged: false,
      currentSection: null,
      sectionOverride: null,
      enteredSectionAt: null,
      lastAttachedAt: null,
      worktreePath: '/tmp/w',
      tmuxSessionName: 'cc-2',
      keepAlive: false,
    );
    final store = await connectedStore(
      [waiting, open],
      states: {id(1): AgentState.waitingForInput, id(2): AgentState.idle},
    );
    final feed = buildActivityFeed([store]);

    expect(filterActivity(feed, ActivityFilter.all).length, feed.length);

    final needs = filterActivity(feed, ActivityFilter.needsYou);
    expect(needs.every((e) => e.actionable), isTrue);
    expect(needs.map((e) => e.kind), contains(ActivityKind.needsYou));

    final prs = filterActivity(feed, ActivityFilter.prs);
    expect(prs.map((e) => e.kind), everyElement(ActivityKind.prReady));
    expect(prs, isNotEmpty);

    expect(needsYouCount(feed), 1);
  });

  test('non-actionable events are ordered most-recent first', () async {
    final older = sessionInfo(
      id: id(1),
      title: 'older',
      unread: true,
      lastAttachedAt: DateTime.utc(2026, 1, 1, 8),
    );
    final newer = sessionInfo(
      id: id(2),
      title: 'newer',
      unread: true,
      lastAttachedAt: DateTime.utc(2026, 1, 1, 20),
    );
    final store = await connectedStore(
      [older, newer],
      states: {id(1): AgentState.idle, id(2): AgentState.idle},
    );

    final feed = buildActivityFeed([store]);
    expect(feed.map((e) => e.title), ['newer', 'older']);
    expect(feed.every((e) => e.kind == ActivityKind.finishedUnread), isTrue);
  });

  test('stopped sessions do not contribute events', () async {
    final stopped = sessionInfo(
      id: id(1),
      title: 'dead',
      status: SessionStatus.stopped,
      unread: true,
    );
    final store = await connectedStore(
      [stopped],
      states: {id(1): AgentState.idle},
    );
    expect(buildActivityFeed([store]), isEmpty);
  });
}
