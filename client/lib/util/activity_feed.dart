import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import 'session_filter.dart' show SessionStatusActive;

/// Derives the cross-server Activity timeline from live session + operation
/// state. Kept Flutter-free (no widget imports) and pure so it unit-tests like
/// `session_filter.dart`: [buildActivityFeed] is a total function of the stores'
/// current snapshots, with no side effects.
///
/// The feed is honest about the data the server actually exposes. Notably absent
/// are the deck's "committed N files" and "CI passed/failed" rows: neither a
/// per-session commit count nor a CI check status is carried in
/// [WorkspaceSnapshotDto] / [SessionInfo], so those kinds are deliberately not
/// synthesised (see the module doc on [ActivityKind]).

/// The kinds of event the feed can surface — only those derivable from
/// [SessionInfo] / [OperationStatusDto].
///
/// Deliberately omitted because the server exposes no field to derive them from
/// (fabricating them would be dishonest):
/// - `committed` — no commit count or commit timestamp in the snapshot.
/// - `ciPassed` / `ciFailed` — no CI check status; [SessionInfo] carries only PR
///   *review* state ([ReviewDecision]), not CI. (There is likewise no CI filter:
///   with no derivable CI events it would be a phantom chip.)
enum ActivityKind {
  /// A running agent is blocked on the user (`waitingForInput`). Actionable.
  needsYou,

  /// A session's cascade is paused awaiting a resume/abandon decision
  /// (`SessionStatus.cascadePaused`). Actionable.
  paused,

  /// An open, non-draft PR whose review is still required — ready to look at.
  prReady,

  /// The session's PR has been merged.
  prMerged,

  /// A push-stack operation completed on the server.
  pushed,

  /// The agent went idle with unread output — it finished and you haven't looked.
  finishedUnread,

  /// The agent is actively working.
  working,
}

/// The Activity screen's filter chips.
enum ActivityFilter { all, needsYou, prs }

/// One derived timeline entry.
///
/// [sessionId] (the [SessionInfo.id] string) is null for server-level events
/// (currently [ActivityKind.pushed], which the operation log does not tie to a
/// session), so the UI only offers a tap-through when it is present. [serverId]
/// is always the owning [ServerConfig.id], so the UI can resolve the store.
class ActivityEvent {
  final ActivityKind kind;

  /// The session this event belongs to, or null for a server-level event.
  final String? sessionId;

  /// The owning server ([ServerConfig.id]) — always set, so the UI can navigate.
  final String serverId;

  /// The server's display name ([ServerConfig.name]).
  final String serverName;

  /// The session's project, or null for a server-level event.
  final String? projectName;

  /// The row's headline (session title, or a synthetic label for op events).
  final String title;

  /// The mono sub-line, e.g. "waiting for input" / "PR #12 merged".
  final String description;

  /// Best-effort event time (most-recent first); null sorts last.
  final DateTime? at;

  /// Whether the event awaits the user (a waiting agent / a paused cascade). The
  /// deck renders these as amber "NEEDS YOU" cards with an "Answer" affordance.
  final bool actionable;

  const ActivityEvent({
    required this.kind,
    required this.sessionId,
    required this.serverId,
    required this.serverName,
    required this.projectName,
    required this.title,
    required this.description,
    required this.at,
    required this.actionable,
  });

  /// The trailing location label used by the deck ("· genio" / "· workstation"):
  /// the project for a session event, else the server for an op event.
  String get location => projectName ?? serverName;
}

/// Builds the cross-server feed from every server's current snapshot.
///
/// Actionable events (waiting agents, paused cascades) are floated to the top;
/// everything else follows, each group ordered most-recent first (events with no
/// timestamp sort last). Order within a rank is stable, so equal timestamps keep
/// server/session enumeration order.
///
/// Only *active* sessions contribute (stopped sessions are skipped, mirroring the
/// Recent tab's rationale — a dead session isn't current activity). A single
/// session can yield both a state event (waiting/working/…) and a PR event, as
/// the deck shows (e.g. an agent waiting on a session that also has a PR ready).
List<ActivityEvent> buildActivityFeed(
  List<CommanderStore> servers, {
  DateTime? now,
}) {
  final events = <ActivityEvent>[];

  for (final store in servers) {
    final serverId = store.config.id;
    final serverName = store.config.name;

    for (final s in store.sessions) {
      if (!s.status.isActive) continue;
      final agent = store.agentStateFor(s.id);

      final state = _stateEvent(s, agent, serverId, serverName);
      if (state != null) events.add(state);

      final pr = _prEvent(s, serverId, serverName);
      if (pr != null) events.add(pr);
    }

    for (final op in store.operations) {
      final event = _operationEvent(op, serverId, serverName, now);
      if (event != null) events.add(event);
    }
  }

  // Decorate with the original index so the sort is stable within a rank.
  final indexed = [for (var i = 0; i < events.length; i++) (i, events[i])];
  indexed.sort((a, b) {
    final byRank = _rank(a.$2).compareTo(_rank(b.$2));
    if (byRank != 0) return byRank;
    final byTime = _compareRecency(a.$2.at, b.$2.at);
    if (byTime != 0) return byTime;
    return a.$1.compareTo(b.$1);
  });
  return [for (final e in indexed) e.$2];
}

