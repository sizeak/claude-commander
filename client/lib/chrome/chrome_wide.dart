/// The desktop/tablet shell and its workspace frame, for both themes.
///
/// Dispatch goes through the [Chrome] interface like every other element in this
/// layer: `buildWide` and `buildWideDetail` are declared on [ChromeForms] and
/// implemented by each chrome, which returns the matching variant from this file.
///
/// The two variants live together here, rather than one in `mission_control/` and
/// one in `lcars/`, because the thing they have in common is the *layout problem* —
/// how many columns the width affords, and which pane owns the navigation. Reading
/// them side by side is what makes the 1180px threshold and the folded-nav
/// fallback comprehensible; split across two files, each half looks arbitrary.
library;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../theme/tokens.dart';
import '../widgets/brand_mark.dart';
import 'chrome.dart';
import 'chrome_forms.dart';
import 'lcars/bleed.dart';
import 'lcars/elbow.dart';

/// Logical width at or above which LCARS gets its **third** column — the elbow
/// nav rail as a column of its own, separate from the fleet list.
///
/// Higher than the wide/narrow breakpoint on purpose. The two LCARS side columns
/// are fixed at [_lcarsNavWidth] + [_lcarsFleetWidth] = 394px, so at 900px the
/// workspace would be left ~500px, where the deck's workspace frames assume
/// 1120px+. Between the wide breakpoint and this one, LCARS folds the nav into
/// the fleet column instead (see [LcarsWide]).
const double kLcarsThreeColumnWidth = 1180;

/// The wide shell's structure, described rather than laid out.
///
/// Deliberately **not** a pair of column widths: the two themes disagree about
/// how many columns there are. Mission Control has two (fleet rail + workspace)
/// with the mode toggle and the actions in the rail's footer; LCARS has three
/// above [kLcarsThreeColumnWidth], the first being an elbow nav rail that needs
/// the very same live data the Mission Control footer does. So the nav's inputs
/// are carried as *data and callbacks* — [modes], [needsInputCount],
/// [newSession], [settings], and the three counts — and each branch letters and
/// places them itself.
@immutable
class ChromeWideSpec {
  /// The fleet pane's body (the shared session list).
  final Widget fleetList;

  /// The detail/workspace pane.
  final Widget workspace;

  /// The Fleet / Activity destinations the shell's nav drives.
  final List<ChromeNavItem> modes;

  /// How many sessions are asking for a human answer. LCARS gives this its own
  /// `INPUT nn` nav block, coloured [CommanderTokens.attention] when non-zero.
  final int needsInputCount;

  /// Cross-server session counts for the fleet pane's header line.
  final int activeCount;
  final int totalCount;
  final int serverCount;

  final ChromeButtonAction? newSession;
  final ChromeButtonAction? settings;

  const ChromeWideSpec({
    required this.fleetList,
    required this.workspace,
    required this.modes,
    required this.needsInputCount,
    required this.activeCount,
    required this.totalCount,
    required this.serverCount,
    this.newSession,
    this.settings,
  });
}

/// The wide shell, framed for the active theme. Owns the `Scaffold` in both
/// themes, for the same reason [Chrome] does: `ScaffoldMessenger`, bottom sheets
/// and dialogs must keep working regardless of theme.
class ChromeWide extends StatelessWidget {
  final ChromeWideSpec spec;
  const ChromeWide(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildWide(context, spec);
}

/// One tab of the workspace pane. Carries its own [Key] so the key travels with
/// the tab rather than being re-derived by each theme's renderer.
@immutable
class ChromeWideTab {
  final Key tabKey;
  final String label;

  const ChromeWideTab({required this.tabKey, required this.label});
}

/// The workspace pane's frame: an identity header, a tab strip, and the active
/// tab's body.
///
/// Described rather than pre-built because the tab strip is *structurally*
/// different per theme, not just differently coloured: Mission Control puts a
/// horizontal underline row between the header and the body, while LCARS stands
/// the tabs up as a column of elbow blocks alongside the body.
@immutable
class ChromeWideDetailSpec {
  /// The session's state glyph (already tone-coloured, so both themes use it).
  final Widget glyph;

