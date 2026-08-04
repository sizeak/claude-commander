import 'package:flutter/material.dart';

import '../../theme/tokens.dart';
import '../../widgets/brand_mark.dart';
import '../chrome.dart';
import '../chrome_forms.dart';
import '../chrome_wide.dart';

/// The default chrome: a Material `Scaffold` with an `AppBar`, app-bar actions,
/// and a docked `FloatingActionButton` for the primary action.
///
/// A faithful reproduction of what every page built by hand before the chrome
/// layer existed. Anything here that looks oddly specific — the absence of a
/// `SafeArea` on app-bar'd pages, `overflow: TextOverflow.ellipsis` on the title
/// — is preserving existing behaviour, not a fresh choice.
class MissionControlChrome extends Chrome {
  const MissionControlChrome();

  @override
  Widget buildPage(BuildContext context, ChromePageSpec spec) {
    final t = CommanderTokens.of(context);
    return Scaffold(
      appBar: spec.title == null ? null : _appBar(context, spec, t),
      // Only the terminal opts out, and it does so to keep the remote PTY from
      // seeing a resize when the keyboard opens.
      resizeToAvoidBottomInset: spec.insets != ChromeInsets.pan,
      body: applyChromeInsets(spec.insets, spec.body),
      floatingActionButton: _fab(context, spec, t),
    );
  }

