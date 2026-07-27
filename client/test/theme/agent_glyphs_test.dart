import 'package:claude_commander_client/theme/agent_glyphs.dart';
import 'package:claude_commander_client/theme/app_colors.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fixtures.dart';

void main() {
  group('sessionDescriptor', () {
    test('waiting for input wins over unread while running', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.running, unread: true),
        AgentState.waitingForInput,
      );
      expect(d.glyph, '?');
      expect(d.color, AppColors.amber);
      expect(d.label, 'waiting');
    });

    test('working shows the pulsing teal dot', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.running),
        AgentState.working,
      );
      expect(d.glyph, '●');
      expect(d.color, AppColors.teal);
      expect(d.pulse, isTrue);
    });

    test('idle + unread shows the accent unread diamond', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.running, unread: true),
        AgentState.idle,
      );
      expect(d.glyph, '◆');
      expect(d.color, AppColors.accent);
      expect(d.label, 'unread');
    });

    test('cascade-paused lifecycle overrides the agent state', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.cascadePaused),
        AgentState.working,
      );
      expect(d.glyph, '⏸');
      expect(d.color, AppColors.amber);
    });

    test('stopped lifecycle renders the hollow idle dot', () {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.stopped),
        AgentState.unknown,
      );
      expect(d.glyph, '○');
      expect(d.color, AppColors.idle);
    });
  });
}
