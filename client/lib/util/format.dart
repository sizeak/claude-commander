// Small, widget-free formatting helpers shared across the redesigned screens
// (relative ages in list rows / the Activity feed). Kept pure so they're
// trivially unit-testable, mirroring `session_filter.dart`.

import 'package:flutter/foundation.dart' show visibleForTesting;

/// Test-only override for the wall clock [relativeAge] falls back to.
///
/// Passing `now` is the seam to prefer. This exists for rows that compute their
/// age inside `build` with no way to inject one — the fleet list's Recent rows —
/// so a golden of them would otherwise drift with the date. Production never
/// assigns this.
///
/// Set it in `setUp` and clear it in `tearDown`, as with `debugSessionScorer`.
@visibleForTesting
DateTime Function()? debugClock;

/// A compact relative age like the deck's `2m` / `14m` / `1h` / `3d`.
///
/// [now] defaults to the wall clock (or [debugClock]) but is injectable for
/// tests. Future or just-now timestamps render as `now`.
String relativeAge(DateTime then, {DateTime? now}) {
  final ref = now ?? debugClock?.call() ?? DateTime.now();
  var secs = ref.difference(then).inSeconds;
  if (secs < 5) return 'now';
  if (secs < 60) return '${secs}s';
  final mins = secs ~/ 60;
  if (mins < 60) return '${mins}m';
  final hours = mins ~/ 60;
  if (hours < 24) return '${hours}h';
  final days = hours ~/ 24;
  if (days < 7) return '${days}d';
  final weeks = days ~/ 7;
  if (weeks < 5) return '${weeks}w';
  final months = days ~/ 30;
  if (months < 12) return '${months}mo';
  return '${days ~/ 365}y';
}
