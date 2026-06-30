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
