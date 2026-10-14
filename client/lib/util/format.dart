// Small, Flutter-free formatting helpers shared across the redesigned screens
// (relative ages in list rows / the Activity feed). Kept pure so they're
// trivially unit-testable, mirroring `session_filter.dart`.

/// A compact relative age like the deck's `2m` / `14m` / `1h` / `3d`.
///
/// [now] defaults to the wall clock but is injectable for tests. Future or
/// just-now timestamps render as `now`.
String relativeAge(DateTime then, {DateTime? now}) {
  final ref = now ?? DateTime.now();
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
