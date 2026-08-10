import 'package:flutter/material.dart';

import '../../theme/tokens.dart';
import '../chrome.dart';
import '../chrome_forms.dart';
import '../chrome_wide.dart';
import 'bleed.dart';
import 'elbow.dart';

/// The LCARS chrome: a black canvas, an elbow rail down the left holding
/// navigation and actions, and a content column capped by a short bar.
///
/// **There is no app bar and no floating action button.** The rail's top elbow
/// *is* the back button, and the primary action is a coloured rail block. That is
/// the whole reason page frames had to become declarative — a page that built its
/// own `AppBar` could not be rendered this way.
///
/// A real [Scaffold] still sits underneath, so `ScaffoldMessenger`, bottom sheets
/// and dialogs work exactly as they do in Mission Control.
class LcarsChrome extends Chrome {
  const LcarsChrome();

  @override
  Widget buildPage(BuildContext context, ChromePageSpec spec) {
    final t = CommanderTokens.of(context);
    return Scaffold(
      backgroundColor: t.canvas,
      resizeToAvoidBottomInset: spec.insets != ChromeInsets.pan,
      body: applyChromeInsets(
        // LCARS draws to the edges, so it always needs the status-bar inset held
        // off — even on a page Mission Control would have let its app bar cover.
        spec.insets == ChromeInsets.standard
            ? ChromeInsets.safeArea
            : spec.insets,
        Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _rail(context, spec, t),
            const SizedBox(width: _railPitch),
            Expanded(child: _content(context, spec, t)),
            const SizedBox(width: 10),
          ],
        ),
      ),
    );
  }

  /// The left rail. Reads top to bottom: identity/back, then actions, then inert
  /// filler that absorbs the slack, then the primary action, then a closing
  /// elbow. The two rounded corners are the first and last blocks only, so the
  /// column reads as one bracket.
  Widget _rail(BuildContext context, ChromePageSpec spec, CommanderTokens t) {
    final back = shouldShowBack(context, spec);
    final blocks = <Widget>[
      ChromeElbow(
        color: back ? t.primary : t.nav,
        corner: ElbowCorner.topLeft,
        height: back ? 62 : 74,
        // With no back affordance the top block carries the screen's LCARS
        // identifier instead ("47-A"), as the deck's root screens do.
        label: back ? '‹ BACK' : spec.code,
        labelAlignment: Alignment.bottomRight,
        labelSize: 12,
        labelWeight: FontWeight.w700,
        onTap: back ? () => Navigator.of(context).maybePop() : null,
      ),
      for (final action in spec.actions)
        ChromeElbow(
          color: _kindColor(action.kind, t),
          labelColor: _kindLabelColor(action.kind, t),
          height: 26,
          label: t.caseLabel(action.label),
          onTap: () => _invoke(context, action),
        ),
      // Two-tone inert filler, brightest first — the deck's rails always step
      // down through a thin bright band into a large dark one.
      ChromeElbow(color: t.borderSubtle, height: 16),
      Expanded(child: ChromeElbow(color: t.divider)),
    ];

    final primary = spec.primaryAction;
    if (primary != null) {
      blocks.add(
        ChromeElbow(
          color: t.info,
          height: 34,
          label: t.caseLabel(primary.label),
          onTap: () => _invoke(context, primary),
        ),
      );
    }
    blocks.add(
      ChromeElbow(
        color: t.nav,
        corner: ElbowCorner.bottomLeft,
        height: 44,
        labelAlignment: Alignment.topRight,
      ),
    );

    return _railColumn(t, blocks);
  }

  /// A rail: [blocks] stacked at the deck's 5px pitch, in a column
  /// [CommanderTokens.railWidth] wide. Shared by the page rail and the view rail
  /// so the two cannot drift.
  Widget _railColumn(CommanderTokens t, List<Widget> blocks) => SizedBox(
    width: t.railWidth,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (var i = 0; i < blocks.length; i++) ...[
          if (i > 0) const SizedBox(height: _railPitch),
          blocks[i],
        ],
      ],
    ),
  );

  /// A block's fill for a given emphasis. Shared by the rail, the button bar and
  /// the footer, so an action reads the same wherever the chrome puts it.
  Color _kindColor(ChromeActionKind kind, CommanderTokens t) => switch (kind) {
    ChromeActionKind.primary => t.primary,
    ChromeActionKind.destructive => t.danger,
    // An unemphasised block is the dark inert fill with lilac text, so the rail
    // does not read as a wall of saturated colour.
    ChromeActionKind.normal => t.borderSubtle,
  };

  /// Text on a [_kindColor] fill: near-black on a saturated block, lilac on the
  /// dark inert one.
  Color _kindLabelColor(ChromeActionKind kind, CommanderTokens t) =>
      kind == ChromeActionKind.normal ? t.nav : t.canvas;

  void _invoke(BuildContext context, ChromeAction action) {
    switch (action) {
      case ChromeButtonAction(:final onPressed):
        onPressed?.call();
      case ChromeMenuAction(:final items):
        // A rail block opens a sheet rather than a dropdown: there is no app bar
        // to hang a menu under, and a sheet is the better touch target anyway.
        showModalBottomSheet<void>(
          context: context,
          builder: (sheetContext) {
            final t = CommanderTokens.of(sheetContext);
            return SafeArea(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final item in items)
                    ListTile(
                      enabled: item.enabled,
                      title: Text(
                        t.caseLabel(item.label),
                        style: TextStyle(
                          fontFamily: t.sans,
                          letterSpacing: 0.6,
                          color: item.enabled ? t.text : t.textFaint,
                        ),
                      ),
                      onTap: () {
                        Navigator.of(sheetContext).pop();
                        item.onSelected?.call();
                      },
                    ),
                ],
              ),
            );
          },
        );
    }
  }

  /// The content column: an elbow cap, then the title block, then the body.
  Widget _content(
    BuildContext context,
    ChromePageSpec spec,
    CommanderTokens t,
  ) {
    final title = spec.title;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ChromeElbowCap(
          color: shouldShowBack(context, spec) ? t.primary : t.nav,
        ),
        if (title != null) ...[
          const SizedBox(height: 7),
          MediaQuery.withClampedTextScaling(
            maxScaleFactor: 1.5,
            child: Text(
              title.toUpperCase(),
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: t.display(size: 22),
            ),
          ),
          if (spec.subtitle != null)
            Text(
              spec.subtitle!.toUpperCase(),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontFamily: t.sans,
                fontSize: 11,
                fontWeight: FontWeight.w500,
                letterSpacing: 1.1,
                color: t.nav,
              ),
            ),
          const SizedBox(height: 9),
        ] else
          const SizedBox(height: 12),
        Expanded(child: spec.body),
      ],
    );
  }

  // ── Form elements ──────────────────────────────────────────────────────────

  /// Deck P2: a solid tone-coloured block carrying the row number, butted against
  /// a panel with a 2px top border of the same colour and a tone-tinted near-black
  /// fill. Only the outer corners of a run are rounded, so a group of rows reads
  /// as one bracketed cluster.
  @override
  Widget buildListRow(BuildContext context, ChromeListRowSpec spec) {
    final t = CommanderTokens.of(context);
    final tone = t.toneStyle(spec.tone);
    final r = Radius.circular(t.pillRadius);
    // A run is bracketed by its ends, so `only` is both of them at once.
    final opensRun =
        spec.position == ChromeRowPosition.first ||
        spec.position == ChromeRowPosition.only;
    final closesRun =
        spec.position == ChromeRowPosition.last ||
        spec.position == ChromeRowPosition.only;
    final rounded = BorderRadius.only(
      topLeft: opensRun ? r : Radius.zero,
      bottomLeft: closesRun ? r : Radius.zero,
    );

    // The row grows with its text, so only the number block needs the scaler
    // clamped — and ChromeElbow already does that.
    Widget row = IntrinsicHeight(
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            width: _rowNumberWidth,
            child: ClipRRect(
              borderRadius: rounded,
              // Top-right, not centre-right: the deck aligns the number with the
              // title's cap height rather than the row's middle.
              child: ChromeElbow(
                color: tone.accent,
                label: spec.number,
                labelAlignment: Alignment.topRight,
              ),
            ),
          ),
          const SizedBox(width: _seam),
          Expanded(child: _rowPanel(t, spec, tone)),
        ],
      ),
    );

    if (spec.dimmed) row = Opacity(opacity: 0.6, child: row);
    final onTap = spec.onTap;
    if (onTap == null) return row;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: onTap,
      child: Semantics(button: true, child: row),
    );
  }

  /// The body half of a [buildListRow]: title, then a metadata line pairing the
  /// accent-coloured subtitle with muted trailing text.
  Widget _rowPanel(CommanderTokens t, ChromeListRowSpec spec, ToneStyle tone) {
    final subtitle = spec.subtitle;
    final trailing = spec.trailingWidget;
    final trailingText = spec.trailing;
    return Container(
      decoration: BoxDecoration(
        color: tone.tintedSurface,
        border: Border(
          top: BorderSide(color: tone.accent, width: t.panelTopBorder),
        ),
      ),
      padding: const EdgeInsets.fromLTRB(9, 5, 9, 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisAlignment: MainAxisAlignment.center,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            t.caseLabel(spec.title),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontFamily: t.sans,
              fontSize: 14,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.7,
              height: 1.15,
              // Selection brightens the title to amber rather than adding a
              // border or a fill — LCARS has no selected-row outline.
              color: spec.selected ? t.primary : t.text,
            ),
          ),
          if (subtitle != null || trailing != null || trailingText != null)
            Row(
              children: [
                Expanded(
                  child: subtitle == null
                      ? const SizedBox.shrink()
                      : Text(
                          t.caseLabel(subtitle),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: _caption(t, tone.accent),
                        ),
                ),
                if (trailing != null)
                  Padding(
                    padding: const EdgeInsets.only(left: 6),
                    child: trailing,
                  )
                else if (trailingText != null)
                  Padding(
                    padding: const EdgeInsets.only(left: 6),
                    child: Text(
                      t.caseLabel(trailingText),
                      style: _caption(t, t.textMuted),
                    ),
                  ),
              ],
            ),
        ],
      ),
    );
  }

  /// Deck P1's `HOST` / `PAIRING TOKEN` boxes: a hard-cornered near-black block
  /// whose entire decoration is a 2px coloured top border. No radius at all —
  /// `t.cardRadius` is 0 here, and rounding one would read as Mission Control.
  @override
  Widget buildPanel(BuildContext context, ChromePanelSpec spec) {
    final t = CommanderTokens.of(context);
    final tone = spec.tone;
    final toneStyle = tone == null ? null : t.toneStyle(tone);
    final eyebrow = spec.eyebrow;
    final child = eyebrow == null
        ? spec.child
        : Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                t.caseLabel(eyebrow),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: _caption(
                  t,
                  t.nav,
                  letterSpacing: 1.1,
                  weight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 4),
              spec.child,
            ],
          );

    final panel = Container(
      decoration: BoxDecoration(
        color: toneStyle?.tintedSurface ?? t.surface,
        border: Border(
          top: BorderSide(
            color: spec.accent ?? toneStyle?.accent ?? t.nav,
            width: t.panelTopBorder,
          ),
        ),
      ),
      padding: spec.padding,
      child: child,
    );

    final onTap = spec.onTap;
    if (onTap == null) return panel;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: onTap,
      child: Semantics(button: true, child: panel),
    );
  }

  /// Deck P1's `ENGAGE / SCAN QR` pair: one contiguous run of blocks separated by
  /// a hairline seam, pill-ended on the outside only, so the bar reads as a single
  /// segmented control rather than a row of buttons.
  @override
  Widget buildButtonBar(BuildContext context, ChromeButtonBarSpec spec) {
    final t = CommanderTokens.of(context);
    final buttons = spec.buttons;
    return Row(
      children: [
        for (var i = 0; i < buttons.length; i++) ...[
          if (i > 0) const SizedBox(width: _seam),
          _barBlock(t, buttons[i], _runEnds(i, buttons.length, t.pillRadius)),
        ],
      ],
    );
  }

  Widget _barBlock(
    CommanderTokens t,
    ChromeBarButton button,
    BorderRadius radius,
  ) {
    // An elbow's single rounded corner is the wrong shape for a bar end, which is
    // a pill, so the radius comes from a clip around the primitive rather than
    // from ElbowCorner. The icon is ignored: LCARS bars are lettered, never
    // glyphed.
    final block = ClipRRect(
      borderRadius: radius,
      child: ChromeElbow(
        color: _kindColor(button.kind, t),
        labelColor: _kindLabelColor(button.kind, t),
        height: _barHeight,
        label: t.caseLabel(button.label),
        labelAlignment: Alignment.center,
        labelSize: 13,
        labelWeight: FontWeight.w700,
        onTap: button.onPressed,
      ),
    );
    return button.expand ? Expanded(child: block) : block;
  }

  /// Deck P2/P3's `SETTINGS / FLEET / + / ACTIVITY`: contiguous blocks meeting
  /// the bottom of the screen, with the outer bottom corners rounded. The centre
  /// action is a block in the run, not a floating button — LCARS has no FAB.
  ///
  /// The settings block leads the run at the rail's width, which is what makes
  /// the frame's bottom-left corner *this* row's rather than a second one above
  /// it: the rail overhead runs down into it, and the bar is the corner it turns.
  @override
  Widget buildFooterNav(BuildContext context, ChromeFooterNavSpec spec) {
    final t = CommanderTokens.of(context);
    final settings = spec.settings;
    final centre = spec.centreAction;
    // Bottom only: the run meets the screen's edge, not its sides.
    final bleed = EdgeInsets.only(bottom: LcarsBleedScope.of(context).bottom);
    // The nav blocks start one slot in when the settings block leads, so every
    // run position below is offset by it.
    final lead = settings == null ? 0 : 1;
    final count = lead + spec.items.length + (centre == null ? 0 : 1);
    // Where the centre action lands among the nav blocks. `count` — i.e. never —
    // when there is no centre action, which also makes the item index below fall
    // through unshifted.
    final centreSlot = centre == null ? count : spec.items.length ~/ 2;

    final blocks = <Widget>[];
    for (var i = 0; i < count - lead; i++) {
      // Bottom corners only: the footer sits against the edge of the screen.
      final ends = _runEnds(i + lead, count, t.pillRadius, bottom: true);
      blocks.add(
        i == centreSlot
            ? _navCentre(t, centre!, ends, bleed)
            : Expanded(
                child: _navBlock(
                  t,
                  spec.items[i < centreSlot ? i : i - 1],
                  ends,
                  bleed,
                ),
              ),
      );
    }

    return Row(
      // Bottom-aligned, not centred: the settings block is the run's only
      // fixed-width one, so it is the only one that can outgrow `_footerHeight`
      // when its label wraps at an accessibility text scale. Centred, that growth
      // would lift every other block off the bottom of the screen — see the 1.3×
      // test in `phone_shell_test.dart`.
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        if (settings != null) ...[
          _navSettings(
            t,
            settings,
            _runEnds(0, count, t.pillRadius, bottom: true),
            bleed,
          ),
          // The rail's own pitch, not the run's tighter seam: this gap continues
          // the gutter between the rail and the content column straight down.
          const SizedBox(width: _railPitch),
        ],
        for (var i = 0; i < blocks.length; i++) ...[
          if (i > 0) const SizedBox(width: _seam),
          blocks[i],
        ],
      ],
    );
  }

  /// One footer destination. [ChromeNavItem.glyph] is deliberately unused — the
  /// deck's LCARS footer is lettered, and a glyph above the label would not fit
  /// the block's height.
  Widget _navBlock(
    CommanderTokens t,
    ChromeNavItem item,
    BorderRadius radius,
    EdgeInsets bleed,
  ) => ClipRRect(
    borderRadius: radius,
    child: ChromeElbow(
      bleed: bleed,
      color: item.selected ? t.primary : t.borderSubtle,
      labelColor: item.selected ? t.canvas : t.nav,
      height: _footerHeight,
      label: t.caseLabel(item.label),
      labelAlignment: Alignment.center,
      labelSize: 13,
      labelWeight: FontWeight.w700,
      onTap: item.onTap,
    ),
  );

  /// The run's leading block: the deck's bottom-left elbow, now lying in the bar.
  ///
  /// [CommanderTokens.railWidth] wide so it sits squarely under the rail above,
  /// and lettered at a rail label's 11px rather than a nav block's 13.
  ///
  /// Measured in Antonio at this exact `TextStyle`, against the 47px a 62px block
  /// leaves after [ChromeElbow]'s padding: 'SETTINGS' takes 45.9px at 13 and
  /// 38.8px at 11. So 13 fits by roughly a pixel and wraps at any text scale
  /// above ~1.0, where 11 holds to ~1.2. Wrapping is not fatal — the block grows
  /// to two lines (`elbow.dart`'s own doc) and [buildFooterNav] bottom-aligns the
  /// run so its neighbours stay on the screen edge — but it costs the footer a
  /// third of its height, so the smaller label is the one that earns its place.
  Widget _navSettings(
    CommanderTokens t,
    ChromeButtonAction settings,
    BorderRadius radius,
    EdgeInsets bleed,
  ) => SizedBox(
    width: t.railWidth,
    child: ClipRRect(
      borderRadius: radius,
      child: ChromeElbow(
        bleed: bleed,
        color: t.nav,
        height: _footerHeight,
        label: t.caseLabel(settings.label),
        labelAlignment: Alignment.center,
        labelWeight: FontWeight.w700,
        onTap: settings.onPressed,
      ),
    ),
  );

  Widget _navCentre(
    CommanderTokens t,
    ChromeButtonAction centre,
    BorderRadius radius,
    EdgeInsets bleed,
  ) => SizedBox(
    width: _footerCentreWidth,
    // The action's own icon, not a `Text('+')`: a text glyph centres its line
    // box rather than its ink, which left the cross painting ~2px low (see
    // ChromeElbow.icon). The block carries no visible label, so the action's
    // real one wraps it for a screen reader.
    child: Semantics(
      label: centre.label,
      child: ClipRRect(
        borderRadius: radius,
        child: ChromeElbow(
          bleed: bleed,
          color: t.attention,
          height: _footerHeight,
          icon: centre.icon,
          iconSize: 20,
          onTap: centre.onPressed,
        ),
      ),
    ),
  );

  /// The phone shell: body above a footer of contiguous blocks.
  ///
  /// No `FloatingActionButton` and no `BottomAppBar` — the deck's LCARS footer is
  /// butted blocks (SETTINGS / FLEET / + / ACTIVITY) whose outer bottom corners
  /// round against the edge of the screen, so the centre action is simply the
  /// middle block rather than something overlapping the bar.
  ///
  /// The run is inset to the body's own margins rather than to margins of its
  /// own: flush left, where the rail is, and 10 off the right, where the content
  /// column ends. That is what lets the footer read as the rail turning its
  /// corner — the two are one bracket, not a frame with a bar under it.
  ///
  /// **No `SafeArea`, deliberately.** One would hold the whole column off the
  /// bezel, which on a gesture-navigation phone leaves a black band under a run
  /// whose entire premise is meeting the edge of the screen. The vertical insets
  /// are published as an [LcarsBleedScope] instead, so each block grows its fill
  /// *and* its padding by them and the labels end up exactly where a `SafeArea`
  /// put them. The horizontal ones stay ordinary padding.
  @override
  Widget buildShell(BuildContext context, ChromeShellSpec spec) {
    final t = CommanderTokens.of(context);
    // Read *here*, above the `Scaffold` this returns, so whether a `Scaffold`
    // republishes its body's padding never has to be assumed.
    final insets = MediaQuery.paddingOf(context);
    return LcarsBleedScope(
      bleed: EdgeInsets.only(top: insets.top, bottom: insets.bottom),
      child: Scaffold(
        backgroundColor: t.canvas,
        body: Padding(
          // Held, not bled: a cutout is an occlusion, not a bezel to decorate.
          // A sub-900dp phone stays on this shell in landscape
          // (`adaptive_shell.dart:25`), where the notch lands on the rail's edge.
          padding: EdgeInsets.only(left: insets.left, right: insets.right),
          child: Column(
            children: [
              Expanded(child: spec.body),
              Padding(
                // Top gap at the rail's pitch, so the rail's filler meets the
                // settings block on the same seam its own blocks are stacked on.
                padding: const EdgeInsets.fromLTRB(0, _railPitch, 10, 0),
                // A `Builder`, because `context` here is the one this method was
                // called with — above the scope it is returning. The footer has
                // to read the bleed from *below* it, exactly as the body's own
                // blocks do.
                child: Builder(
                  builder: (context) => buildFooterNav(
                    context,
                    ChromeFooterNavSpec(
                      items: spec.items,
                      centreAction: spec.centreAction,
                      settings: spec.settings,
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  /// A contiguous run of blocks, pill-ended on the outside only — the same shape
  /// [buildButtonBar] and the footer use, so a slice control reads as the theme's
  /// one segmented idiom. [ChromeSegmentedStyle] is ignored: the deck has no
  /// separate chip shape, and two rounded pill rows would be Mission Control's
  /// distinction, not this theme's.
  @override
  Widget buildSegmented(BuildContext context, ChromeSegmentedSpec spec) {
    final t = CommanderTokens.of(context);
    final segments = spec.segments;
    final note = spec.note;
    // The note closes the run, so it counts as one of its blocks for the purpose
    // of which ends get rounded.
    final count = segments.length + (note == null ? 0 : 1);
    return Row(
      children: [
        for (var i = 0; i < segments.length; i++) ...[
          if (i > 0) const SizedBox(width: _seam),
          Expanded(
            child: ClipRRect(
              borderRadius: _runEnds(i, count, t.pillRadius),
              child: ChromeElbow(
                color: segments[i].selected ? t.primary : t.borderSubtle,
                labelColor: segments[i].selected ? t.canvas : t.nav,
                height: _segmentHeight,
                label: t.caseLabel(segments[i].label),
                labelAlignment: Alignment.center,
                labelSize: 12,
                labelWeight: FontWeight.w700,
                onTap: segments[i].onTap,
              ),
            ),
          ),
        ],
        if (note != null) ...[
          const SizedBox(width: _seam),
          ClipRRect(
            borderRadius: _runEnds(count - 1, count, t.pillRadius),
            child: ChromeElbow(
              // Inert, so it takes the dark filler fill rather than a slice's —
              // it reports the mode, it does not select one.
              color: t.divider,
              labelColor: t.nav,
              height: _segmentHeight,
              label: t.caseLabel(note),
              labelAlignment: Alignment.center,
            ),
          ),
        ],
      ],
    );
  }

  /// Deck P1's input boxes: a near-black block whose whole decoration is a 2px
  /// coloured top border, exactly as [buildPanel] draws one. No radius —
  /// `t.cardRadius` is 0 here, so the theme's `inputDecorationTheme` border is
  /// dropped altogether rather than drawn square all round.
  @override
  Widget buildField(BuildContext context, ChromeFieldSpec spec) {
    final t = CommanderTokens.of(context);
    final icon = spec.icon;
    final hint = spec.hint;
    final onClear = spec.onClear;
    return Container(
      decoration: BoxDecoration(
        color: t.surface,
        border: Border(
          top: BorderSide(color: t.nav, width: t.panelTopBorder),
        ),
      ),
      child: TextField(
        controller: spec.controller,
        onChanged: spec.onChanged,
        textInputAction: spec.textInputAction,
        style: TextStyle(
          fontFamily: t.sans,
          fontSize: 14,
          letterSpacing: 0.6,
          color: t.text,
        ),
        decoration: InputDecoration(
          isDense: true,
          // The container above owns the fill and the border, so the decoration
          // draws neither in any state.
          filled: false,
          border: InputBorder.none,
          enabledBorder: InputBorder.none,
          focusedBorder: InputBorder.none,
          contentPadding: const EdgeInsets.symmetric(
            horizontal: 4,
            vertical: 11,
          ),
          prefixIcon: icon == null ? null : Icon(icon, size: 16),
          prefixIconColor: t.nav,
          hintText: hint == null ? null : t.caseLabel(hint),
          hintStyle: _caption(t, t.textFaint, letterSpacing: 0.9),
          suffixIcon: onClear == null
              ? null
              : IconButton(
                  icon: const Icon(Icons.clear, size: 16),
                  tooltip: 'Clear',
                  color: t.nav,
                  onPressed: onClear,
                ),
        ),
      ),
    );
  }

  /// Deck P2's phone fleet frame: an elbow rail down the left carrying the view's
  /// identifier and its slices, and a content column capped by a short bar.
  ///
  /// The same bracket [buildPage] draws, but scoped to a *view* rather than a
  /// route — so there is no back block (a shell tab has nothing to pop) and the
  /// slices take the position the page rail gives its actions.
  @override
  Widget buildViewRail(BuildContext context, ChromeViewRailSpec spec) {
    final t = CommanderTokens.of(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _viewRail(spec, t),
        const SizedBox(width: _railPitch),
        Expanded(child: _viewContent(context, spec, t)),
        const SizedBox(width: 10),
      ],
    );
  }

  /// The view rail, top to bottom: the identifier, a block per slice, a band
  /// (labelled with the slice note when there is one), then inert filler.
  ///
  /// No closing elbow, unlike [_rail]: this rail is only ever drawn inside the
  /// phone shell, whose footer carries the frame's bottom-left corner — see
  /// [buildFooterNav]. Closing it here would bracket the screen twice.
  Widget _viewRail(ChromeViewRailSpec spec, CommanderTokens t) {
    final slices = spec.slices;
    final note = slices?.note;
    final blocks = <Widget>[
      ChromeElbow(
        color: t.nav,
        corner: ElbowCorner.topLeft,
        height: 74,
        label: spec.code,
        labelAlignment: Alignment.bottomRight,
        labelSize: 12,
        labelWeight: FontWeight.w700,
      ),
      for (final slice in slices?.segments ?? const <ChromeSegment>[])
        ChromeElbow(
          color: slice.selected ? t.primary : t.borderSubtle,
          labelColor: slice.selected ? t.canvas : t.nav,
          height: _railSliceHeight,
          label: t.caseLabel(slice.label),
          onTap: slice.onTap,
        ),
      // The thin bright band the deck steps down through before the dark filler.
      // It carries the mode note when there is one — an inert label on an inert
      // block, which is where LCARS puts a readout.
      if (note == null)
        ChromeElbow(color: t.borderSubtle, height: 16)
      else
        ChromeElbow(
          color: t.borderSubtle,
          labelColor: t.nav,
          height: 26,
          label: t.caseLabel(note),
        ),
      Expanded(child: ChromeElbow(color: t.divider)),
    ];
    return _railColumn(t, blocks);
  }

  /// The content column: the elbow cap closing the rail's bracket, the title and
  /// its subtitle, the filter field, then the body.
  Widget _viewContent(
    BuildContext context,
    ChromeViewRailSpec spec,
    CommanderTokens t,
  ) {
    final subtitle = spec.subtitle;
    final filter = spec.filter;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ChromeElbowCap(color: t.nav),
        const SizedBox(height: 7),
        MediaQuery.withClampedTextScaling(
          maxScaleFactor: 1.5,
          child: Text(
            spec.title.toUpperCase(),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: t.display(size: 22),
          ),
        ),
        if (subtitle != null)
          Text(
            subtitle.toUpperCase(),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: _caption(t, t.nav, letterSpacing: 1.1),
          ),
        const SizedBox(height: 9),
        if (filter != null) ...[
          buildField(context, filter),
          const SizedBox(height: 9),
        ],
        Expanded(child: spec.body),
      ],
    );
  }

  @override
  Widget buildWide(BuildContext context, ChromeWideSpec spec) =>
      LcarsWide(spec);

  @override
  Widget buildWideDetail(BuildContext context, ChromeWideDetailSpec spec) =>
      LcarsDetail(spec);

  /// The window bar as a run of blocks: a lilac cap carrying the app name, dark
  /// filler that absorbs the slack (and is the drag surface), then three control
  /// blocks closing the run.
  ///
  /// Not a flat bar of icon buttons — LCARS has no such element. The controls are
  /// short-coded blocks (`MIN`, `MAX`, `CLOSE`) like every other LCARS control,
  /// wrapped in [Tooltip]s so an abbreviation still announces its full name.
  @override
  Widget buildWindowBar(BuildContext context, ChromeWindowBarSpec spec) {
    final t = CommanderTokens.of(context);
    final controls = windowBarControls(spec);
    // The name cap, the filler and each control are one run, so only the two
    // outer ends round — the bar reads as a single bracket across the window.
    final count = controls.length + 2;
    return Padding(
      padding: const EdgeInsets.only(bottom: _seam),
      child: Row(
        children: [
          // The name cap and the filler are the drag surface; the controls to
          // their right are deliberately outside it.
          Expanded(
            child: applyWindowBarGestures(
              spec,
              Row(
                children: [
                  ClipRRect(
                    borderRadius: _runEnds(0, count, t.pillRadius),
                    child: ChromeElbow(
                      color: t.nav,
                      labelColor: t.canvas,
                      height: _windowBarHeight,
                      label: t.caseLabel(spec.title),
                      labelAlignment: Alignment.center,
                      labelSize: 12,
                      labelWeight: FontWeight.w700,
                    ),
                  ),
                  const SizedBox(width: _seam),
                  // Inert filler: the block that makes the run span the window,
                  // and the easiest part of the bar to grab for a drag.
                  Expanded(
                    child: ChromeElbow(
                      color: t.divider,
                      height: _windowBarHeight,
                    ),
                  ),
                ],
              ),
            ),
          ),
          for (var i = 0; i < controls.length; i++) ...[
            const SizedBox(width: _seam),
            Tooltip(
              message: controls[i].label,
              child: ClipRRect(
                borderRadius: _runEnds(i + 2, count, t.pillRadius),
                child: ChromeElbow(
                  color: controls[i].destructive ? t.danger : t.borderSubtle,
                  labelColor: controls[i].destructive ? t.canvas : t.nav,
                  height: _windowBarHeight,
                  label: controls[i].code,
                  labelAlignment: Alignment.center,
                  labelSize: 12,
                  labelWeight: FontWeight.w700,
                  onTap: controls[i].onTap,
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }

  @override
  Widget buildEyebrow(BuildContext context, String label) {
    final t = CommanderTokens.of(context);
    return Padding(
      // Vertical only — the content column already owns the horizontal inset.
      // Without this the label sat flush against the row beneath it, so a group
      // header read as part of its first row rather than as a heading over the
      // run. Mission Control's eyebrow carried this padding from the start;
      // LCARS' was a bare Text.
      //
      // 13 rather than 12, and the odd number is load-bearing. An 11px Antonio
      // line box is 14.23 logical px, so headings accrue fractional offsets down
      // the list; at dpr 1.5 the *first* one landed half a physical pixel out of
      // phase with the rest, putting its cap tops flush on a pixel boundary. It
      // rendered a hard top edge — read as clipped, though nothing clips it —
      // while every later heading got a soft antialiased row above its caps. One
      // extra logical pixel moves this heading 1.5 physical px (flipping its
      // phase) and every heading below it 3.0 (leaving theirs alone), so the
      // first agrees with the rest. Verified by dumping both headings' pixels:
      // identical rasterisation, row for row.
      padding: const EdgeInsets.only(top: 13, bottom: 6),
      child: Text(
        t.caseLabel(label),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: _caption(t, t.info, letterSpacing: 1.4),
      ),
    );
  }
}

// ── Local geometry ───────────────────────────────────────────────────────────
// Private to LCARS: these are transcribed from the deck's frames, and nothing
// outside this chrome should depend on them.

/// The gap between blocks in a contiguous horizontal run. Tighter than the rail's
/// vertical 5px, matching the deck's `gap:4px` on bars and row pairs — the seam is
/// meant to read as a join, not a separation.
const _seam = 4.0;

/// The vertical pitch of a rail: the gap between stacked blocks, and the gap
/// between the rail and the content column beside it. The deck's rails are 5px
/// apart throughout.
const _railPitch = 5.0;

/// The list row's leading number block (deck P2's node headers: `width:38px`).
const _rowNumberWidth = 38.0;

/// The app-drawn window bar's block height. Matches Mission Control's bar so
/// switching theme while borderless does not reflow the page beneath it.
const _windowBarHeight = 32.0;

/// A slice block in a view rail. Taller than a rail action (26) because a slice is
/// the rail's primary control, matching the wide nav's destination blocks.
const _railSliceHeight = 30.0;

/// A block in a horizontal segmented run. Shorter than a button bar's 36 — a
/// slice control sits inside a view's controls, not under its content.
const _segmentHeight = 30.0;

/// Bar block height. The deck's `padding:11px 0` on 13px type comes to ~37px;
/// rounded down, and comfortably clear of ChromeElbow's clamped 1.3× scaler.
const _barHeight = 36.0;

/// Footer block height. The deck's is ~33px, but a 13px destination label at
/// ChromeElbow's clamped 1.3× text scaler leaves that barely any room, so the
/// run is 5px taller than the frame — which also drags the nav closer to a
/// usable touch target.
const _footerHeight = 38.0;

/// The footer's centre action block (deck P2/P3: `width:46px`).
const _footerCentreWidth = 46.0;

/// The recurring 11px condensed caption: row subtitles and trailing metadata,
/// panel eyebrows, section eyebrows.
///
/// Antonio rather than [CommanderTokens.mono] — this is chrome, and only real
/// agent output stays monospace.
TextStyle _caption(
  CommanderTokens t,
  Color color, {
  double letterSpacing = 0,
  FontWeight weight = FontWeight.w500,
}) => TextStyle(
  fontFamily: t.sans,
  fontSize: 11,
  fontWeight: weight,
  letterSpacing: letterSpacing,
  color: color,
);

/// Rounds the outer ends of a horizontal run of [count] blocks: block [i] gets a
/// [radius] corner on its left when it is first and on its right when it is last,
/// so a run of any length reads as one bracketed unit.
///
/// [bottom] rounds only the bottom pair, which is how the deck's footer meets the
/// edge of the screen.
BorderRadius _runEnds(int i, int count, double radius, {bool bottom = false}) {
  final r = Radius.circular(radius);
  final start = i == 0 ? r : Radius.zero;
  final end = i == count - 1 ? r : Radius.zero;
  return BorderRadius.only(
    topLeft: bottom ? Radius.zero : start,
    bottomLeft: start,
    topRight: bottom ? Radius.zero : end,
    bottomRight: end,
  );
}
