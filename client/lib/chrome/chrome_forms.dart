import 'package:flutter/material.dart';

import '../theme/tokens.dart';
import 'chrome.dart';
import 'chrome_wide.dart';

/// A row in a list of sessions, activity events, servers or files.
///
/// The two themes render this very differently, which is why it is a chrome
/// element rather than a shared widget with a few colour parameters:
///
/// * Mission Control: a rounded, bordered card (or a divider-ruled row) with a
///   leading state glyph.
/// * LCARS: a leading solid colour block carrying a two-digit number, butted
///   against a panel with a 2px coloured top border and a tone-tinted near-black
///   fill. No radius except on the first and last rows of a run.
@immutable
class ChromeListRowSpec {
  final String title;

  /// The metadata line under the title. LCARS uppercases it.
  final String? subtitle;

  /// Right-aligned trailing text (a relative age, a state word).
  final String? trailing;

  /// A trailing widget, used where the trailing slot is a badge rather than
  /// text. Takes precedence over [trailing].
  final Widget? trailingWidget;

  /// Drives every colour in the row.
  final SessionTone tone;

  /// The leading glyph Mission Control shows. LCARS ignores it in favour of
  /// [number].
  final Widget? glyph;

  /// The two-digit identifier LCARS prints in the leading block. Mission Control
  /// ignores it. Derive it with [lcarsRowNumber].
  final String? number;

  final bool selected;

  /// Dimmed, for an inert or unreachable row.
  final bool dimmed;

  /// Render [title] in the monospace face. For rows whose title is a file path:
  /// everywhere else in the review UI a path is monospace, and a sans one reads
  /// as prose rather than as a path.
  final bool monoTitle;

  final VoidCallback? onTap;

  /// Where this row sits in a visual run, so LCARS can round only the outer
  /// corners and Mission Control can ignore it.
  final ChromeRowPosition position;

  const ChromeListRowSpec({
    required this.title,
    this.subtitle,
    this.trailing,
    this.trailingWidget,
    this.tone = SessionTone.idle,
    this.glyph,
    this.number,
    this.selected = false,
    this.dimmed = false,
    this.monoTitle = false,
    this.onTap,
    this.position = ChromeRowPosition.middle,
  });
}

/// Position within a run of rows. LCARS rounds the outer corners of a run so it
/// reads as one bracketed group.
enum ChromeRowPosition { first, middle, last, only }

/// A stable two-digit LCARS row number derived from a session id.
///
/// The deck shows non-sequential numbers (`02 03 07 11 14`), so they are clearly
/// derived from the session rather than its index — an index would renumber every
/// row whenever the list re-sorted, which for a list that reorders on activity
/// would mean the numbers flickered constantly.
///
/// Never `00`: in the deck that value means "this node has no sessions", so it
/// must not collide with a real row.
String lcarsRowNumber(String id) {
  // FNV-1a over the id's code units. Any stable hash would do; this one is short
  // enough to read and has no dependencies.
  var hash = 0x811c9dc5;
  for (final unit in id.codeUnits) {
    hash = (hash ^ unit) * 0x01000193 & 0xFFFFFFFF;
  }
  return (hash % 99 + 1).toString().padLeft(2, '0');
}

/// A block of content: a card in Mission Control, a top-bordered panel in LCARS.
@immutable
class ChromePanelSpec {
  /// An uppercase label above the content ("SUMMARY", "FILES CHANGED").
  final String? eyebrow;

  /// Tints the panel. Null uses the neutral surface.
  final SessionTone? tone;

  /// Overrides [tone]'s accent for the border/eyebrow, for panels whose meaning
  /// is not a session state (a success-green diff summary, say).
  final Color? accent;

  final EdgeInsetsGeometry padding;
  final VoidCallback? onTap;
  final Widget child;

  const ChromePanelSpec({
    this.eyebrow,
    this.tone,
    this.accent,
    this.padding = const EdgeInsets.all(10),
    this.onTap,
    required this.child,
  });
}

/// One entry in a [ChromeButtonBar].
@immutable
class ChromeBarButton {
  final String label;

  /// Shown instead of [label] by Mission Control where an icon reads better.
  final IconData? icon;

  /// The hover/long-press description, defaulting to [label].
  ///
  /// Distinct from [label] because the two serve different masters: [label] is
  /// the visible caption and has to stay short enough for an LCARS block, while
  /// the tooltip can say the whole thing. The session lifecycle bar relies on
  /// exactly that — 'Push' / 'Push stack', 'Cascade' / 'Cascade merge'.
  final String? tooltip;

  final ChromeActionKind kind;
  final VoidCallback? onPressed;

