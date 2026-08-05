import 'package:flutter/material.dart';

import '../theme/tokens.dart';
import 'chrome_forms.dart';
import 'lcars/lcars_chrome.dart';
import 'mission_control/mission_control_chrome.dart';

/// How a page handles the soft keyboard.
///
/// Three values because the app genuinely has three behaviours today, and
/// collapsing them would regress one of them:
enum ChromeInsets {
  /// No `SafeArea` wrapper; the `Scaffold` shrinks its body for the keyboard.
  /// What every app-bar'd page does today — the app bar covers the top inset and
  /// the `Scaffold` handles the bottom.
  standard,

  /// A `SafeArea` wrapper, still resizing. For full-bleed pages with no app bar
  /// to hold the top inset off the status bar (the connect screen).
  safeArea,

  /// `SafeArea(maintainBottomViewPadding: true)` **and**
  /// `resizeToAvoidBottomInset: false`.
  ///
  /// Only the terminal. The remote PTY must never see a resize when the keyboard
  /// opens — the pane pans instead — because tmux loses a scrolled copy-mode
  /// anchor on a client resize. See PR #259; do not "simplify" this away.
  pan,
}

/// What an action means, which is how LCARS decides a block's colour and where
/// Mission Control gets its emphasis.
enum ChromeActionKind { primary, normal, destructive }

/// Something the user can invoke from a page's chrome.
///
/// Declarative rather than a pre-built `Widget` because the two themes render
/// these completely differently: Mission Control puts them in an `AppBar` (and a
/// primary one in a docked `FloatingActionButton`), while LCARS has no app bar
/// or FAB at all and renders each as a block in its elbow rail.
sealed class ChromeAction {
  final IconData icon;
  final String label;
  final ChromeActionKind kind;

  const ChromeAction({
    required this.icon,
    required this.label,
    this.kind = ChromeActionKind.normal,
  });
}

/// A plain tap action. A null [onPressed] renders disabled.
class ChromeButtonAction extends ChromeAction {
  final VoidCallback? onPressed;

  const ChromeButtonAction({
    required super.icon,
    required super.label,
    required this.onPressed,
    super.kind,
  });
}

/// An action that opens a menu — the settings ⚙ and the session "manage"
/// overflow. Modelled explicitly rather than letting callers smuggle a
/// `PopupMenuButton` through as a widget, because LCARS opens a sheet from a rail
/// block instead of a dropdown from an app bar.
class ChromeMenuAction extends ChromeAction {
  final List<ChromeMenuItem> items;

  const ChromeMenuAction({
    required super.icon,
    required super.label,
    required this.items,
    super.kind,
  });
}

/// One row of a [ChromeMenuAction].
class ChromeMenuItem {
  final String label;
  final VoidCallback? onSelected;
  final bool enabled;

  const ChromeMenuItem({
    required this.label,
    required this.onSelected,
    this.enabled = true,
  });
}

/// Everything a page tells its chrome about its own frame.
@immutable
class ChromePageSpec {
  /// The LCARS rail identifier ("47-A"). Pure LCARS flavour — Mission Control
  /// ignores it entirely.
  final String? code;

  /// The page title. **Null means full-bleed**: no app bar, no rail header. That
  /// is how the connect screen stays honest rather than being given an empty bar.
  final String? title;

  /// A secondary line under the title ("GENIO · FIX/AUTH-BYPASS"). Mission
  /// Control renders it beneath the title; LCARS renders it as the lilac
  /// sub-header from the deck.
  final String? subtitle;

  /// Secondary actions. Mission Control: app-bar actions. LCARS: rail blocks.
  final List<ChromeAction> actions;

  /// The one prominent action. Mission Control: a docked
  /// `FloatingActionButton`. LCARS: a coloured rail block.
  final ChromeAction? primaryAction;

  final ChromeInsets insets;

  /// Whether to offer a back affordance. Defaults to whether the route can pop,
  /// so a pushed page gets one and a shell root does not.
  final bool? showBack;

  final Widget body;

  const ChromePageSpec({
    this.code,
    this.title,
    this.subtitle,
    this.actions = const [],
    this.primaryAction,
    this.insets = ChromeInsets.standard,
    this.showBack,
    required this.body,
  });
}

/// The per-theme renderer for structural chrome.
///
/// Pages describe *what* they contain and let the chrome decide *how* it is
/// framed. This exists because the two themes disagree structurally, not just
/// chromatically: LCARS has no app bar (its rail's top elbow block is the back
/// button), no floating action button, and no bottom navigation bar. A page that
/// built its own `Scaffold` with an `AppBar` could not be rendered as LCARS at
/// all without being rewritten.
///
/// Both implementations **always build a real [Scaffold]** beneath their own
/// decoration, so `ScaffoldMessenger` (≈20 snackbar sites), bottom-sheet
/// anchoring and `FloatingActionButton` positioning keep working regardless of
/// theme.
abstract class Chrome implements ChromeForms {
  const Chrome();

