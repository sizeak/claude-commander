// Flutter-free rendering of a thrown error as one short human-facing line.
//
// Every failure that crosses the cdylib arrives as flutter_rust_bridge's
// `AnyhowException`, whose message is `format!("{:?}", anyhow::Error)` — the
// `Debug` form, i.e. the whole `Caused by:` chain *and* a captured
// `Stack backtrace:` when the Rust runtime has one (it does on Android release
// builds). Rendering that with `toString()` put a ten-frame backtrace on the
// session list the moment a server was unreachable, so the unwrapping lives in
// one place rather than at each render site.

/// The wrapper frb's Dart-side exception prints around its message
/// (`AnyhowException(<message>)`).
const _anyhowPrefix = 'AnyhowException(';

/// Markers that begin the developer-facing tail of a Rust error's `Debug` form.
/// Everything from the first one onwards is dropped.
const _tailMarkers = ['Stack backtrace:', 'stack backtrace:', 'Caused by:'];

/// Prefixes that only restate the category the UI already conveys (a cloud-off
/// icon, a red "failed" label), stripped so the reason itself gets the space.
const _noisePrefixes = ['backend unavailable: ', 'Exception: '];

/// Shown when an error carries no usable text at all.
const _fallback = 'Something went wrong';

/// A short, single-line rendering of [error] fit for a snackbar, an inline note
/// or a status strip.
///
/// Unwraps frb's `AnyhowException(...)`, drops the `Caused by:` / backtrace
/// tail, keeps the first line, strips a redundant category prefix, and clamps
/// the result to [maxLength]. Accepts a plain `String` too, so a connection
/// state's `reason` (already a bare message) goes through the same funnel.
///
/// [capitalize] sentence-cases the first letter — wanted where the text stands
/// alone, unwanted where it is appended to a label ("Degraded: …").
String errorText(Object error, {bool capitalize = true, int maxLength = 160}) {
  var text = error.toString().trim();

  if (text.startsWith(_anyhowPrefix)) {
    text = text.substring(_anyhowPrefix.length);
    if (text.endsWith(')')) text = text.substring(0, text.length - 1);
    text = text.trim();
  }

  for (final marker in _tailMarkers) {
    final at = text.indexOf(marker);
    if (at >= 0) text = text.substring(0, at);
  }

  text = text.trim().split('\n').first.trim();

  for (final prefix in _noisePrefixes) {
    if (text.toLowerCase().startsWith(prefix.toLowerCase())) {
      text = text.substring(prefix.length).trim();
      break;
    }
  }

  if (text.isEmpty) return _fallback;
  if (capitalize) text = text[0].toUpperCase() + text.substring(1);
  if (text.length > maxLength) {
    text = '${text.substring(0, maxLength).trimRight()}…';
  }
  return text;
}
