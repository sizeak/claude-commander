import 'package:flutter/material.dart';

import '../src/rust/api/mirrors.dart';
import 'tokens.dart';

/// A resolved visual descriptor for a session/agent state: the glyph shown in
/// the leading column of a list row, its [SessionTone], and a short lower-case
/// label.
///
/// This is the single source of the deck's state vocabulary — working ●,
/// waiting ?, unread ◆, idle ●, stopped ○, cascade paused ⏸ — reused by the
/// Fleet list, the rail, session detail, and the Activity feed so the glyphs
/// never drift between screens.
///
/// Carries a [tone] rather than a `Color`. It used to hold the colour directly,
/// which made it unthemable and, worse, invited call sites to compare
/// `descriptor.color == AppColors.amber` as a stand-in for "this row wants
/// attention" — a test that quietly covered *both* waiting and cascade-paused
/// because they shared one amber. Use [sessionWantsAttention] for that question
/// and [CommanderTokens.toneStyle] for the colours.
@immutable
class StateDescriptor {
  final String glyph;
  final SessionTone tone;
  final String label;

  /// Whether this state should animate (a gentle pulse) — used for the live
  /// "working" dot.
  final bool pulse;

  const StateDescriptor(
    this.glyph,
    this.tone,
    this.label, {
    this.pulse = false,
  });

  /// True when this state should pull the eye to its row.
  bool get wantsAttention => sessionWantsAttention(tone);
}

/// The leading glyph for a session row, combining lifecycle [SessionStatus],
/// the live [AgentState], and the unread flag. Lifecycle is checked first
/// (cascade-paused / stopped / creating / merging / pushing win outright); only
/// a running session falls through to the live agent sub-state, where the order
/// is waiting → working → unread → idle.
StateDescriptor sessionDescriptor(SessionInfo info, AgentState agent) {
  switch (info.status) {
    case SessionStatus.cascadePaused:
      return const StateDescriptor('⏸', SessionTone.held, 'cascade paused');
    case SessionStatus.stopped:
      return const StateDescriptor('○', SessionTone.stopped, 'stopped');
    case SessionStatus.creating:
      return const StateDescriptor('◍', SessionTone.creating, 'creating');
    case SessionStatus.merging:
      return const StateDescriptor('⑃', SessionTone.merging, 'merging');
    case SessionStatus.pushing:
      return const StateDescriptor('⬆', SessionTone.pushing, 'pushing');
    case SessionStatus.running:
      break;
  }
  // Running: the live agent sub-state drives the glyph.
  switch (agent) {
    case AgentState.waitingForInput:
      return const StateDescriptor('?', SessionTone.waiting, 'waiting');
    case AgentState.working:
      return const StateDescriptor(
        '●',
        SessionTone.working,
        'working',
        pulse: true,
      );
    case AgentState.idle:
    case AgentState.unknown:
      if (info.unread) {
        return const StateDescriptor('◆', SessionTone.unread, 'unread');
      }
      return const StateDescriptor('●', SessionTone.idle, 'idle');
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
    final tokens = CommanderTokens.of(context);
    final glyph = Text(
      widget.descriptor.glyph,
      textAlign: TextAlign.center,
      style: TextStyle(
        fontSize: widget.size,
        color: tokens.toneStyle(widget.descriptor.tone).accent,
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
