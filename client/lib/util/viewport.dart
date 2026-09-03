import 'package:flutter/widgets.dart';

/// Viewport height at or below which the chrome switches to its compact form.
///
/// Height, not [Orientation]: what hurts on a phone held sideways is that
/// ~150dp of fixed chrome eats 40% of a 360dp-tall viewport, and a short
/// desktop window has exactly the same problem. 500 sits above every phone
/// landscape height in use (360–430 logical) and well below a tablet's short
/// side (768), so it separates the case that needs the space from the ones
/// that don't.
const double kShortViewportHeight = 500;

/// Whether the window is too short to spend full-height chrome on.
///
/// Reads `MediaQuery.sizeOf`, which the soft keyboard does **not** change (a
/// keyboard moves `viewInsets`, not the size). That matters beyond tidiness:
/// the terminal derives the remote pane's row count from what is left after
/// its chrome, so a compactness test that flipped when the keyboard opened
/// would resize the PTY — the very thing `ChromeInsets.pan` exists to prevent.
bool isShortViewport(BuildContext context) =>
    MediaQuery.sizeOf(context).height <= kShortViewportHeight;
