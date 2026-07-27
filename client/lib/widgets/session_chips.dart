import 'package:flutter/material.dart';

import '../src/rust/api/mirrors.dart';
import '../theme/app_colors.dart';
import '../theme/app_theme.dart';

/// Small status/PR/agent-state pills shared by the session list and detail
/// views, so their colour coding stays consistent. All pills render through
/// [AppChip]; colours come from [AppColors].

Widget statusChip(BuildContext context, SessionStatus status) {
  final (label, color) = switch (status) {
    SessionStatus.creating => ('creating', AppColors.accentSoft),
    SessionStatus.running => ('running', AppColors.textBright),
    SessionStatus.stopped => ('stopped', AppColors.idle),
    SessionStatus.merging => ('merging', AppColors.accentSoft),
    SessionStatus.cascadePaused => ('cascade paused', AppColors.amber),
    SessionStatus.pushing => ('pushing', AppColors.teal),
  };
  // "running" reads as neutral metadata rather than a status accent.
  return status == SessionStatus.running
      ? AppChip(label: label, color: color, neutral: true)
      : AppChip(label: label, color: color);
}

/// The agent sub-state only carries meaning while a session is running, so the
/// caller decides whether to show it.
Widget agentStateChip(BuildContext context, AgentState state) {
  final (label, color) = switch (state) {
    AgentState.working => ('● working', AppColors.teal),
    AgentState.idle => ('idle', AppColors.idle),
    AgentState.waitingForInput => ('? waiting for input', AppColors.amber),
    AgentState.unknown => ('unknown', AppColors.idle),
  };
  return AppChip(label: label, color: color);
}

Widget prChip(BuildContext context, int number, PrState state) {
  final (label, color) = switch (state) {
    PrState.open => ('PR #$number open', AppColors.accentSoft),
    PrState.closed => ('PR #$number closed', AppColors.red),
    PrState.merged => ('PR #$number merged', AppColors.accentSoft),
  };
  return AppChip(label: label, color: color);
}

/// The session's section, shown read-only in the detail header (editing lives in
/// the overflow menu). Neutral styling — it's metadata, not a status.
Widget sectionChip(BuildContext context, String section) =>
    AppChip(label: '▤ $section', color: AppColors.textMuted, neutral: true);

/// Marker that the session is pinned alive (won't be hibernated). Tinted with
/// the accent — it's an opt-in mode worth highlighting, not a passive status.
Widget keepAliveChip(BuildContext context) =>
    AppChip(label: '✓ keep-alive', color: AppColors.accentSoft);

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
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
      decoration: BoxDecoration(
        color: neutral ? AppColors.surface : color.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(20),
        border: Border.all(
          color: neutral ? AppColors.border : color.withValues(alpha: 0.4),
        ),
      ),
      child: Text(
        label,
        style: AppTheme.mono(
          size: 9.5,
          weight: FontWeight.w600,
          color: neutral ? AppColors.textMuted : color,
        ),
      ),
    );
  }
}
