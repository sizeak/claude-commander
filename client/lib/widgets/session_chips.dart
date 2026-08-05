import 'package:flutter/material.dart';

import '../src/rust/api/mirrors.dart';
import '../theme/tokens.dart';

/// Small status/PR/agent-state pills shared by the session list and detail
/// views, so their colour coding stays consistent. All pills render through
/// [AppChip]; colours come from [CommanderTokens].

Widget statusChip(BuildContext context, SessionStatus status) {
  final t = CommanderTokens.of(context);
  final (label, color) = switch (status) {
    SessionStatus.creating => ('creating', t.info),
    SessionStatus.running => ('running', t.textBright),
    SessionStatus.stopped => ('stopped', t.idle),
    SessionStatus.merging => ('merging', t.info),
    SessionStatus.cascadePaused => ('cascade paused', t.held),
    SessionStatus.pushing => ('pushing', t.working),
  };
  // "running" reads as neutral metadata rather than a status accent.
  return status == SessionStatus.running
      ? AppChip(label: label, color: color, neutral: true)
      : AppChip(label: label, color: color);
}

/// The agent sub-state only carries meaning while a session is running, so the
/// caller decides whether to show it.
Widget agentStateChip(BuildContext context, AgentState state) {
  final t = CommanderTokens.of(context);
  final (label, color) = switch (state) {
    AgentState.working => ('● working', t.working),
    AgentState.idle => ('idle', t.idle),
    AgentState.waitingForInput => ('? waiting for input', t.attention),
    AgentState.unknown => ('unknown', t.idle),
  };
  return AppChip(label: label, color: color);
}

Widget prChip(BuildContext context, int number, PrState state) {
  final t = CommanderTokens.of(context);
  final (label, color) = switch (state) {
    PrState.open => ('PR #$number open', t.info),
    PrState.closed => ('PR #$number closed', t.danger),
    PrState.merged => ('PR #$number merged', t.info),
  };
  return AppChip(label: label, color: color);
}

/// The session's section, shown read-only in the detail header (editing lives in
/// the overflow menu). Neutral styling — it's metadata, not a status.
Widget sectionChip(BuildContext context, String section) => AppChip(
  label: '▤ $section',
  color: CommanderTokens.of(context).textMuted,
  neutral: true,
);

/// Marker that the session is pinned alive (won't be hibernated). Tinted with
/// the accent — it's an opt-in mode worth highlighting, not a passive status.
Widget keepAliveChip(BuildContext context) =>
    AppChip(label: '✓ keep-alive', color: CommanderTokens.of(context).info);

/// The shared pill used by every chip above and directly by pages that need a
/// one-off badge. A tinted chip fills at ~14% of [color] with a ~40% border and
/// coloured mono text; a [neutral] chip uses the flat surface/border treatment
/// with muted text (used for passive metadata like "running" or a section).
class AppChip extends StatelessWidget {
  final String label;
  final Color color;
  final bool neutral;

  const AppChip({
    super.key,
    required this.label,
    required this.color,
    this.neutral = false,
  });

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
      decoration: BoxDecoration(
        color: neutral ? t.surface : color.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(t.pillRadius),
        border: Border.all(
          color: neutral ? t.border : color.withValues(alpha: 0.4),
        ),
      ),
      child: Text(
        label,
        style: t.meta(
          size: 9.5,
          weight: FontWeight.w600,
          color: neutral ? t.textMuted : color,
        ),
      ),
    );
  }
}