  final String title;

  /// The metadata line under the title ("genio · fix/auth · claude · local").
  final String? meta;

  /// A trailing badge beside the title (the PR chip).
  final Widget? badge;

  final ChromeButtonAction? refresh;

  final List<ChromeWideTab> tabs;
  final int selected;
  final ValueChanged<int> onSelect;

  /// The selected tab's body.
  final Widget content;

  const ChromeWideDetailSpec({
    required this.glyph,
    required this.title,
    this.meta,
    this.badge,
    this.refresh,
    required this.tabs,
    required this.selected,
    required this.onSelect,
    required this.content,
  });
}

/// The workspace pane, framed for the active theme. See [ChromeWideDetailSpec].
class ChromeWideDetail extends StatelessWidget {
  final ChromeWideDetailSpec spec;
  const ChromeWideDetail(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildWideDetail(context, spec);
}

/// The fleet pane's count line, formatted once so the two themes cannot drift on
/// pluralisation. LCARS uppercases it; Mission Control prints it verbatim, which
/// is the string the rail header has always shown.
String _countsLine(ChromeWideSpec spec) =>
    '${spec.activeCount} active · ${spec.totalCount} total · '
    '${spec.serverCount} server${spec.serverCount == 1 ? '' : 's'}';

// ── Mission Control ──────────────────────────────────────────────────────────

/// Mission Control's two panes: the fleet rail and the workspace, split by a
/// hairline. Unchanged from the hand-built layout this replaced.
class MissionControlWide extends StatelessWidget {
  final ChromeWideSpec spec;
  const MissionControlWide(this.spec, {super.key});

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Scaffold(
      body: SafeArea(
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            SizedBox(width: _mcRailWidth, child: _rail(t)),
            VerticalDivider(width: 1, color: t.borderSubtle),
            Expanded(child: spec.workspace),
          ],
        ),
      ),
    );
  }

  /// The persistent Fleet rail: a branded header (brand mark + "Fleet" + mono
  /// counts), the shared session list, and a footer carrying the
  /// FLEET/ACTIVITY toggle, settings, and new-session.
  Widget _rail(CommanderTokens t) => Container(
    color: t.canvas,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _header(t),
        Expanded(child: spec.fleetList),
        _footer(t),
      ],
    ),
  );

  Widget _header(CommanderTokens t) => Padding(
    padding: const EdgeInsets.fromLTRB(16, 18, 16, 12),
    child: Row(
      children: [
        const BrandMark(size: 30),
        const SizedBox(width: 11),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'Fleet',
                style: TextStyle(
                  fontSize: 19,
                  fontWeight: FontWeight.w700,
                  letterSpacing: -0.4,
                  color: t.text,
                ),
              ),
              const SizedBox(height: 1),
              Text(_countsLine(spec), style: t.meta(size: 10)),
            ],
          ),
        ),
      ],
    ),
  );

  Widget _footer(CommanderTokens t) {
    final settings = spec.settings;
    final newSession = spec.newSession;
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: t.borderSubtle)),
      ),
      child: Row(
        children: [
          for (var i = 0; i < spec.modes.length; i++) ...[
            if (i > 0) const SizedBox(width: 14),
            _ModeToggle(spec.modes[i]),
          ],
          const Spacer(),
          if (settings != null) ...[
            _McIconButton(settings),
            const SizedBox(width: 8),
          ],
          if (newSession != null) _McIconButton(newSession),
        ],
      ),
    );
  }
}

