import 'package:flutter/material.dart';

import '../chrome/chrome.dart';
import '../chrome/chrome_forms.dart';
import '../theme/theme_controller.dart';
import '../theme/tokens.dart';

/// The height of a theme's live preview. Big enough to read as a miniature
/// screen, small enough that every theme fits on one phone screen without
/// scrolling past the first card.
const double _previewHeight = 88;

/// Pick the app's theme. One card per [ThemeId], each carrying a live preview
/// painted in that theme's own colours, and tapping one applies it immediately
/// — the whole app rethemes underneath while this page stays open showing the
/// new selection.
///
/// The selection is a device preference, not server state: [ThemeController]
/// persists it to the device's own preference store, and it never travels to a
/// server.
class ThemePickerPage extends StatelessWidget {
  const ThemePickerPage({super.key});

  @override
  Widget build(BuildContext context) {
    final controller = ThemeScope.of(context)!;
    return ChromePage(
      title: 'Theme',
      code: '47-Y',
      // Rebuild on selection so the check mark and accents move even though the
      // app above also rebuilds — this page must not depend on who is listening
      // higher up.
      body: ListenableBuilder(
        listenable: controller,
        builder: (context, _) {
          final t = CommanderTokens.of(context);
          return ListView(
            padding: const EdgeInsets.fromLTRB(14, 4, 14, 24),
            children: [
              Padding(
                padding: const EdgeInsets.only(bottom: 12),
                child: Text(
                  'Applies on this device, across phone, tablet and desktop '
                  'layouts.',
                  style: t.meta(size: 11, height: 1.4),
                ),
              ),
              for (final id in ThemeId.values)
                Padding(
                  padding: const EdgeInsets.only(bottom: 10),
                  child: _ThemeCard(
                    id: id,
                    selected: id == controller.id,
                    onTap: () => controller.select(id),
                  ),
                ),
            ],
          );
        },
      ),
    );
  }
}

/// The mono caption under a theme's name. An exhaustive switch, so adding a
/// theme is a compile error here rather than a card with no description.
String _describe(ThemeId id) => switch (id) {
  ThemeId.missionControl => 'dark · indigo/cyan · Space Grotesk',
  ThemeId.lcars => 'black · amber/lilac · Antonio',
};

/// The badge on a theme's card, or null for none. Exhaustive for the same
/// reason as [_describe].
String? _badge(ThemeId id) => switch (id) {
  ThemeId.missionControl => 'DEFAULT',
  ThemeId.lcars => 'NEW',
};

/// One theme's card: its name, badge, mono description, a check mark when
/// active, and a live preview of the theme itself.
class _ThemeCard extends StatelessWidget {
  final ThemeId id;
  final bool selected;
  final VoidCallback onTap;

  const _ThemeCard({
    required this.id,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    return ChromePanel(
      ChromePanelSpec(
        // Null leaves the panel on each theme's own neutral border rather than
        // picking one here; only the selected card claims the accent.
        accent: selected ? t.primary : null,
        padding: const EdgeInsets.all(10),
        onTap: onTap,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                // Most of the free width goes to the name, leaving the Spacer
                // below just enough to hold the check mark out at the edge.
                Flexible(
                  flex: 4,
                  child: Text(
                    t.caseLabel(id.label),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontFamily: t.sans,
                      fontSize: 15,
                      fontWeight: FontWeight.w700,
                      letterSpacing: t.uppercaseLabels ? 0.7 : -0.2,
                      color: t.text,
                    ),
                  ),
                ),
                if (_badge(id) case final badge?) ...[
                  const SizedBox(width: 8),
                  _Badge(label: badge, highlighted: selected),
                ],
                const Spacer(),
                if (selected)
                  Icon(Icons.check_circle, size: 17, color: t.primary),
              ],
            ),
            const SizedBox(height: 3),
            Text(_describe(id), style: t.meta(size: 10)),
            const SizedBox(height: 9),
            ThemePreview(id: id),
          ],
        ),
      ),
    );
  }
}

/// The small DEFAULT / NEW pill.
class _Badge extends StatelessWidget {
  final String label;
  final bool highlighted;

  const _Badge({required this.label, required this.highlighted});

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final color = highlighted ? t.primary : t.textFaint;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(t.pillRadius),
        border: Border.all(color: color.withValues(alpha: 0.45)),
      ),
      child: Text(
        label,
        style: t.meta(
          size: 8.5,
          weight: FontWeight.w700,
          color: color,
          letterSpacing: 1,
        ),
      ),
    );
  }
}

/// A miniature of a theme, ~[_previewHeight] tall.
///
/// **This is the one widget in the app that must not use
/// `CommanderTokens.of(context)`.** Its whole job is to show what a theme the
/// user has *not* selected looks like, so every colour and measurement comes
/// from `id.tokens` — the previewed theme's own token set — rather than the
/// active one. Everything around it (the card, the label, the badge) still
/// reads the active theme as normal.
///
/// It paints an abstraction rather than embedding a real page: a shrunken copy
/// of the session list would need a store, a snapshot and a live connection,
/// and a screenshot would go stale the moment a token changed. Bars and blocks
/// drawn from the tokens cannot drift from the theme they describe.
class ThemePreview extends StatelessWidget {
  final ThemeId id;

