import 'package:flutter/material.dart';

import '../theme/tokens.dart';
import 'chrome.dart';

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

  final ChromeActionKind kind;
  final VoidCallback? onPressed;

  /// Takes the remaining width rather than sizing to its label.
  final bool expand;

  const ChromeBarButton({
    required this.label,
    this.icon,
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

  /// A section eyebrow ("SERVERS", "FILES CHANGED").
  Widget buildEyebrow(BuildContext context, String label);
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

class ChromeEyebrow extends StatelessWidget {
  final String label;
  const ChromeEyebrow(this.label, {super.key});

  @override
  Widget build(BuildContext context) =>
      Chrome.of(context).buildEyebrow(context, label);
}