/// One FLEET/ACTIVITY footer toggle: the deck's glyph + mono label, tinted
/// accent when active and muted otherwise.
class _ModeToggle extends StatelessWidget {
  final ChromeNavItem item;
  const _ModeToggle(this.item);

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final color = item.selected ? t.nav : t.textFaint;
    return InkWell(
      onTap: item.onTap,
      borderRadius: BorderRadius.circular(6),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 4),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              item.glyph,
              style: TextStyle(fontSize: 14, color: color, height: 1),
            ),
            const SizedBox(width: 6),
            Text(
              item.label,
              style: t.meta(size: 10, weight: FontWeight.w600, color: color),
            ),
          ],
        ),
      ),
    );
  }
}

/// A rounded-square icon button for the rail footer (settings ⚙, new-session +)
/// and the workspace header (refresh). A [ChromeActionKind.primary] action fills
/// with the primary role; anything else is a bordered surface.
class _McIconButton extends StatelessWidget {
  final ChromeButtonAction action;
  const _McIconButton(this.action);

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final accent = action.kind == ChromeActionKind.primary;
    return Tooltip(
      message: action.label,
      child: InkWell(
        onTap: action.onPressed,
        borderRadius: BorderRadius.circular(9),
        child: Container(
          width: 32,
          height: 32,
          decoration: BoxDecoration(
            color: accent ? t.primary : t.surface,
            borderRadius: BorderRadius.circular(9),
            border: accent ? null : Border.all(color: t.border),
          ),
          child: Icon(
            action.icon,
            size: 17,
            color: accent ? t.canvas : t.textMuted,
          ),
        ),
      ),
    );
  }
}

/// Mission Control's workspace: header, the underline tab row, then the body.
class MissionControlDetail extends StatelessWidget {
  final ChromeWideDetailSpec spec;
  const MissionControlDetail(this.spec, {super.key});

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Container(
      color: t.canvas,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _header(t),
          _tabs(t),
          Expanded(child: spec.content),
        ],
      ),
    );
  }

  Widget _header(CommanderTokens t) {
    final meta = spec.meta;
    final badge = spec.badge;
    final refresh = spec.refresh;
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 12, 12),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.divider)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    spec.glyph,
                    const SizedBox(width: 6),
                    Flexible(
                      child: Text(
                        spec.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 17,
                          fontWeight: FontWeight.w700,
                          letterSpacing: -0.2,
                          color: t.text,
                        ),
                      ),
                    ),
                    if (badge != null) ...[const SizedBox(width: 9), badge],
                  ],
                ),
                if (meta != null) ...[
                  const SizedBox(height: 5),
                  Text(
                    meta,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: t.meta(size: 11),
                  ),
                ],
              ],
            ),
          ),
          if (refresh != null) ...[
            const SizedBox(width: 12),
            _McIconButton(refresh),
          ],
        ],
      ),
    );
  }

  /// The deck's underline tab row: each tab is a mono label; the active one is
  /// bright with a 2px accent underline.
  Widget _tabs(CommanderTokens t) => Container(
    padding: const EdgeInsets.fromLTRB(20, 10, 20, 0),
    decoration: BoxDecoration(
      border: Border(bottom: BorderSide(color: t.divider)),
    ),
    child: Row(
      children: [for (var i = 0; i < spec.tabs.length; i++) _tabItem(t, i)],
    ),
  );

  Widget _tabItem(CommanderTokens t, int i) {
    final tab = spec.tabs[i];
    final selected = spec.selected == i;
    return Padding(
      padding: const EdgeInsets.only(right: 22),
      child: InkWell(
        key: tab.tabKey,
        onTap: () => spec.onSelect(i),
        child: Container(
          padding: const EdgeInsets.only(bottom: 11),
          decoration: BoxDecoration(
            border: Border(
              bottom: BorderSide(
                color: selected ? t.nav : Colors.transparent,
                width: 2,
              ),
            ),
          ),
          child: Text(
            tab.label,
            style: t.meta(
              size: 12,
              weight: FontWeight.w600,
              color: selected ? t.text : t.textMuted,
            ),
          ),
        ),
      ),
    );
  }
}

