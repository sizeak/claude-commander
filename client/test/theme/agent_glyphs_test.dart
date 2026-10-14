import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/theme/agent_glyphs.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fixtures.dart';
import 'mission_control_reference.dart';

void main() {
  group('sessionDescriptor', () {
    test('waiting for input wins over unread while running', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.running, unread: true),
        AgentState.waitingForInput,
      );
      expect(d.glyph, '?');
      expect(d.tone, SessionTone.waiting);
      expect(d.label, 'waiting');
    });

    test('working shows the pulsing dot', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.running),
        AgentState.working,
      );
      expect(d.glyph, '●');
      expect(d.tone, SessionTone.working);
      expect(d.pulse, isTrue);
    });

    test('idle + unread shows the unread diamond', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.running, unread: true),
        AgentState.idle,
      );
      expect(d.glyph, '◆');
      expect(d.tone, SessionTone.unread);
      expect(d.label, 'unread');
    });

    test('an unknown agent state still surfaces unread', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.running, unread: true),
        AgentState.unknown,
      );
      expect(d.tone, SessionTone.unread);
    });

    test('cascade-paused lifecycle overrides the agent state', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.cascadePaused),
        AgentState.working,
      );
      expect(d.glyph, '⏸');
      expect(d.tone, SessionTone.held);
    });

    test('stopped lifecycle renders the hollow idle dot', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.stopped),
        AgentState.unknown,
      );
      expect(d.glyph, '○');
      expect(d.tone, SessionTone.stopped);
    });
  });

  group('the pre-theming colours are preserved', () {
    // Before the tone refactor these colours were baked into StateDescriptor.
    // Resolving tone → colour under Mission Control must reproduce them exactly,
    // or the "Phase 1 is visually neutral" claim is false.
    const mc = missionControlTokens;

    test('every lifecycle state resolves to the colour it used to carry', () {
      final cases = <(SessionStatus, AgentState, Color)>[
        (SessionStatus.running, AgentState.waitingForInput, McRef.amber),
        (SessionStatus.running, AgentState.working, McRef.teal),
        (SessionStatus.running, AgentState.idle, McRef.idle),
        (SessionStatus.cascadePaused, AgentState.working, McRef.amber),
        (SessionStatus.stopped, AgentState.unknown, McRef.idle),
        (SessionStatus.creating, AgentState.unknown, McRef.accentSoft),
        (SessionStatus.merging, AgentState.unknown, McRef.accentSoft),
        (SessionStatus.pushing, AgentState.unknown, McRef.teal),
      ];
      for (final (status, agent, expected) in cases) {
        final d = sessionDescriptor(sessionInfo(status: status), agent);
        expect(
          mc.toneStyle(d.tone).accent,
          expected,
          reason: '$status / $agent',
        );
      }
    });

    test('unread resolves to the old accent', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.running, unread: true),
        AgentState.idle,
      );
      expect(mc.toneStyle(d.tone).accent, McRef.accent);
    });
  });

  group('wantsAttention', () {
    // The old call sites asked `descriptor.color == AppColors.amber`, which was
    // true for waiting AND cascade-paused because they shared one amber. This is
    // the regression the tone split could most easily have introduced.
    test('covers both waiting and cascade-paused', () {
      final waiting = sessionDescriptor(
        sessionInfo(status: SessionStatus.running),
        AgentState.waitingForInput,
      );
      final held = sessionDescriptor(
        sessionInfo(status: SessionStatus.cascadePaused),
        AgentState.idle,
      );
      expect(waiting.wantsAttention, isTrue);
      expect(held.wantsAttention, isTrue, reason: 'was amber before the split');
    });

    test('and nothing else', () {
      for (final (status, agent) in <(SessionStatus, AgentState)>[
        (SessionStatus.running, AgentState.working),
        (SessionStatus.running, AgentState.idle),
        (SessionStatus.stopped, AgentState.unknown),
        (SessionStatus.creating, AgentState.unknown),
        (SessionStatus.merging, AgentState.unknown),
        (SessionStatus.pushing, AgentState.unknown),
      ]) {
        final d = sessionDescriptor(sessionInfo(status: status), agent);
        expect(d.wantsAttention, isFalse, reason: '$status / $agent');
      }
    });
  });

  group('SessionGlyph', () {
    // One test per theme rather than a loop: re-pumping with a different theme
    // in the same test starts MaterialApp's AnimatedTheme mid-lerp, and this
    // glyph pulses forever so pumpAndSettle can never resolve it. A fresh pump
    // per test has no previous theme to animate from.
    Future<Color?> glyphColour(WidgetTester tester, CommanderTokens t) async {
      await tester.pumpWidget(
        MaterialApp(
          theme: ThemeData.dark().copyWith(extensions: [t]),
          home: SessionGlyph(
            sessionDescriptor(
              sessionInfo(status: SessionStatus.running),
              AgentState.working,
            ),
          ),
        ),
      );
      return tester.widget<Text>(find.byType(Text)).style?.color;
    }

    testWidgets('paints working teal under Mission Control', (tester) async {
      expect(await glyphColour(tester, missionControlTokens), McRef.teal);
    });

    testWidgets('paints working amber under LCARS', (tester) async {
      // The role remap that a hue-keyed palette could not have expressed.
      expect(
        await glyphColour(tester, lcarsTokens),
        lcarsTokens.toneStyle(SessionTone.working).accent,
      );
      expect(await glyphColour(tester, lcarsTokens), isNot(McRef.teal));
    });
  });
}
