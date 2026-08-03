import 'package:claude_commander_client/theme/terminal_theme.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'mission_control_reference.dart';

/// Parity: the Mission Control token set must reproduce the pre-theming palette
/// exactly, role by role. This is the only thing standing between a ~357-site
/// mechanical migration and a silent colour regression — the widget suite has
/// almost no colour assertions, so nothing else would notice.
///
/// See `mission_control_reference.dart` for why the expected values live in a
/// test fixture rather than in `lib/`.
void main() {
  const mc = missionControlTokens;
  const lcars = lcarsTokens;

  group('Mission Control token parity', () {
    test('surfaces', () {
      expect(mc.terminalBg, McRef.bgTerminal);
      expect(mc.canvas, McRef.bg);
      expect(mc.canvasRaised, McRef.bgRaised);
      expect(mc.surface, McRef.surface);
      expect(mc.surfaceSelected, McRef.surfaceSel);
    });

    test('borders', () {
      expect(mc.border, McRef.border);
      expect(mc.borderSubtle, McRef.borderSubtle);
      expect(mc.divider, McRef.divider);
    });

    test('text ramp', () {
      expect(mc.text, McRef.text);
      expect(mc.textBright, McRef.textBright);
      expect(mc.textMuted, McRef.textMuted);
      expect(mc.textFaint, McRef.textFaint);
      expect(mc.textDim, McRef.textDim);
    });

    test('semantic accents', () {
      expect(mc.primary, McRef.accent);
      expect(mc.primarySoft, McRef.accentSoft);
      expect(mc.working, McRef.teal);
      expect(mc.success, McRef.green);
      expect(mc.attention, McRef.amber);
      expect(mc.attentionOn, McRef.amberText);
      expect(mc.danger, McRef.red);
      expect(mc.idle, McRef.idle);
    });

    test('roles Mission Control conflates onto one hue', () {
      // LCARS separates these; Mission Control never did, and Phase 1 must not
      // invent a distinction the old palette did not have.
      expect(mc.nav, McRef.accent, reason: 'nav was never its own hue');
      expect(mc.info, McRef.accentSoft);
      expect(mc.unread, McRef.accent);
      expect(mc.held, McRef.amber, reason: 'held shared amber with attention');
    });

    test('terminal + diff', () {
      expect(mc.terminalFg, McRef.terminalFg);
      expect(mc.diffGutter, McRef.diffGutter);
      expect(mc.diffGutterBg, McRef.diffGutterBg);
    });

    test('brand gradient', () {
      expect(mc.brandGradient, McRef.brandGradient);
    });

    test('fonts', () {
      expect(mc.sans, McRef.sans);
      expect(mc.mono, McRef.mono);
    });
  });

  group('xterm terminal theme parity', () {
    // terminal_page.dart's `_terminalTheme` was a top-level const built from
    // AppColors. All 24 fields are pinned so LCARS cannot be given an amber
    // cursor by accidentally changing Mission Control's.
    final t = terminalThemeFor(mc);

    test('cursor, selection, foreground, background', () {
      expect(t.cursor, McRef.accent);
      // Compared as packed 8-bit: the old value was the literal `0x407C6CFF`,
      // whose alpha is 64/255 = 0.25098, while `withValues(alpha: 0.25)` stores
      // exactly 0.25. Those doubles are unequal but both render as 0x40, and
      // what reaches the framebuffer is what matters here.
      expect(t.selection.toARGB32(), 0x407C6CFF);
      expect(t.foreground, McRef.terminalFg);
      expect(t.background, McRef.bgTerminal);
    });

    test('the normal ANSI ramp', () {
      expect(t.black, McRef.borderSubtle);
      expect(t.red, McRef.red);
      expect(t.green, McRef.green);
      expect(t.yellow, McRef.amber);
      expect(t.blue, McRef.accentSoft);
      expect(t.magenta, McRef.accent);
      expect(t.cyan, McRef.teal);
      expect(t.white, McRef.textBright);
    });

    test('the bright ANSI ramp', () {
      expect(t.brightBlack, McRef.textDim);
      expect(t.brightRed, McRef.red);
      expect(t.brightGreen, McRef.green);
      expect(t.brightYellow, McRef.amberText);
      expect(t.brightBlue, McRef.accentSoft);
      expect(t.brightMagenta, McRef.accentSoft);
      expect(t.brightCyan, McRef.teal);
      expect(t.brightWhite, McRef.text);
    });

    test('search hits', () {
      expect(t.searchHitBackground.toARGB32(), 0x66F5B545);
      expect(t.searchHitBackgroundCurrent, McRef.amber);
      expect(t.searchHitForeground, McRef.bg);
    });
  });

  group('tone resolution', () {
    test('Mission Control reproduces the old descriptor colours exactly', () {
      // These were `StateDescriptor`'s hardcoded colours before the refactor.
      expect(mc.toneStyle(SessionTone.waiting).accent, McRef.amber);
      expect(mc.toneStyle(SessionTone.held).accent, McRef.amber);
      expect(mc.toneStyle(SessionTone.working).accent, McRef.teal);
      expect(mc.toneStyle(SessionTone.pushing).accent, McRef.teal);
      expect(mc.toneStyle(SessionTone.creating).accent, McRef.accentSoft);
      expect(mc.toneStyle(SessionTone.merging).accent, McRef.accentSoft);
      expect(mc.toneStyle(SessionTone.unread).accent, McRef.accent);
      expect(mc.toneStyle(SessionTone.idle).accent, McRef.idle);
      expect(mc.toneStyle(SessionTone.stopped).accent, McRef.idle);
    });

    test('waiting and held are indistinguishable in Mission Control', () {
      // The old `descriptor.color == AppColors.amber` check meant "waiting OR
      // cascade-paused". Splitting the tones must not split the tint.
      expect(
        mc.toneStyle(SessionTone.held).accent,
        mc.toneStyle(SessionTone.waiting).accent,
      );
    });

    test('LCARS gives waiting and held distinct accents', () {
      expect(
        lcars.toneStyle(SessionTone.held).accent,
        isNot(lcars.toneStyle(SessionTone.waiting).accent),
      );
    });

    test('both themes resolve every tone to a complete triple', () {
      for (final tokens in [mc, lcars]) {
        for (final tone in SessionTone.values) {
          final style = tokens.toneStyle(tone);
          expect(style.accent.a, 1.0, reason: '$tone accent must be opaque');
          expect(style.onTint.a, 1.0, reason: '$tone onTint must be opaque');
          // tintedSurface may be translucent (Mission Control tints with alpha).
          expect(style.tintedSurface, isNotNull);
        }
      }
    });
  });

  group('lerp', () {
    test('lerping to another token set yields the target at t = 1', () {
      final lerped = mc.lerp(lcars, 1.0);
      expect(lerped.primary, lcars.primary);
      expect(lerped.canvas, lcars.canvas);
    });

    test('a non-CommanderTokens other returns this unchanged', () {
      expect(mc.lerp(null, 0.5), same(mc));
    });
  });

  group('CommanderTokens.of', () {
    testWidgets('resolves the extension from the enclosing theme', (
      tester,
    ) async {
      late CommanderTokens seen;
      await tester.pumpWidget(
        MaterialApp(
          theme: ThemeData.dark().copyWith(extensions: const [lcarsTokens]),
          home: Builder(
            builder: (context) {
              seen = CommanderTokens.of(context);
              return const SizedBox();
            },
          ),
        ),
      );
      expect(seen.primary, lcarsTokens.primary);
    });

    testWidgets('falls back to Mission Control with no extension', (
      tester,
    ) async {
      // Deliberate: the existing 20 test files pump bare widgets with no theme
      // wrapper, and must keep working untouched.
      late CommanderTokens seen;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) {
              seen = CommanderTokens.of(context);
              return const SizedBox();
            },
          ),
        ),
      );
      expect(seen.primary, missionControlTokens.primary);
    });
  });
}