  /// Overrides the colour [kind] would give this button.
  ///
  /// Exists because `kind` has three values and Mission Control's lifecycle bar
  /// has five hues: Kill is amber and Restart is teal, distinct from the neutral
  /// Shell/Cascade/Push. Collapsing them onto `normal` silently flattened both —
  /// a regression the token-parity test cannot see, because it pins what the
  /// colours *are*, not where they are used.
  ///
  /// LCARS ignores this: its blocks are deliberately a three-role set, so extra
  /// hues would dilute the grammar rather than enrich it.
  final Color? accent;

  /// Takes the remaining width rather than sizing to its label.
  final bool expand;

  const ChromeBarButton({
    required this.label,
    this.icon,
    this.tooltip,
    this.accent,
    this.kind = ChromeActionKind.normal,
    this.onPressed,
    this.expand = false,
  });
}

/// A row of buttons: separate outlined buttons in Mission Control, one
/// contiguous run of blocks with rounded outer ends in LCARS.
///
/// Covers the session lifecycle bar, the terminal's modifier keys, and the
/// review screen's approve/request-changes bar.
@immutable
class ChromeButtonBarSpec {
  final List<ChromeBarButton> buttons;

  const ChromeButtonBarSpec({required this.buttons});
}

/// One choice in a [ChromeSegmented] run, or one slice in a [ChromeViewRail].
///
/// Distinct from [ChromeNavItem] — which it otherwise resembles — because a
/// segment has no glyph (neither theme draws one on a segmented control or a rail
/// block) and does carry [attention], which a footer destination has no use for.
@immutable
class ChromeSegment {
  final String label;
  final bool selected;
  final VoidCallback onTap;

  /// Marks the one choice that is asking for a human ("Needs you"). Mission
  /// Control tints its label; LCARS ignores it, a selected block being amber
  /// already.
  final bool attention;

  const ChromeSegment({
    required this.label,
    required this.selected,
    required this.onTap,
    this.attention = false,
  });
}

/// Which shape Mission Control gives a run of segments.
///
/// Two values because the app has two, and they are not interchangeable: the
/// fleet list's Recent/All toggle is one bordered container with an inner
/// selected pill, while the activity feed's filters are separate rounded pills in
/// a horizontally scrolling strip that carries its own insets. LCARS collapses
/// both to the same contiguous run of blocks.
enum ChromeSegmentedStyle { control, chips }

/// A single-select run of choices: a segmented control or a filter-chip strip in
/// Mission Control, a contiguous run of blocks with rounded outer ends in LCARS.
@immutable
class ChromeSegmentedSpec {
  final List<ChromeSegment> segments;

  final ChromeSegmentedStyle style;

  /// A short read-only note beside the run — the fleet list's `grouped` /
  /// `↓ recency` mode indicator. Mission Control renders it as a tile at the
  /// trailing end; LCARS as an inert block closing the run.
  final String? note;

  const ChromeSegmentedSpec({
    required this.segments,
    this.style = ChromeSegmentedStyle.control,
    this.note,
  });
}

/// A single-line text input: a bordered, rounded `TextField` in Mission Control,
/// a hard-cornered panel with a 2px coloured top border in LCARS (`t.cardRadius`
/// is 0 there, and rounding one would read as Mission Control).
@immutable
class ChromeFieldSpec {
  final TextEditingController? controller;

  /// Placeholder text. LCARS uppercases it.
  final String? hint;

  /// A leading glyph. LCARS tints it with the nav accent.
  final IconData? icon;

  final ValueChanged<String>? onChanged;

  /// A clear affordance is rendered when this is non-null, so "is there anything
  /// to clear?" stays the caller's decision (it owns the controller) while the
  /// button's icon, tooltip and placement stay the chrome's.
  final VoidCallback? onClear;

  final TextInputAction? textInputAction;

  const ChromeFieldSpec({
    this.controller,
    this.hint,
    this.icon,
    this.onChanged,
    this.onClear,
    this.textInputAction,
  });
}

/// Which header treatment Mission Control gives a [ChromeViewRail].
///
/// The app's two views differ in more than a brand mark — the padding, the title
/// face and the subtitle size all differ — so this selects the whole treatment
/// rather than being a `brand` flag that quietly also moves the metrics. LCARS
/// ignores it: both views get the same elbow rail.
enum ChromeViewRailStyle {
  /// The fleet list: a `BrandMark`-led header over a segmented slice control.
  branded,

  /// The activity feed: a plain title header over a scrolling filter strip.
  plain,
}