// ── LCARS ────────────────────────────────────────────────────────────────────

/// LCARS' wide shell: an elbow nav rail, the fleet column, then the workspace.
///
/// Three columns above [kLcarsThreeColumnWidth]. Below it — but still wide
/// enough for the two-pane shell — the nav rail folds into the fleet column as a
/// horizontal run of blocks under the list, so the workspace keeps its width
/// rather than paying 104px for a rail.
class LcarsWide extends StatelessWidget {
  final ChromeWideSpec spec;
  const LcarsWide(this.spec, {super.key});

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    // Read above the Scaffold, as the phone frames do, so whether a Scaffold
    // republishes its body's padding never has to be assumed.
    final insets = MediaQuery.paddingOf(context);
    final bleed = EdgeInsets.only(top: insets.top, bottom: insets.bottom);
    // Outside the Scaffold on purpose: this must measure the same box the
    // wide/narrow breakpoint did, and holding the display cutout inside the
    // Scaffold would shave it off the width first.
    return LayoutBuilder(
      builder: (context, constraints) {
        final three = constraints.maxWidth >= kLcarsThreeColumnWidth;
        return AnnotatedRegion<SystemUiOverlayStyle>(
          value: lcarsSystemBars,
          child: Scaffold(
            backgroundColor: t.canvas,
            // The vertical insets are bled into by the frame's own blocks
            // rather than held off by a `SafeArea`; the horizontal ones are
            // still held, because a cutout is an occlusion and not a bezel to
            // decorate. Same split as the phone frames.
            body: Padding(
              padding: EdgeInsets.only(left: insets.left, right: insets.right),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (three) ...[
                    SizedBox(
                      key: const ValueKey('wide-nav'),
                      width: _lcarsNavWidth,
                      child: _nav(t, bleed),
                    ),
                    // Both columns beside this gap bleed into the band, so it
                    // is filled across it — see [lcarsBandSeam]. Its fill ends
                    // level with the fleet cap, the shorter of the two blocks
                    // it bridges.
                    lcarsBandSeam(
                      width: _lcarsGap,
                      height: elbowCapHeight(
                        kElbowCapHeight,
                        EdgeInsets.only(top: bleed.top),
                      ),
                      color: t.nav,
                      bleed: bleed,
                    ),
                  ],
                  // Keyed so crossing kLcarsThreeColumnWidth — which inserts two
                  // children ahead of these — moves their elements rather than
                  // re-inflating them. Unkeyed, the fold would reset the session
                  // list's search text and filters on a window resize.
                  SizedBox(
                    key: const ValueKey('wide-fleet'),
                    width: _lcarsFleetWidth,
                    child: _fleet(t, bleed, folded: !three),
                  ),
                  // Plain, not a seam: the workspace holds its insets, so the
                  // band ends at the fleet column's right edge and there is
                  // nothing on this side to bridge to.
                  const SizedBox(width: _lcarsGap),
                  Expanded(
                    key: const ValueKey('wide-workspace'),
                    // Held, not bled. The workspace opens with a plain text
                    // header and closes with page content — neither is an LCARS
                    // block, so there is nothing up there to carry a band and
                    // nothing down there that should sit under the gesture
                    // strip.
                    child: Padding(
                      padding: EdgeInsets.only(
                        top: bleed.top,
                        bottom: bleed.bottom,
                      ),
                      child: spec.workspace,
                    ),
                  ),
                  // No trailing margin: the frame runs flush to the right bezel,
                  // the last LCARS surface that stopped short of it.
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  /// Deck frame L1's nav rail, read top to bottom: the `CMDR` identity block,
  /// the mode destinations, the live needs-input count, inert filler that
  /// absorbs the slack, the new-session block, and a closing settings elbow. Only
  /// the first and last blocks are rounded, so the column reads as one bracket.
  Widget _nav(CommanderTokens t, EdgeInsets bleed) {
    final newSession = spec.newSession;
    final settings = spec.settings;
    final blocks = <Widget>[
      ChromeElbow(
        bleed: EdgeInsets.only(top: bleed.top),
        color: t.nav,
        corner: ElbowCorner.topLeft,
        height: 74,
        label: 'CMDR',
        labelAlignment: Alignment.bottomRight,
        labelSize: 12,
        labelWeight: FontWeight.w700,
      ),
      for (final mode in spec.modes)
        ChromeElbow(
          color: mode.selected ? t.primary : t.borderSubtle,
          labelColor: mode.selected ? t.canvas : t.nav,
          height: _lcarsNavBlock,
          label: t.caseLabel(mode.label),
          onTap: mode.onTap,
        ),
      _inputBlock(t),
      // Two-tone inert filler, brightest first — the deck's rails always step
      // down through a thin bright band into a large dark one.
      ChromeElbow(color: t.borderSubtle, height: 16),
      Expanded(child: ChromeElbow(color: t.divider)),
      if (newSession != null)
        ChromeElbow(
          color: t.info,
          height: 34,
          label: t.caseLabel(newSession.label),
          onTap: newSession.onPressed,
        ),
      ChromeElbow(
        bleed: EdgeInsets.only(bottom: bleed.bottom),
        color: t.nav,
        corner: ElbowCorner.bottomLeft,
        height: 44,
        label: settings == null ? null : t.caseLabel(settings.label),
        labelAlignment: Alignment.topRight,
        onTap: settings?.onPressed,
      ),
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (var i = 0; i < blocks.length; i++) ...[
          if (i > 0) const SizedBox(height: _lcarsGap),
          blocks[i],
        ],
      ],
    );
  }

  /// The live needs-input count as a nav block: [CommanderTokens.attention] while
  /// anything is waiting on a human, and the inert dark fill otherwise, so the
  /// rail itself carries the one number the user has to act on.
  Widget _inputBlock(CommanderTokens t) {
    final waiting = spec.needsInputCount > 0;
    return ChromeElbow(
      color: waiting ? t.attention : t.borderSubtle,
      labelColor: waiting ? t.canvas : t.nav,
      height: 26,
      label: 'INPUT ${spec.needsInputCount.toString().padLeft(2, '0')}',
    );
  }

  /// The fleet column: an elbow cap, the FLEET title and its count line, the
  /// list, and — when the nav has folded in — a horizontal run of nav blocks
  /// beneath it.
  Widget _fleet(CommanderTokens t, EdgeInsets bleed, {required bool folded}) {
    final counts = t.caseLabel(_countsLine(spec));
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ChromeElbowCap(
          bleed: EdgeInsets.only(top: bleed.top),
          color: t.nav,
        ),
        const SizedBox(height: 7),
        Text('FLEET', style: t.display(size: 22)),
        Text(
          // With no nav column there is no INPUT block, so the count that needs
          // acting on rides along with the rest of the fleet's numbers.
          folded && spec.needsInputCount > 0
              ? '$counts · ${spec.needsInputCount} INPUT'
              : counts,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: _caption(t, t.nav),
        ),
        const SizedBox(height: 9),
        Expanded(child: spec.fleetList),
        // Folded, the nav run is the column's closing block and bleeds into the
        // gesture strip; unfolded, the list itself is the bottom, and a
        // scrollable running under that strip is a regression rather than a
        // feature — so the column holds the inset instead.
        if (folded) ...[
          const SizedBox(height: 6),
          _foldedNav(t, bleed),
        ] else
          SizedBox(height: bleed.bottom),
      ],
    );
  }

  /// The nav rail folded flat: the destinations and the two actions as one
  /// contiguous run of blocks, pill-ended on the outside only — the same shape
  /// the phone shell's footer uses, so the fold reads as the same control.
  Widget _foldedNav(CommanderTokens t, EdgeInsets bleed) {
    final actions = [?spec.newSession, ?spec.settings];
    final count = spec.modes.length + actions.length;
    final blocks = <Widget>[
      for (var i = 0; i < spec.modes.length; i++)
        Expanded(
          child: _foldedBlock(
            t,
            label: t.caseLabel(spec.modes[i].label),
            fill: spec.modes[i].selected ? t.primary : t.borderSubtle,
            ink: spec.modes[i].selected ? t.canvas : t.nav,
            onTap: spec.modes[i].onTap,
            radius: _runEnds(i, count, t.pillRadius),
            bleed: bleed,
          ),
        ),
      for (var i = 0; i < actions.length; i++)
        Expanded(
          child: _foldedBlock(
            t,
            label: t.caseLabel(actions[i].label),
            fill: actions[i].kind == ChromeActionKind.primary
                ? t.info
                : t.borderSubtle,
            ink: actions[i].kind == ChromeActionKind.primary ? t.canvas : t.nav,
            onTap: actions[i].onPressed,
            radius: _runEnds(spec.modes.length + i, count, t.pillRadius),
            bleed: bleed,
          ),
        ),
    ];
    return Row(
      children: [
        for (var i = 0; i < blocks.length; i++) ...[
          if (i > 0) const SizedBox(width: _lcarsSeam),
          blocks[i],
        ],
      ],
    );
  }

  Widget _foldedBlock(
    CommanderTokens t, {
    required String label,
    required Color fill,
    required Color ink,
    required BorderRadius radius,
    VoidCallback? onTap,
    EdgeInsets bleed = EdgeInsets.zero,
  }) => ClipRRect(
    // An elbow's single rounded corner is the wrong shape for the end of a
    // horizontal run, which is a pill, so the radius is clipped around the
    // primitive rather than coming from ElbowCorner.
    borderRadius: radius,
    child: ChromeElbow(
      bleed: EdgeInsets.only(bottom: bleed.bottom),
      color: fill,
      labelColor: ink,
      height: 32,
      label: label,
      labelAlignment: Alignment.center,
      labelSize: 11,
      labelWeight: FontWeight.w700,
      onTap: onTap,
    ),
  );
}

/// LCARS' workspace: the title header, then the body with the tabs stood up as a
/// column of elbow blocks alongside it.
///
/// The tabs are a column and not a row because an underline row is a Mission
/// Control shape — LCARS marks selection by *filling* a block, and a filled block
/// wants to be part of a bracket down one edge. It sits on the right so the
/// workspace is framed on the side the shell's nav rail does not already occupy.
class LcarsDetail extends StatelessWidget {
  final ChromeWideDetailSpec spec;
  const LcarsDetail(this.spec, {super.key});

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Container(
      color: t.canvas,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _header(t),
          Expanded(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // The tab column sits *inboard* of the content, between the
                // fleet list and the workspace — that is where the design deck's
                // landscape frames put it, and it keeps the rail-then-content
                // reading order consistent with the outer nav rail. Emulator
                // capture caught it pinned to the far right edge.
                SizedBox(width: _lcarsTabWidth, child: _tabs(t)),
                const SizedBox(width: _lcarsGap),
                Expanded(child: spec.content),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _header(CommanderTokens t) {
    final meta = spec.meta;
    final badge = spec.badge;
    final refresh = spec.refresh;
    return Padding(
      padding: const EdgeInsets.fromLTRB(0, 4, 0, 9),
      child: Row(
        children: [
          spec.glyph,
          const SizedBox(width: 6),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Flexible(
                      child: MediaQuery.withClampedTextScaling(
                        maxScaleFactor: 1.5,
                        child: Text(
                          spec.title.toUpperCase(),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: t.display(size: 22),
                        ),
                      ),
                    ),
                    if (badge != null) ...[const SizedBox(width: 9), badge],
                  ],
                ),
                if (meta != null)
                  Text(
                    meta.toUpperCase(),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: _caption(t, t.nav, letterSpacing: 1.1),
                  ),
              ],
            ),
          ),
          if (refresh != null) ...[
            const SizedBox(width: 12),
            SizedBox(
              width: _lcarsTabWidth,
              child: ChromeElbow(
                color: t.borderSubtle,
                labelColor: t.nav,
                height: 26,
                label: t.caseLabel(refresh.label),
                labelAlignment: Alignment.center,
                onTap: refresh.onPressed,
              ),
            ),
          ],
        ],
      ),
    );
  }

  /// The tab column: one block per tab — the selected one filled amber with
  /// near-black lettering, the rest the dark inert fill with lilac — over filler
  /// that carries the bracket down to the bottom of the pane.
  Widget _tabs(CommanderTokens t) {
    final blocks = <Widget>[
      for (var i = 0; i < spec.tabs.length; i++)
        ChromeElbow(
          key: spec.tabs[i].tabKey,
          color: spec.selected == i ? t.primary : t.borderSubtle,
          labelColor: spec.selected == i ? t.canvas : t.nav,
          corner: i == 0 ? ElbowCorner.topRight : ElbowCorner.none,
          height: 34,
          label: t.caseLabel(spec.tabs[i].label),
          // Inboard edge: the labels face the content they switch.
          labelAlignment: Alignment.centerLeft,
          labelWeight: FontWeight.w700,
          onTap: () => spec.onSelect(i),
        ),
      ChromeElbow(color: t.borderSubtle, height: 16),
      Expanded(
        child: ChromeElbow(color: t.divider, corner: ElbowCorner.bottomRight),
      ),
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (var i = 0; i < blocks.length; i++) ...[
          if (i > 0) const SizedBox(height: _lcarsGap),
          blocks[i],
        ],
      ],
    );
  }
}