  PreferredSizeWidget _appBar(
    BuildContext context,
    ChromePageSpec spec,
    CommanderTokens t,
  ) {
    final subtitle = spec.subtitle;
    return AppBar(
      // `automaticallyImplyLeading` already resolves to canPop, so an explicit
      // false is only needed when a page suppresses the back button itself.
      automaticallyImplyLeading: shouldShowBack(context, spec),
      title: subtitle == null
          ? Text(spec.title!, overflow: TextOverflow.ellipsis)
          : Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(spec.title!, overflow: TextOverflow.ellipsis),
                Text(
                  subtitle,
                  overflow: TextOverflow.ellipsis,
                  style: t.meta(size: 10.5),
                ),
              ],
            ),
      actions: [for (final a in spec.actions) _action(context, a, t)],
    );
  }

  Widget _action(BuildContext context, ChromeAction a, CommanderTokens t) =>
      switch (a) {
        ChromeButtonAction(:final onPressed) => IconButton(
          icon: Icon(a.icon),
          tooltip: a.label,
          color: a.kind == ChromeActionKind.destructive ? t.danger : null,
          onPressed: onPressed,
        ),
        ChromeMenuAction(:final items) => PopupMenuButton<int>(
          icon: Icon(a.icon),
          tooltip: a.label,
          position: PopupMenuPosition.under,
          onSelected: (i) => items[i].onSelected?.call(),
          itemBuilder: (context) => [
            for (var i = 0; i < items.length; i++)
              PopupMenuItem(
                value: i,
                enabled: items[i].enabled,
                child: Text(items[i].label),
              ),
          ],
        ),
      };

  Widget? _fab(BuildContext context, ChromePageSpec spec, CommanderTokens t) {
    final action = spec.primaryAction;
    if (action == null) return null;
    // A menu as the *primary* action has no sensible FAB rendering, and no page
    // asks for one. Kept explicit so a future caller gets an error rather than a
    // silently dropped action.
    if (action is! ChromeButtonAction) {
      throw UnsupportedError(
        'Mission Control renders primaryAction as a FloatingActionButton, which '
        'cannot host a menu. Pass a ChromeButtonAction, or put the menu in '
        'actions instead.',
      );
    }
    // Deliberately uncoloured: the manager pages' FABs never set colours, so
    // Material 3 gave them `primaryContainer`/`onPrimaryContainer` — the slate
    // tile, not a violet one. Forcing `t.primary` here turned all three violet.
    // The phone shell's docked FAB *was* explicitly violet and sets its own in
    // [buildShell].
    return FloatingActionButton(
      onPressed: action.onPressed,
      tooltip: action.label,
      child: Icon(action.icon),
    );
  }

  // ── Forms ──────────────────────────────────────────────────────────────────
  //
  // Everything below reproduces a widget that already exists in the app, named
  // in each doc comment, because pages are being migrated onto these and Mission
  // Control must not change. The geometry literals (10, 9, 7, 62, 64…) are those
  // widgets' own values, not the radius tokens: `cardRadius` is 13, and swapping
  // it in here would retune rows and bars that are pinned by screenshots.
  //
  // `ChromeListRowSpec.number` and `ChromeListRowSpec.position` are LCARS-only
  // and deliberately ignored: Mission Control has no numbered leading block, and
  // its rows are individually carded or divider-ruled rather than forming a
  // bracketed run whose outer corners need rounding.

  @override
  Widget buildListRow(BuildContext context, ChromeListRowSpec spec) {
    final t = CommanderTokens.of(context);
    // The app has two row shapes and the spec carries no flag to choose between
    // them, so the subtitle decides: `_GroupedTile` (carded, single line) never
    // has one, `_RecentTile` (divider-ruled, two lines) always does. That way
    // both migrate onto this method unchanged.
    final row = spec.subtitle == null
        ? _cardedRow(context, spec, t)
        : _ruledRow(context, spec, t);
    // 0.6 is what the degraded server row uses — the app's only dimmed row.
    return spec.dimmed ? Opacity(opacity: 0.6, child: row) : row;
  }

  /// `session_list_page.dart`'s `_GroupedTile`: a rounded, bordered card with the
  /// state glyph, the title, and a trailing badge or state word.
  Widget _cardedRow(
    BuildContext context,
    ChromeListRowSpec spec,
    CommanderTokens t,
  ) {
    const radius = 10.0;
    final tone = t.toneStyle(spec.tone);
    final attention = sessionWantsAttention(spec.tone);
    final Color bg, borderColor;
    if (spec.selected) {
      bg = t.primary.withValues(alpha: 0.1);
      borderColor = t.primary.withValues(alpha: 0.5);
    } else if (attention) {
      // `tintedSurface` *is* the tone tint `_GroupedTile` derived by hand
      // (attention at 9%); the border is the accent at 45%.
      bg = tone.tintedSurface;
      borderColor = tone.accent.withValues(alpha: 0.45);
    } else {
      // Every calm tone's `tintedSurface` is the neutral surface, which is what
      // `_GroupedTile` used for a row that is neither selected nor waiting.
      bg = tone.tintedSurface;
      borderColor = t.border;
    }
    final trailing = _trailing(
      spec,
      t.meta(size: 10, color: attention ? tone.onTint : t.textMuted),
    );
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: spec.onTap,
        borderRadius: BorderRadius.circular(radius),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 9),
          decoration: BoxDecoration(
            color: bg,
            borderRadius: BorderRadius.circular(radius),
            border: Border.all(color: borderColor),
          ),
          child: Row(
            children: [
              if (spec.glyph != null) ...[
                spec.glyph!,
                const SizedBox(width: 9),
              ],
              Expanded(
                child: Text(
                  spec.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w600,
                    color: t.text,
                  ),
                ),
              ),
              if (trailing != null) ...[const SizedBox(width: 8), trailing],
            ],
          ),
        ),
      ),
    );
  }

  /// `session_list_page.dart`'s `_RecentTile`: a dense two-line row ruled with a
  /// bottom divider rather than carded, tinting the selected surface.
  Widget _ruledRow(
    BuildContext context,
    ChromeListRowSpec spec,
    CommanderTokens t,
  ) {
    final tone = t.toneStyle(spec.tone);
    final attention = sessionWantsAttention(spec.tone);
    // The recents row's trailing slot is a faint relative age, not a state word.
    final trailing = _trailing(spec, t.meta(size: 10, color: t.textFaint));
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: spec.onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 9),
          decoration: BoxDecoration(
            color: spec.selected ? t.surfaceSelected : null,
            border: Border(bottom: BorderSide(color: t.divider)),
          ),
          child: Row(
            children: [
              if (spec.glyph != null) ...[
                spec.glyph!,
                const SizedBox(width: 10),
              ],
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      spec.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: spec.monoTitle
                          ? t.meta(size: 13, color: t.text)
                          : TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w600,
                              color: t.text,
                            ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      spec.subtitle!,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: t.meta(
                        size: 10,
                        color: attention ? tone.onTint : t.textMuted,
                      ),
                    ),
                  ],
                ),
              ),
              if (trailing != null) ...[const SizedBox(width: 8), trailing],
            ],
          ),
        ),
      ),
    );
  }

  /// A row's trailing slot: the widget if one was given (a PR badge), else the
  /// trailing text in [style], else nothing.
  Widget? _trailing(ChromeListRowSpec spec, TextStyle style) =>
      spec.trailingWidget ??
      (spec.trailing == null ? null : Text(spec.trailing!, style: style));

  @override
  Widget buildPanel(BuildContext context, ChromePanelSpec spec) {
    final t = CommanderTokens.of(context);
    final tone = spec.tone == null ? null : t.toneStyle(spec.tone!);
    final accent = spec.accent ?? tone?.accent;
    // The recurring Mission Control card: tinted (or neutral) fill, one border
    // all round at the card radius. A tone's `tintedSurface` supplies the 9%
    // tint, and 40% the border alpha.
    //
    // Those match the activity feed's needs-you card exactly. They are a hair off
    // two others the hand-built code had drifted to — the session detail waiting
    // hint was 8%/35% at radius 11, the activity card radius 12 — so unifying
    // them here nudges those two by amounts under a pixel and a few percent
    // alpha. Deliberate: the alternative is a per-caller override to preserve
    // drift nobody chose.
    Widget panel = Container(
      padding: spec.padding,
      decoration: BoxDecoration(
        color: tone?.tintedSurface ?? t.surface,
        borderRadius: BorderRadius.circular(t.cardRadius),
        border: Border.all(color: accent?.withValues(alpha: 0.4) ?? t.border),
      ),
      child: spec.child,
    );
    if (spec.onTap != null) {
      panel = Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: spec.onTap,
          borderRadius: BorderRadius.circular(t.cardRadius),
          child: panel,
        ),
      );
    }
    final eyebrow = spec.eyebrow;
    if (eyebrow == null) return panel;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        // Above the box, as the review screen's "FILES CHANGED" sits above its
        // list — not inset into the panel's own padding, which LCARS does with
        // its coloured top border.
        Padding(
          padding: const EdgeInsets.only(left: 2, bottom: 6),
          child: Text(eyebrow, style: t.eyebrow(color: accent)),
        ),
        panel,
      ],
    );
  }

  /// `session_detail_page.dart`'s `_lifecycleBar`: a rule, then a row of cells
  /// 8px apart with destructive actions pushed to the trailing edge.
  ///
  /// The `Spacer()` before Delete is not expressible in [ChromeButtonBarSpec],
  /// so it is derived from [ChromeActionKind.destructive] instead — which is
  /// exactly where the existing bar puts its only destructive action.
  @override
  Widget buildButtonBar(BuildContext context, ChromeButtonBarSpec spec) {
    bool destructive(ChromeBarButton b) =>
        b.kind == ChromeActionKind.destructive;
    final leading = spec.buttons.where((b) => !destructive(b));
    final trailing = spec.buttons.where(destructive);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const Divider(),
        const SizedBox(height: 4),
        Row(
          children: [
            ..._spaced(context, leading),
            if (leading.isNotEmpty && trailing.isNotEmpty) const Spacer(),
            ..._spaced(context, trailing),
          ],
        ),
      ],
    );
  }

  /// [buttons] as bar cells separated by the lifecycle bar's 8px gap, each one
  /// `Expanded` when it asked to take the remaining width.
  List<Widget> _spaced(
    BuildContext context,
    Iterable<ChromeBarButton> buttons,
  ) {
    final out = <Widget>[];
    for (final button in buttons) {
      if (out.isNotEmpty) out.add(const SizedBox(width: 8));
      final cell = _barButton(context, button);
      out.add(button.expand ? Expanded(child: cell) : cell);
    }
    return out;
  }

  /// One bar cell. With an icon it is `_lifecycleAction`'s rounded-square icon
  /// button over a small mono label; without one it is the terminal modifier
  /// bar's flat key pill, which is the only label-only bar the app has.
  Widget _barButton(BuildContext context, ChromeBarButton button) {
    final t = CommanderTokens.of(context);
    final enabled = button.onPressed != null;
    final destructive = button.kind == ChromeActionKind.destructive;
    // An explicit accent wins: `kind` has three values and this bar has five
    // hues (see ChromeBarButton.accent).
    final color =
        button.accent ??
        switch (button.kind) {
          ChromeActionKind.primary => t.primary,
          ChromeActionKind.normal => t.textBright,
          ChromeActionKind.destructive => t.danger,
        };
    final icon = button.icon;
    if (icon == null) return _keyPill(context, button, color, enabled, t);
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          decoration: BoxDecoration(
            color: destructive ? t.danger.withValues(alpha: 0.1) : t.surface,
            borderRadius: BorderRadius.circular(9),
            border: Border.all(
              color: destructive ? t.danger.withValues(alpha: 0.3) : t.border,
            ),
          ),
          child: IconButton(
            onPressed: button.onPressed,
            icon: Icon(icon, size: 18),
            // Fuller than the caption where the caption had to stay short.
            tooltip: button.tooltip ?? button.label,
            color: color,
            disabledColor: t.textDim,
            visualDensity: VisualDensity.compact,
            padding: EdgeInsets.zero,
            constraints: const BoxConstraints(minWidth: 38, minHeight: 38),
          ),
        ),
        const SizedBox(height: 4),
        Text(
          button.label,
          style: t.meta(
            size: 8.5,
            color: enabled ? (destructive ? t.danger : t.textMuted) : t.textDim,
          ),
        ),
      ],
    );
  }

  /// `terminal_page.dart`'s `_ModifierBar._key`: a raised mono chip. The height
  /// is explicit because that bar got 34px from its host strip (48 less 7+7 of
  /// list padding), which a plain `Row` would not reproduce.
  Widget _keyPill(
    BuildContext context,
    ChromeBarButton button,
    Color color,
    bool enabled,
    CommanderTokens t,
  ) => SizedBox(
    height: 34,
    child: Material(
      color: t.surface,
      borderRadius: BorderRadius.circular(7),
      child: InkWell(
        onTap: button.onPressed,
        borderRadius: BorderRadius.circular(7),
        child: Container(
          alignment: Alignment.center,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(7),
            border: Border.all(color: t.border),
          ),
          child: Text(
            button.label,
            style: t.meta(
              size: 10.5,
              weight: FontWeight.w600,
              color: enabled ? color : t.textDim,
            ),
          ),
        ),
      ),
    ),
  );

  /// `phone_shell.dart`'s `BottomAppBar` + docked `FloatingActionButton`.
  ///
  /// The shell got its FAB from `Scaffold.floatingActionButton` with
  /// `FloatingActionButtonLocation.centerDocked`, which no single returned
  /// widget can do, so the overhang is reproduced by hand: a `Stack` that keeps
  /// the bar's own size, plus the FAB pulled up by half its height with clipping
  /// off. See the caveat on hit-testing in [_dockedFab].
  @override
  Widget buildFooterNav(BuildContext context, ChromeFooterNavSpec spec) {
    final t = CommanderTokens.of(context);
    final centre = spec.centreAction;
    // The gap the FAB sits in goes where the shell put it: between the tabs.
    final gapAt = centre == null ? -1 : spec.items.length ~/ 2;
    final bar = BottomAppBar(
      color: t.canvasRaised,
      height: 62,
      padding: EdgeInsets.zero,
      child: SafeArea(
        top: false,
        child: Row(
          children: [
            for (var i = 0; i < spec.items.length; i++) ...[
              if (i == gapAt) const SizedBox(width: 64),
              Expanded(child: _navTab(context, spec.items[i], t)),
            ],
          ],
        ),
      ),
    );
    // Returns the bar alone. `centreAction` only reserves the notch here; the
    // FAB itself is docked by [buildShell] via the Scaffold, which is the only
    // way its overlapping half is tappable.
    return bar;
  }

  /// The centre action as a docked FAB.
  ///
  /// `centerDocked` lays the FAB out with its centre on the bar's top edge, so
  /// a default 56px FAB overhangs by 28. Reproducing that from inside the bar's
  /// own box means the overhanging top half is drawn (clipping is off) but **not
  /// hit-testable**: a `RenderBox` rejects hits outside its bounds before
  /// reaching its children. The bottom half, over the bar, taps normally.
  /// Docking it from the page's `Scaffold` is the only way to get both, which
  /// would mean the footer joining [ChromePageSpec] rather than being a body
  /// widget.
  /// `phone_shell.dart`'s frame: the body over a `BottomAppBar`, with the centre
  /// action docked as a `FloatingActionButton`.
  ///
  /// The FAB is handed to the `Scaffold` rather than stacked on the bar, because
  /// `floatingActionButtonLocation` is what makes its overlapping top half
  /// *tappable* — a negatively-positioned child of the bar would paint in the
  /// right place but never receive the tap, since Flutter does not hit-test
  /// outside a render box's bounds.
  @override
  Widget buildShell(BuildContext context, ChromeShellSpec spec) {
    final t = CommanderTokens.of(context);
    final centre = spec.centreAction;
    return Scaffold(
      body: SafeArea(bottom: false, child: spec.body),
      floatingActionButton: centre == null
          ? null
          : FloatingActionButton(
              onPressed: centre.onPressed,
              backgroundColor: t.primary,
              foregroundColor: t.canvas,
              elevation: 6,
              shape: RoundedRectangleBorder(
                // Left as a literal: LCARS has no FAB at all, so there is no
                // radius worth parameterising.
                borderRadius: BorderRadius.circular(16),
              ),
              tooltip: centre.label,
              child: Icon(centre.icon, size: 26),
            ),
      floatingActionButtonLocation: FloatingActionButtonLocation.centerDocked,
      bottomNavigationBar: buildFooterNav(
        context,
        ChromeFooterNavSpec(items: spec.items, centreAction: centre),
      ),
    );
  }

  /// `phone_shell.dart`'s `_NavTab`: the deck's glyph over a mono uppercase
  /// label, tinted with the nav accent when active and muted otherwise.
  Widget _navTab(BuildContext context, ChromeNavItem item, CommanderTokens t) {
    final color = item.selected ? t.nav : t.textFaint;
    return InkWell(
      onTap: item.onTap,
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            item.glyph,
            style: TextStyle(fontSize: 17, color: color, height: 1),
          ),
          const SizedBox(height: 4),
          Text(
            item.label,
            style: t.meta(size: 9, weight: FontWeight.w600, color: color),
          ),
        ],
      ),
    );
  }

  /// `session_list_page.dart`'s `_segmented` / `_segment` (the Recent/All toggle
  /// with its mode indicator) and `activity_page.dart`'s `_filterChips` /
  /// `_FilterChip`. Both are reproduced verbatim, which is why one method has two
  /// shapes rather than one shape with a flag: they were never the same control.
  @override
  Widget buildSegmented(BuildContext context, ChromeSegmentedSpec spec) {
    final t = CommanderTokens.of(context);
    return switch (spec.style) {
      ChromeSegmentedStyle.control => _segmentedControl(t, spec),
      ChromeSegmentedStyle.chips => _segmentedChips(t, spec),
    };
  }

  /// One bordered container holding the segments, with the mode indicator as a
  /// matching tile beside it. No outer padding — its caller supplies that, as the
  /// fleet list's controls column always did.
  Widget _segmentedControl(CommanderTokens t, ChromeSegmentedSpec spec) {
    final note = spec.note;
    return Row(
      children: [
        Expanded(
          child: Container(
            decoration: BoxDecoration(
              color: t.surface,
              borderRadius: BorderRadius.circular(9),
              border: Border.all(color: t.border),
            ),
            padding: const EdgeInsets.all(3),
            child: Row(
              children: [for (final s in spec.segments) _segment(t, s)],
            ),
          ),
        ),
        if (note != null) ...[
          const SizedBox(width: 9),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 9),
            decoration: BoxDecoration(
              color: t.surface,
              borderRadius: BorderRadius.circular(9),
              border: Border.all(color: t.border),
            ),
            child: Text(
              note,
              style: t.meta(
                size: 10,
                weight: FontWeight.w600,
                color: t.textBright,
              ),
            ),
          ),
        ],
      ],
    );
  }

  Widget _segment(CommanderTokens t, ChromeSegment segment) => Expanded(
    child: GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: segment.onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 6),
        decoration: segment.selected
            ? BoxDecoration(
                color: t.surfaceSelected,
                borderRadius: BorderRadius.circular(6),
              )
            : null,
        child: Text(
          segment.label,
          textAlign: TextAlign.center,
          style: t.meta(
            size: 11,
            weight: FontWeight.w600,
            color: segment.selected ? t.text : t.textMuted,
          ),
        ),
      ),
    ),
  );

  /// Separate rounded pills in a horizontally scrolling strip. The insets are the
  /// scroll view's own, because the row scrolls — which is why this shape needs no
  /// padding from its caller and the segmented control does.
  ///
  /// [ChromeSegmentedSpec.note] is ignored: the filter strip never had one, and
  /// there is nowhere in a scrolling row to put a fixed tile.
  Widget _segmentedChips(CommanderTokens t, ChromeSegmentedSpec spec) {
    return SizedBox(
      height: 46,
      child: ListView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 6),
        children: [for (final s in spec.segments) _chip(t, s)],
      ),
    );
  }

  Widget _chip(CommanderTokens t, ChromeSegment segment) => Padding(
    padding: const EdgeInsets.only(right: 7),
    child: Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: segment.onTap,
        borderRadius: BorderRadius.circular(20),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 13, vertical: 7),
          decoration: BoxDecoration(
            color: segment.selected ? t.surfaceSelected : t.surface,
            borderRadius: BorderRadius.circular(20),
            border: Border.all(
              color: segment.selected ? t.surfaceSelected : t.border,
            ),
          ),
          child: Text(
            segment.label,
            style: t.meta(
              size: 10.5,
              weight: FontWeight.w600,
              color: segment.selected
                  ? t.text
                  // The attention slice keeps its amber caption while unselected,
                  // which is what `_FilterChip`'s `color` parameter carried.
                  : (segment.attention ? t.attentionOn : t.textMuted),
            ),
          ),
        ),
      ),
    ),
  );

  /// `session_list_page.dart`'s search box: a dense `TextField` taking its fill,
  /// radius and border from `inputDecorationTheme`.
  @override
  Widget buildField(BuildContext context, ChromeFieldSpec spec) {
    final t = CommanderTokens.of(context);
    final icon = spec.icon;
    final onClear = spec.onClear;
    return TextField(
      controller: spec.controller,
      onChanged: spec.onChanged,
      textInputAction: spec.textInputAction,
      style: TextStyle(fontSize: 13.5, color: t.text),
      decoration: InputDecoration(
        isDense: true,
        prefixIcon: icon == null ? null : Icon(icon, size: 18),
        prefixIconColor: t.textFaint,
        hintText: spec.hint,
        suffixIcon: onClear == null
            ? null
            : IconButton(
                icon: const Icon(Icons.clear, size: 18),
                tooltip: 'Clear',
                onPressed: onClear,
              ),
      ),
    );
  }

  /// `session_list_page.dart`'s `_FleetHeader` and `activity_page.dart`'s
  /// `_header`, over the controls those pages carried beneath them.
  ///
  /// There is no rail here — Mission Control's view chrome is a header. The
  /// rail is LCARS' answer to the same spec.
  @override
  Widget buildViewRail(BuildContext context, ChromeViewRailSpec spec) {
    final t = CommanderTokens.of(context);
    final controls = _viewControls(context, spec);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _viewHeader(context, spec, t),
        ?controls,
        Expanded(child: spec.body),
      ],
    );
  }

  Widget _viewHeader(
    BuildContext context,
    ChromeViewRailSpec spec,
    CommanderTokens t,
  ) {
    final branded = spec.style == ChromeViewRailStyle.branded;
    final subtitle = spec.subtitle;
    return Padding(
      // The fleet header's insets and the activity header's, unchanged — they
      // never agreed, and neither is worth retuning here.
      padding: branded
          ? const EdgeInsets.fromLTRB(16, 10, 16, 6)
          : const EdgeInsets.fromLTRB(18, 14, 18, 8),
      child: Row(
        children: [
          if (branded) ...[
            const BrandMark(size: 32),
            const SizedBox(width: 10),
          ],
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  spec.title,
                  style: branded
                      ? TextStyle(
                          fontSize: 23,
                          fontWeight: FontWeight.w700,
                          letterSpacing: -0.4,
                          color: t.text,
                        )
                      : Theme.of(
                          context,
                        ).textTheme.titleLarge?.copyWith(fontSize: 24),
                ),
                if (subtitle != null) ...[
                  const SizedBox(height: 2),
                  Text(subtitle, style: t.meta(size: branded ? 10.5 : 11)),
                ],
              ],
            ),
          ),
          // No gap before the first tile: the `Expanded` above absorbs the slack,
          // which is what the fleet header's settings button sat against.
          for (var i = 0; i < spec.actions.length; i++) ...[
            if (i > 0) const SizedBox(width: 8),
            _viewAction(context, spec.actions[i], t),
          ],
        ],
      ),
    );
  }

  /// The fleet header's rounded ⚙ tile. A menu action falls back to the app-bar
  /// rendering, since a `PopupMenuButton` needs to be its own button and no view
  /// asks for one.
  Widget _viewAction(
    BuildContext context,
    ChromeAction action,
    CommanderTokens t,
  ) {
    if (action is! ChromeButtonAction) return _action(context, action, t);
    // Tooltip as well as Semantics: the ⚙ tile this replaces was a
    // `PopupMenuButton(tooltip: 'Settings')`, and the wide shell's equivalent
    // icon button kept one — dropping it here was an inconsistency, not policy.
    return Tooltip(
      message: action.label,
      child: InkWell(
        onTap: action.onPressed,
        child: Semantics(
          button: true,
          label: action.label,
          child: Container(
            width: 34,
            height: 34,
            decoration: BoxDecoration(
              color: t.surface,
              borderRadius: BorderRadius.circular(10),
              border: Border.all(color: t.border),
            ),
            child: Icon(action.icon, size: 16, color: t.textMuted),
          ),
        ),
      ),
    );
  }

  /// The filter field over the slices, in the fleet list's one padded column.
  ///
  /// The chip strip is returned bare: it scrolls, so its insets are its own (see
  /// [_segmentedChips]), and wrapping it would inset the row twice.
  Widget? _viewControls(BuildContext context, ChromeViewRailSpec spec) {
    final filter = spec.filter;
    final slices = spec.slices;
    if (filter == null && slices == null) return null;
    if (filter == null &&
        slices != null &&
        slices.style == ChromeSegmentedStyle.chips) {
      return buildSegmented(context, slices);
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 6, 16, 0),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (filter != null) buildField(context, filter),
          if (filter != null && slices != null) const SizedBox(height: 9),
          if (slices != null) buildSegmented(context, slices),
        ],
      ),
    );
  }

  /// `session_list_page.dart`'s `_ProjectHeader`, minus the count it composed
  /// into its own label. Verbatim casing: uppercasing here would break the call
  /// sites that already pass sentence case ("Files changed").
  @override
  /// The padding is `_ProjectHeader`'s. The review screen's own eyebrows sat at
  /// `LTRB(16, 14, 16, 10)` before the migration, so they tighten slightly here —
  /// one shared element cannot carry two paddings, and the session list is the
  /// denser, more frequently seen of the two.
  Widget buildEyebrow(BuildContext context, String label) {
    final t = CommanderTokens.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(14, 10, 12, 6),
      child: Text(
        label,
        style: t.eyebrow(),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
    );
  }

  @override
  Widget buildWide(BuildContext context, ChromeWideSpec spec) =>
      MissionControlWide(spec);

  @override
  Widget buildWideDetail(BuildContext context, ChromeWideDetailSpec spec) =>
      MissionControlDetail(spec);
}