  /// The chrome for the active theme.
  static Chrome of(BuildContext context) =>
      switch (CommanderTokens.of(context).chrome) {
        ChromeKind.missionControl => const MissionControlChrome(),
        ChromeKind.lcars => const LcarsChrome(),
      };

  /// Frames a page.
  Widget buildPage(BuildContext context, ChromePageSpec spec);
}

/// A page frame, rendered by whichever [Chrome] the active theme selects.
///
/// Replaces a hand-built `Scaffold` + `AppBar`. Use it as the root of any routed
/// page:
///
/// ```dart
/// ChromePage(
///   code: '47-A',
///   title: session.title,
///   subtitle: 'GENIO · FIX/AUTH-BYPASS',
///   insets: ChromeInsets.pan,
///   body: TerminalBody(...),
/// )
/// ```
class ChromePage extends StatelessWidget {
  final String? code;
  final String? title;
  final String? subtitle;
  final List<ChromeAction> actions;
  final ChromeAction? primaryAction;
  final ChromeInsets insets;
  final bool? showBack;
  final Widget body;

  const ChromePage({
    super.key,
    this.code,
    this.title,
    this.subtitle,
    this.actions = const [],
    this.primaryAction,
    this.insets = ChromeInsets.standard,
    this.showBack,
    required this.body,
  });

  @override
  Widget build(BuildContext context) => Chrome.of(context).buildPage(
    context,
    ChromePageSpec(
      code: code,
      title: title,
      subtitle: subtitle,
      actions: actions,
      primaryAction: primaryAction,
      insets: insets,
      showBack: showBack,
      body: body,
    ),
  );
}

/// Wraps [body] according to [insets] — shared by both chromes so the terminal's
/// keyboard behaviour cannot diverge between themes.
Widget applyChromeInsets(ChromeInsets insets, Widget body) => switch (insets) {
  ChromeInsets.standard => body,
  ChromeInsets.safeArea => SafeArea(child: body),
  // Keep reserving the bottom system chrome even while the keyboard covers it.
  // Without this, SafeArea's bottom padding collapses to zero when the keyboard
  // appears and the pane's row count would move with it — the very resize that
  // `resizeToAvoidBottomInset: false` exists to avoid.
  ChromeInsets.pan => SafeArea(maintainBottomViewPadding: true, child: body),
};

/// Whether a back affordance should show: the page's explicit choice, else
/// whether this route can actually pop.
bool shouldShowBack(BuildContext context, ChromePageSpec spec) =>
    spec.showBack ?? Navigator.of(context).canPop();

/// Makes a window bar's **empty region** draggable and double-tappable, shared by
/// both chromes so the *window* half of the bar cannot diverge between themes —
/// only its looks are the theme's business.
///
/// Wrap the title and filler only, never the controls. Wrapping the whole bar
/// translucently instead looks equivalent and is not: two quick clicks on
/// adjacent controls fall inside the double-tap slop, so the bar claims the
/// second one and minimise-then-close silently becomes minimise-then-maximise.
Widget applyWindowBarGestures(ChromeWindowBarSpec spec, Widget dragRegion) =>
    GestureDetector(
      behavior: HitTestBehavior.opaque,
      onPanStart: (_) => spec.onDragStart(),
      onDoubleTap: spec.onDoubleTap,
      child: dragRegion,
    );

/// One control in a window bar: an icon for Mission Control, a short block
/// [code] for LCARS, and the [label] both use as its tooltip.
typedef WindowBarControl = ({
  IconData icon,
  String code,
  String label,
  bool destructive,
  VoidCallback onTap,
});

/// The window bar's three controls, as data.
///
/// Shared so the button order — minimise, maximise, close — is one decision
/// rather than two, and so the labels, which are the tooltips *and* the
/// accessibility names, cannot drift between themes.
List<WindowBarControl> windowBarControls(ChromeWindowBarSpec spec) => [
  (
    icon: Icons.remove,
    code: 'MIN',
    label: 'Minimise',
    destructive: false,
    onTap: spec.onMinimize,
  ),
  (
    icon: spec.isMaximized ? Icons.filter_none : Icons.crop_square,
    code: spec.isMaximized ? 'RES' : 'MAX',
    label: spec.isMaximized ? 'Restore' : 'Maximise',
    destructive: false,
    onTap: spec.onToggleMaximize,
  ),
  (
    icon: Icons.close,
    code: 'CLOSE',
    label: 'Close',
    destructive: true,
    onTap: spec.onClose,
  ),
];