/// Restricts [events] to a filter. [ActivityFilter.needsYou] keeps everything
/// actionable (both waiting agents and paused cascades); [ActivityFilter.prs]
/// keeps PR lifecycle events.
List<ActivityEvent> filterActivity(
  List<ActivityEvent> events,
  ActivityFilter filter,
) {
  switch (filter) {
    case ActivityFilter.all:
      return events;
    case ActivityFilter.needsYou:
      return [for (final e in events) if (e.actionable) e];
    case ActivityFilter.prs:
      return [
        for (final e in events)
          if (e.kind == ActivityKind.prReady || e.kind == ActivityKind.prMerged)
            e,
      ];
  }
}

/// How many actionable ("needs you") events there are — the count the deck shows
/// on the "Needs you · N" chip.
int needsYouCount(List<ActivityEvent> events) =>
    events.where((e) => e.actionable).length;

// --- derivation helpers ---------------------------------------------------

/// The single lifecycle/agent-state event for a session, by the deck's priority
/// (paused → waiting → finished-unread → working), or null when the session is
/// in a resting state with nothing to report.
ActivityEvent? _stateEvent(
  SessionInfo s,
  AgentState agent,
  String serverId,
  String serverName,
) {
  final at = s.enteredSectionAt ?? s.lastAttachedAt ?? s.createdAt;

  ActivityEvent make(ActivityKind kind, String description, bool actionable) =>
      ActivityEvent(
        kind: kind,
        sessionId: s.id,
        serverId: serverId,
        serverName: serverName,
        projectName: s.projectName,
        title: s.title,
        description: description,
        at: at,
        actionable: actionable,
      );

  if (s.status == SessionStatus.cascadePaused) {
    return make(ActivityKind.paused, 'cascade paused · awaiting decision', true);
  }
  // The agent sub-state only carries meaning while running (mirrors
  // `sessionDescriptor`).
  if (s.status != SessionStatus.running) return null;
  switch (agent) {
    case AgentState.waitingForInput:
      return make(ActivityKind.needsYou, 'waiting for input', true);
    case AgentState.working:
      return make(ActivityKind.working, 'working', false);
    case AgentState.idle:
    case AgentState.unknown:
      if (s.unread) {
        return make(ActivityKind.finishedUnread, 'finished · unread', false);
      }
      return null;
  }
}

/// The PR lifecycle event for a session, or null when it has no noteworthy PR
/// state. Merged wins over ready.
ActivityEvent? _prEvent(SessionInfo s, String serverId, String serverName) {
  final at = s.lastAttachedAt ?? s.createdAt;
  final num = s.prNumber;

  ActivityEvent make(ActivityKind kind, String description) => ActivityEvent(
    kind: kind,
    sessionId: s.id,
    serverId: serverId,
    serverName: serverName,
    projectName: s.projectName,
    title: s.title,
    description: description,
    at: at,
    actionable: false,
  );

  if (s.prMerged || s.prState == PrState.merged) {
    return make(ActivityKind.prMerged, num == null ? 'PR merged' : 'PR #$num merged');
  }
  // "Ready for review": an open, non-draft PR still awaiting a first review.
  if (num != null &&
      s.prState == PrState.open &&
      !s.prDraft &&
      s.reviewDecision == ReviewDecision.reviewRequired) {
    return make(ActivityKind.prReady, 'PR #$num ready for review');
  }
  return null;
}

/// A server-level event for a completed stack operation. Only a succeeded
/// push-stack maps to a kind the deck shows; other operation outcomes (cascade
/// merges, failures, pauses — the last surfaced instead via the session's
/// `cascadePaused` state) are left out rather than shoehorned into a wrong kind.
ActivityEvent? _operationEvent(
  OperationStatusDto op,
  String serverId,
  String serverName,
  DateTime? now,
) {
  if (op.kind != OperationKind.pushStack ||
      op.outcome.kind != OperationOutcomeKind.succeeded) {
    return null;
  }
  final detail = op.outcome.detail.trim();
  return ActivityEvent(
    kind: ActivityKind.pushed,
    sessionId: null,
    serverId: serverId,
    serverName: serverName,
    projectName: null,
    title: 'Push stack',
    description: detail.isEmpty ? 'pushed stack' : detail,
    at: op.finishedAt ?? now ?? DateTime.now(),
    actionable: false,
  );
}

/// Actionable events sort ahead of everything else.
int _rank(ActivityEvent e) => e.actionable ? 0 : 1;

/// Compare two event times most-recent first, with nulls last.
int _compareRecency(DateTime? a, DateTime? b) {
  if (a == null && b == null) return 0;
  if (a == null) return 1;
  if (b == null) return -1;
  return b.compareTo(a);
}
