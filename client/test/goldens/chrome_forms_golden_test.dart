import 'package:claude_commander_client/chrome/chrome.dart';
import 'package:claude_commander_client/chrome/chrome_forms.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/golden.dart';

/// Reference images for each chrome form, in both themes.
///
/// Isolated widgets rather than screens on purpose: this is the layer where the
/// two themes genuinely disagree — a bordered card against a top-ruled panel, an
/// app bar against an elbow rail — and it has no visual coverage otherwise. A
/// form rendered on its own also cannot be churned by a copy change three pages
/// away, so a diff here always means the chrome moved.
void main() {
  /// Runs [body] once per theme, naming the golden `<case>_<theme>`.
  void forEachTheme(
    String name,
    Size size,
    Widget Function(CommanderTokens tokens) build,
  ) {
    goldenThemes.forEach((theme, tokens) {
      testWidgets('$name · $theme', (tester) async {
        await pumpGolden(
          tester,
          tokens: tokens,
          size: size,
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: build(tokens),
          ),
        );
        await expectGolden(tester, '${name}_$theme');
      });
    });
  }

  forEachTheme(
    'window_bar',
    const Size(460, 60),
    (_) => ChromeWindowBar(
      ChromeWindowBarSpec(
        title: 'Claude Commander',
        onDragStart: () {},
        onDoubleTap: () {},
        onMinimize: () {},
        onToggleMaximize: () {},
        onClose: () {},
      ),
    ),
  );

  // Maximised as its own case: the control swaps glyph *and* tooltip, and this
  // is the only place that pairing is asserted visually.
  forEachTheme(
    'window_bar_maximized',
    const Size(460, 60),
    (_) => ChromeWindowBar(
      ChromeWindowBarSpec(
        title: 'Claude Commander',
        isMaximized: true,
        onDragStart: () {},
        onDoubleTap: () {},
        onMinimize: () {},
        onToggleMaximize: () {},
        onClose: () {},
      ),
    ),
  );

  // A run of rows, not one: LCARS rounds only a run's outer ends, so a single
  // row would never exercise `ChromeRowPosition`.
  forEachTheme(
    'list_row_run',
    const Size(420, 260),
    (_) => Column(
      children: [
        ChromeListRow(
          ChromeListRowSpec(
            title: 'conversation-model',
            subtitle: 'stopped · my-repo',
            trailing: 'stopped',
            tone: SessionTone.idle,
            number: '94',
            position: ChromeRowPosition.first,
            onTap: () {},
          ),
        ),
        ChromeListRow(
          ChromeListRowSpec(
            title: 'nix-ci-slow',
            trailing: 'waiting',
            tone: SessionTone.waiting,
            number: '09',
            onTap: () {},
          ),
        ),
        ChromeListRow(
          ChromeListRowSpec(
            title: 'flutter-ful',
            trailing: 'working',
            tone: SessionTone.working,
            number: '45',
            selected: true,
            position: ChromeRowPosition.last,
            onTap: () {},
          ),
        ),
      ],
    ),
  );

  forEachTheme(
    'panel',
    const Size(420, 160),
    (tokens) => ChromePanel(
      ChromePanelSpec(
        eyebrow: 'SUMMARY',
        tone: SessionTone.working,
        child: Text('3 files changed · +42 −7', style: tokens.meta(size: 11)),
      ),
    ),
  );

  forEachTheme(
    'eyebrow',
    const Size(420, 80),
    (_) => const ChromeEyebrow('CLAUDE-COMMANDER · 7'),
  );

  forEachTheme(
    'button_bar',
    const Size(460, 110),
    (_) => ChromeButtonBar(
      ChromeButtonBarSpec(
        buttons: [
          ChromeBarButton(
            label: 'Shell',
            icon: Icons.terminal,
            onPressed: () {},
          ),
          ChromeBarButton(
            label: 'Restart',
            icon: Icons.refresh,
            onPressed: () {},
          ),
          ChromeBarButton(
            label: 'Kill',
            icon: Icons.stop_circle_outlined,
            kind: ChromeActionKind.destructive,
            onPressed: () {},
          ),
        ],
      ),
    ),
  );

  forEachTheme(
    'segmented',
    const Size(420, 90),
    (_) => ChromeSegmented(
      ChromeSegmentedSpec(
        segments: [
          ChromeSegment(label: 'Recent', selected: false, onTap: () {}),
          ChromeSegment(label: 'All', selected: true, onTap: () {}),
        ],
        note: 'grouped',
      ),
    ),
  );

  forEachTheme(
    'field',
    const Size(420, 90),
    (_) => ChromeField(
      const ChromeFieldSpec(
        hint: 'Filter by name, branch, program…',
        icon: Icons.search,
      ),
    ),
  );
}
