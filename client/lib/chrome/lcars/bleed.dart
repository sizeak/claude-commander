/// The LCARS edge-to-edge kit: the ambient bleed a frame publishes, the fill
/// that keeps a bled band continuous, and the system-bar style a bled frame
/// declares. Shared because both LCARS frames — the phone shell/page and the
/// wide shell — bleed identically, and a second copy of any of it would be a
/// second thing to keep in step.
library;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// The bezel the LCARS frame may paint into, resolved once per frame by the
/// chrome that owns the `Scaffold` and read by every block beneath it.
///
/// An inherited value rather than a constructor argument because **there is no
/// parameter path to the view rail**: pages build `ChromeViewRail` themselves
/// (`session_list_page.dart:219`, `activity_page.dart:64`) and it reaches the
/// chrome via `Chrome.of(context)` (`chrome_forms.dart:552-558`), inside a body
/// the shell treats as opaque. The top band could not be reached any other way.
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

/// LCARS is `nav: #CC99CC` and `primary: #F7A01D` — both bright, so the system
/// icons over the band must be dark. `systemNavigationBarColor: transparent`
/// is kept for older devices, but on a three-button Pixel 8a (Android 17) it
/// painted an opaque light scrim across the nav bar regardless — that colour
/// property appears to be a no-op on a modern build, and clashed with the
/// black LCARS canvas besides. `systemNavigationBarContrastEnforced: false`
/// is what actually governs that scrim: per Flutter's own doc on the property
/// (`system_chrome.dart:265-274`, pinned 3.41.5), SDK 29+ may apply a
/// translucent body scrim behind a transparent nav bar to keep it readable,
/// and setting this to `false` overrides it — which is what let the footer's
/// bleed show through. `systemStatusBarContrastEnforced: false` is set for
/// the same reason on the status bar's matching property, even though only
/// the nav bar was seen scrimmed on device: the status bar's band is bled
/// into identically and the same SDK 29+ scrim policy applies to it.
const lcarsSystemBars = SystemUiOverlayStyle(
  statusBarColor: Color(0x00000000),
  statusBarIconBrightness: Brightness.dark,
  statusBarBrightness: Brightness.light,
  systemStatusBarContrastEnforced: false,
  systemNavigationBarColor: Color(0x00000000),
  systemNavigationBarIconBrightness: Brightness.dark,
  systemNavigationBarContrastEnforced: false,
);

/// The gap between two columns, filled across the band when both of them bleed
/// into it and left open below.
///
/// A band behind the status bar has to be *continuous*. Measured on a Pixel 8a,
/// leaving the phone frame's rail/content gap open painted a black column
/// straight through the system clock; the wide frame has the same gap between
/// its nav and fleet columns. [height] is how far down the fill runs — the
/// bottom of whichever bled block it continues into, so the two cannot end at
/// different places.
///
/// With no top inset there is no band and nothing to fill, and this is exactly
/// the plain [SizedBox] the frame has always put there — which is what every
/// desktop and tablet golden depends on.
Widget lcarsBandSeam({
  required double width,
  required double height,
  required Color color,
  required EdgeInsets bleed,
}) {
  if (bleed.top == 0) return SizedBox(width: width);
  return SizedBox(
    width: width,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SizedBox(
          height: height,
          child: ColoredBox(color: color),
        ),
      ],
    ),
  );
}
