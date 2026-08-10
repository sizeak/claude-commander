import 'package:flutter/widgets.dart';

/// The bezel the LCARS frame may paint into, resolved once per frame by the
/// chrome that owns the `Scaffold` and read by every block beneath it.
///
/// An inherited value rather than a constructor argument because **there is no
/// parameter path to the view rail**: pages build `ChromeViewRail` themselves
/// (`session_list_page.dart:219`, `activity_page.dart:64`) and it reaches the
/// chrome via `Chrome.of(context)` (`chrome_forms.dart:552-558`), inside a body
/// the shell treats as opaque. The top band could not be reached any other way.
///
/// It carries the *resolved* bleed rather than raw insets, which is what lets
/// `ChromeInsets.pan` publish [EdgeInsets.zero]: the terminal already wraps its
/// whole row in a `SafeArea` (`chrome.dart:224`), so a block that also bled
/// would be offset twice.
class LcarsBleedScope extends InheritedWidget {
  final EdgeInsets bleed;

  const LcarsBleedScope({super.key, required this.bleed, required super.child});

  /// The ambient bleed, or zero with no scope above — which is what every widget
  /// test that does not opt in receives, and why the existing goldens do not
  /// move.
  static EdgeInsets of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<LcarsBleedScope>()?.bleed ??
      EdgeInsets.zero;

  @override
  bool updateShouldNotify(LcarsBleedScope oldWidget) =>
      bleed != oldWidget.bleed;
}