/// A whole *view's* frame: what slice of its data is showing, how it is filtered,
/// what it is called, and what can be done to it.
///
/// Distinct from [ChromeShellSpec], and the split matters: the shell's footer
/// carries **app**-level navigation (Fleet / + / Activity) while this carries
/// **view**-scoped controls. The deck's phone frames have both at once, which is
/// why the rail belongs to the view rather than to the shell — a view embedded in
/// the wide shell (whose own chrome already titles its panes) simply doesn't ask
/// for one.
///
/// Mission Control renders it as the branded header the two pages built by hand,
/// with the filter and slices in a padded controls column beneath. LCARS renders
/// the deck's left elbow rail: [code], a block per slice, an inert band, filler,
/// and a closing elbow for the last action — with the title, subtitle and filter
/// in the content column beside it.
@immutable
class ChromeViewRailSpec {
  /// The LCARS rail identifier ("47-A"). Mission Control ignores it.
  final String? code;

  final String title;

  /// The aggregate line under the title ("0 active · 0 total · 1 server").
  final String? subtitle;

  final ChromeViewRailStyle style;

  /// The view's filter field. Mission Control puts it under the header, above the
  /// slices; LCARS puts it at the top of the content column. Placement is the
  /// whole reason it is a slot here rather than the first widget of [body].
  final ChromeFieldSpec? filter;

  /// Which slice of the view's data is showing (Recent/All, or the activity
  /// filters). LCARS turns these into rail blocks, which is why they are data
  /// rather than a pre-built [ChromeSegmented].
  final ChromeSegmentedSpec? slices;

  /// Actions on the view itself (settings). LCARS gives the last one the rail's
  /// closing elbow — the deck's bottom-left block — and any earlier ones a block
  /// of their own; Mission Control renders each as a tile beside the title.
  final List<ChromeAction> actions;

  final Widget body;

  const ChromeViewRailSpec({
    this.code,
    required this.title,
    this.subtitle,
    this.style = ChromeViewRailStyle.branded,
    this.filter,
    this.slices,
    this.actions = const [],
    required this.body,
  });
}

/// One destination in the phone shell's footer navigation.
@immutable
class ChromeNavItem {
  final String label;

  /// The glyph Mission Control shows above the label ('▤', '≋').
  final String glyph;

  final bool selected;
  final VoidCallback onTap;

  const ChromeNavItem({
    required this.label,
    required this.glyph,
    required this.selected,
    required this.onTap,
  });
}

/// The phone shell's bottom navigation.
///
/// Mission Control: a `BottomAppBar` with a docked centre `FloatingActionButton`.
/// LCARS: three contiguous blocks (FLEET / + / ACTIVITY) with rounded outer
/// bottom corners and no FAB at all.
@immutable
class ChromeFooterNavSpec {
  final List<ChromeNavItem> items;

  /// The centre action. Mission Control docks it as a FAB; LCARS makes it the
  /// middle block.
  final ChromeButtonAction? centreAction;

  const ChromeFooterNavSpec({required this.items, this.centreAction});
}

/// The phone shell's whole frame: a body over a footer navigation bar.
///
/// The chrome owns the `Scaffold` here for the same reason it does for a page —
/// and for one concrete reason besides. Mission Control docks its centre action
/// as a `FloatingActionButton` overlapping the bar's top edge, which only
/// hit-tests correctly when the `Scaffold` positions it via
/// `floatingActionButtonLocation`. Returning the bar alone and letting the shell
/// stack a FAB on top produces a button that paints but cannot be tapped, since
/// Flutter does not hit-test outside a render box's bounds.
@immutable
class ChromeShellSpec {
  final Widget body;
  final List<ChromeNavItem> items;

  /// The prominent centre action (new session). Mission Control docks it as a
  /// FAB between the tabs; LCARS makes it the middle footer block.
  final ChromeButtonAction? centreAction;

  const ChromeShellSpec({
    required this.body,
    required this.items,
    this.centreAction,
  });
}

/// The desktop window's own title bar, drawn by the app.
///
/// Shown when the native title bar is hidden (`TitleBarMode.borderless`), which
/// is the desktop default: GTK3 has no `xdg-decoration` support, so a GTK3 window
/// on Wayland is always client-side decorated and KWin will never draw its own
/// title bar for this app — the "native" frame is a GNOME-style header bar even
/// in a KDE session, and this is the only frame that can match the desktop.
///
/// Declarative for the same reason [ChromeAction] is: the two themes disagree
/// structurally, LCARS having no notion of a flat bar of icon buttons. Carrying
/// the *callbacks* rather than a built widget also keeps `chrome/` free of any
/// `window_manager` import, so both bars are testable with plain closures.
@immutable
class ChromeWindowBarSpec {
  /// The app's name. Deliberately not the active page's title, which the page's
  /// own chrome already shows and which would make the bar flicker on every
  /// navigation.
  final String title;