  const ThemePreview({super.key, required this.id});

  @override
  Widget build(BuildContext context) {
    final p = id.tokens;
    return ClipRRect(
      borderRadius: BorderRadius.circular(p.cardRadius),
      child: SizedBox(
        height: _previewHeight,
        child: Container(
          color: p.canvas,
          padding: const EdgeInsets.all(7),
          // Structure, not just colour, is what distinguishes the themes, so
          // each gets the shape the deck's picker shows: rounded cards on
          // near-black against an elbow rail beside top-ruled panels.
          child: switch (p.chrome) {
            ChromeKind.missionControl => _missionControl(p),
            ChromeKind.lcars => _lcars(p),
          },
        ),
      ),
    );
  }

  /// Mission Control: a thin header strip over two rounded, bordered cards,
  /// each with a leading state dot — one neutral, one wanting attention.
  Widget _missionControl(CommanderTokens p) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Row(
        children: [
          _dot(p.primary, 7),
          const SizedBox(width: 5),
          _bar(p.text, width: 34),
          const Spacer(),
          _bar(p.primary, width: 16),
        ],
      ),
      const SizedBox(height: 7),
      Expanded(
        child: _mcCard(p, accent: p.working, fill: p.surface),
      ),
      const SizedBox(height: 5),
      Expanded(
        child: _mcCard(
          p,
          accent: p.attention,
          fill: p.attention.withValues(alpha: 0.1),
        ),
      ),
    ],
  );

  Widget _mcCard(
    CommanderTokens p, {
    required Color accent,
    required Color fill,
  }) => Container(
    padding: const EdgeInsets.symmetric(horizontal: 7),
    decoration: BoxDecoration(
      color: fill,
      borderRadius: BorderRadius.circular(p.cardRadius),
      border: Border.all(color: accent.withValues(alpha: 0.4)),
    ),
    child: Row(
      children: [
        _dot(accent, 6),
        const SizedBox(width: 6),
        Expanded(
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _bar(p.text, width: 52),
              const SizedBox(height: 4),
              _bar(p.textMuted, width: 30, height: 3),
            ],
          ),
        ),
      ],
    ),
  );

  /// LCARS: the elbow rail as a left column of amber / lilac / periwinkle /
  /// salmon blocks, beside two panels whose only border is a coloured top rule.
  Widget _lcars(CommanderTokens p) => Row(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      SizedBox(
        width: 20,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // The rail's rounded outer corners, at the elbow radius scaled to
            // this preview — the full 32 would swallow a 20px-wide block.
            Expanded(flex: 3, child: _railBlock(p, p.primary, top: true)),
            const SizedBox(height: 4),
            Expanded(flex: 2, child: _railBlock(p, p.nav)),
            const SizedBox(height: 4),
            Expanded(flex: 2, child: _railBlock(p, p.info)),
            const SizedBox(height: 4),
            Expanded(flex: 4, child: _railBlock(p, p.attention, bottom: true)),
          ],
        ),
      ),
      const SizedBox(width: 6),
      Expanded(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(child: _lcarsPanel(p, p.primary)),
            const SizedBox(height: 5),
            Expanded(child: _lcarsPanel(p, p.nav)),
          ],
        ),
      ),
    ],
  );

  Widget _railBlock(
    CommanderTokens p,
    Color color, {
    bool top = false,
    bool bottom = false,
  }) {
    final r = Radius.circular(p.elbowRadius * 0.25);
    return Container(
      decoration: BoxDecoration(
        color: color,
        borderRadius: BorderRadius.only(
          topLeft: top ? r : Radius.zero,
          bottomLeft: bottom ? r : Radius.zero,
        ),
      ),
    );
  }

  Widget _lcarsPanel(CommanderTokens p, Color accent) => Container(
    padding: const EdgeInsets.symmetric(horizontal: 6),
    decoration: BoxDecoration(
      color: p.surface,
      border: Border(
        top: BorderSide(color: accent, width: p.panelTopBorder),
      ),
    ),
    child: Column(
      mainAxisAlignment: MainAxisAlignment.center,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _bar(p.text, width: 56),
        const SizedBox(height: 4),
        _bar(p.textMuted, width: 34, height: 3),
      ],
    ),
  );

  Widget _dot(Color color, double size) => Container(
    width: size,
    height: size,
    decoration: BoxDecoration(color: color, shape: BoxShape.circle),
  );

  /// A stand-in for a line of text.
  Widget _bar(Color color, {required double width, double height = 4}) =>
      Container(
        width: width,
        height: height,
        decoration: BoxDecoration(
          color: color.withValues(alpha: 0.8),
          borderRadius: BorderRadius.circular(height / 2),
        ),
      );
}