// ── Local geometry ───────────────────────────────────────────────────────────

/// Mission Control's fleet rail. The width the two-pane layout has always used.
const _mcRailWidth = 312.0;

/// LCARS' nav column (deck frame L1's rail on a landscape deck — wider than the
/// portrait [CommanderTokens.railWidth], which has no room for lettering).
const _lcarsNavWidth = 104.0;

/// LCARS' fleet column.
const _lcarsFleetWidth = 290.0;

/// The gap between LCARS blocks and columns. The deck's rails are 5px apart, and
/// the columns sit on the same rhythm.
const _lcarsGap = 5.0;

/// The gap inside a contiguous horizontal run of LCARS blocks — tighter than
/// [_lcarsGap], because a seam should read as a join rather than a separation.
const _lcarsSeam = 4.0;

/// A nav destination block's height.
const _lcarsNavBlock = 30.0;

/// The workspace tab column, which also sizes the header's refresh block so the
/// two line up down the pane's right edge.
const _lcarsTabWidth = 96.0;

/// The recurring 11px condensed caption. Antonio rather than
/// [CommanderTokens.mono] — this is chrome, and only real agent output stays
/// monospace.
TextStyle _caption(
  CommanderTokens t,
  Color color, {
  double letterSpacing = 0,
}) => TextStyle(
  fontFamily: t.sans,
  fontSize: 11,
  fontWeight: FontWeight.w500,
  letterSpacing: letterSpacing,
  color: color,
);

/// Rounds the outer ends of a horizontal run of [count] blocks: block [i] gets a
/// [radius] corner on its left when it is first and on its right when it is last,
/// so a run of any length reads as one bracketed unit.
BorderRadius _runEnds(int i, int count, double radius) {
  final r = Radius.circular(radius);
  return BorderRadius.only(
    topLeft: i == 0 ? r : Radius.zero,
    bottomLeft: i == 0 ? r : Radius.zero,
    topRight: i == count - 1 ? r : Radius.zero,
    bottomRight: i == count - 1 ? r : Radius.zero,
  );
}