  /// Drives the maximise control's glyph and label.
  final bool isMaximized;

  /// The user began dragging the bar's empty space — hand it to the compositor as
  /// a window move.
  final VoidCallback onDragStart;

  /// A double-tap on the bar, which every desktop treats as maximise/restore.
  final VoidCallback onDoubleTap;

  final VoidCallback onMinimize;
  final VoidCallback onToggleMaximize;

  /// Quits the app, as the native close button does.
  final VoidCallback onClose;

  const ChromeWindowBarSpec({
    required this.title,
    this.isMaximized = false,
    required this.onDragStart,
    required this.onDoubleTap,
    required this.onMinimize,
    required this.onToggleMaximize,
    required this.onClose,
  });
}

/// The form-element half of the chrome contract.
///
/// Split from [Chrome] into its own interface purely to keep the files readable.
/// [Chrome] implements it, so `Chrome.of(context)` exposes everything.
abstract interface class ChromeForms {
  Widget buildListRow(BuildContext context, ChromeListRowSpec spec);
  Widget buildPanel(BuildContext context, ChromePanelSpec spec);
  Widget buildButtonBar(BuildContext context, ChromeButtonBarSpec spec);
  Widget buildFooterNav(BuildContext context, ChromeFooterNavSpec spec);
  Widget buildShell(BuildContext context, ChromeShellSpec spec);
  Widget buildSegmented(BuildContext context, ChromeSegmentedSpec spec);
  Widget buildField(BuildContext context, ChromeFieldSpec spec);
  Widget buildViewRail(BuildContext context, ChromeViewRailSpec spec);

  /// The wide (desktop/tablet) shell, and the workspace pane inside it. Their
  /// specs and both implementations live in `chrome_wide.dart` — see that file
  /// for why the two variants are co-located.
  Widget buildWide(BuildContext context, ChromeWideSpec spec);
  Widget buildWideDetail(BuildContext context, ChromeWideDetailSpec spec);

  /// A section eyebrow ("SERVERS", "FILES CHANGED").
  Widget buildEyebrow(BuildContext context, String label);

  /// The app-drawn window title bar, for borderless desktop windows.
  Widget buildWindowBar(BuildContext context, ChromeWindowBarSpec spec);
}

// ── Widget front-ends ────────────────────────────────────────────────────────
// Pages use these rather than calling Chrome.of(context) directly, so a call site
// reads as a widget tree.

class ChromeListRow extends StatelessWidget {
  final ChromeListRowSpec spec;
  const ChromeListRow(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildListRow(context, spec);
}

class ChromePanel extends StatelessWidget {
  final ChromePanelSpec spec;
  const ChromePanel(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildPanel(context, spec);
}

class ChromeButtonBar extends StatelessWidget {
  final ChromeButtonBarSpec spec;
  const ChromeButtonBar(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildButtonBar(context, spec);
}

class ChromeFooterNav extends StatelessWidget {
  final ChromeFooterNavSpec spec;
  const ChromeFooterNav(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildFooterNav(context, spec);
}

/// The phone shell's frame, `Scaffold` included. See [ChromeShellSpec].
class ChromeShell extends StatelessWidget {
  final ChromeShellSpec spec;
  const ChromeShell(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildShell(context, spec);
}

/// A single-select run of choices. See [ChromeSegmentedSpec].
class ChromeSegmented extends StatelessWidget {
  final ChromeSegmentedSpec spec;
  const ChromeSegmented(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildSegmented(context, spec);
}

/// A single-line text input. See [ChromeFieldSpec].
class ChromeField extends StatelessWidget {
  final ChromeFieldSpec spec;
  const ChromeField(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildField(context, spec);
}

/// A view's frame — its title, filter, slices and actions around its body. See
/// [ChromeViewRailSpec].
class ChromeViewRail extends StatelessWidget {
  final ChromeViewRailSpec spec;
  const ChromeViewRail(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildViewRail(context, spec);
}

class ChromeEyebrow extends StatelessWidget {
  final String label;
  const ChromeEyebrow(this.label, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildEyebrow(context, label);
}

/// The app-drawn window title bar. See [ChromeWindowBarSpec].
class ChromeWindowBar extends StatelessWidget {
  final ChromeWindowBarSpec spec;
  const ChromeWindowBar(this.spec, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildWindowBar(context, spec);
}
