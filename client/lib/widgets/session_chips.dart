import 'package:flutter/material.dart';

import '../src/rust/api/mirrors.dart';

/// Small status/PR/agent-state pills shared by the session list and detail
/// views, so their colour coding stays consistent.

Widget statusChip(BuildContext context, SessionStatus status) {
  final (label, color) = switch (status) {
    SessionStatus.creating => ('creating', Colors.blue),
    SessionStatus.running => ('running', Colors.green),
    SessionStatus.stopped => ('stopped', Colors.grey),
    SessionStatus.merging => ('merging', Colors.orange),
    SessionStatus.cascadePaused => ('cascade paused', Colors.deepOrange),
    SessionStatus.pushing => ('pushing', Colors.teal),
  };
  return _chip(label, color);
}

/// The agent sub-state only carries meaning while a session is running, so the
/// caller decides whether to show it.
Widget agentStateChip(BuildContext context, AgentState state) {
  final (label, color) = switch (state) {
    AgentState.working => ('working', Colors.lightBlue),
    AgentState.idle => ('idle', Colors.green),
    AgentState.waitingForInput => ('waiting', Colors.amber),
    AgentState.unknown => ('unknown', Colors.grey),
  };
  return _chip(label, color);
}

Widget prChip(BuildContext context, int number, PrState state) {
  final color = switch (state) {
    PrState.open => Colors.green,
    PrState.closed => Colors.red,
    PrState.merged => Colors.purple,
  };
  return _chip('PR #$number ${state.name}', color);
}

/// The session's section, shown read-only in the detail header (editing lives in
/// the overflow menu). Uses the theme's outline colour so it reads as neutral
/// metadata rather than a status.
Widget sectionChip(BuildContext context, String section) =>
    _chip('§ $section', Theme.of(context).colorScheme.outline);

/// Marker that the session is pinned alive (won't be hibernated). Tinted with
/// the theme's tertiary accent — it's an opt-in mode worth highlighting, not a
/// passive status.
Widget keepAliveChip(BuildContext context) =>
    _chip('keep alive', Theme.of(context).colorScheme.tertiary);

Widget _chip(String label, Color color) {
  return Container(
    padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
    decoration: BoxDecoration(
      color: color.withValues(alpha: 0.18),
      borderRadius: BorderRadius.circular(6),
      border: Border.all(color: color.withValues(alpha: 0.5)),
    ),
    child: Text(label, style: TextStyle(color: color, fontSize: 12)),
  );
}
