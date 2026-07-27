import 'package:flutter/material.dart';

import '../src/rust/api/mirrors.dart';
import 'app_colors.dart';

/// A resolved visual descriptor for a session/agent state: the glyph shown in
/// the leading column of a list row, its colour, and a short lower-case label.
///
/// This is the single source of the deck's state vocabulary — working ● (teal),
/// waiting ? (amber), unread ◆ (accent), idle ● (grey), stopped ○, cascade
/// paused ⏸ (amber) — reused by the Fleet list, the rail, session detail, and
/// the Activity feed so the glyphs never drift between screens.
@immutable
class StateDescriptor {
  final String glyph;
  final Color color;
  final String label;

  /// Whether this state should animate (a gentle pulse) — used for the live
  /// "working" dot.
  final bool pulse;

  const StateDescriptor(
    this.glyph,
    this.color,
    this.label, {
    this.pulse = false,
  });
}

/// The leading glyph for a session row, combining lifecycle [SessionStatus],
/// the live [AgentState], and the unread flag. Lifecycle is checked first
/// (cascade-paused / stopped / creating / merging / pushing win outright); only
/// a running session falls through to the live agent sub-state, where the order
/// is waiting → working → unread → idle.
StateDescriptor sessionDescriptor(SessionInfo info, AgentState agent) {
  switch (info.status) {
    case SessionStatus.cascadePaused:
      return const StateDescriptor('⏸', AppColors.amber, 'cascade paused');
    case SessionStatus.stopped:
      return const StateDescriptor('○', AppColors.idle, 'stopped');
    case SessionStatus.creating:
      return const StateDescriptor('◍', AppColors.accentSoft, 'creating');
    case SessionStatus.merging:
      return const StateDescriptor('⑃', AppColors.accentSoft, 'merging');
    case SessionStatus.pushing:
      return const StateDescriptor('⬆', AppColors.teal, 'pushing');
    case SessionStatus.running:
      break;
  }
  // Running: the live agent sub-state drives the glyph.
  switch (agent) {
    case AgentState.waitingForInput:
      return const StateDescriptor('?', AppColors.amber, 'waiting');
    case AgentState.working:
      return const StateDescriptor('●', AppColors.teal, 'working', pulse: true);
    case AgentState.idle:
      if (info.unread) {
        return const StateDescriptor('◆', AppColors.accent, 'unread');
      }
      return const StateDescriptor('●', AppColors.idle, 'idle');
    case AgentState.unknown:
      if (info.unread) {
        return const StateDescriptor('◆', AppColors.accent, 'unread');
      }
      return const StateDescriptor('●', AppColors.idle, 'idle');
  }
}

/// The leading state glyph as a fixed-width coloured cell, matching the deck's
/// list rows. When [StateDescriptor.pulse] is set the glyph breathes.
class SessionGlyph extends StatefulWidget {
  final StateDescriptor descriptor;
  final double size;
  final double width;

  const SessionGlyph(
    this.descriptor, {
    super.key,
    this.size = 11,
    this.width = 14,
  });

  @override
  State<SessionGlyph> createState() => _SessionGlyphState();
}

class _SessionGlyphState extends State<SessionGlyph>
    with SingleTickerProviderStateMixin {
  // Constructed eagerly in initState (not as a lazy `late final`): a non-pulsing
  // glyph never reads _c during its life, so a lazy field would first construct
  // the controller inside dispose() — and the AnimationController ctor does a
  // TickerMode inherited lookup, which is unsafe once the element is unmounting.
  late final AnimationController _c;

  @override
  void initState() {
    super.initState();
    _c = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1600),
    );
    if (widget.descriptor.pulse) _c.repeat(reverse: true);
  }

  @override
  void didUpdateWidget(SessionGlyph old) {
    super.didUpdateWidget(old);
    if (widget.descriptor.pulse && !_c.isAnimating) {
      _c.repeat(reverse: true);
    } else if (!widget.descriptor.pulse && _c.isAnimating) {
      _c.stop();
      _c.value = 1;
    }
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final glyph = Text(
      widget.descriptor.glyph,
      textAlign: TextAlign.center,
      style: TextStyle(
        fontSize: widget.size,
        color: widget.descriptor.color,
        height: 1,
      ),
    );
    return SizedBox(
      width: widget.width,
      child: widget.descriptor.pulse
          ? FadeTransition(
              opacity: Tween(begin: 0.4, end: 1.0).animate(_c),
              child: glyph,
            )
          : glyph,
    );
  }
}
