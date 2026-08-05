import 'dart:async';
import 'dart:typed_data';

import 'package:super_clipboard/super_clipboard.dart';

/// Reads an image off the system clipboard.
///
/// Flutter's built-in `Clipboard` handles **text only**, so an image needs a
/// platform plugin. This sits behind an interface because the real
/// implementation talks to platform channels a widget test cannot drive — tests
/// substitute a fake to exercise the paste path (see the terminal page's Ctrl+V
/// handling, which falls back to a text paste when this yields nothing).
abstract class ClipboardImageReader {
  /// Image bytes from the clipboard, or `null` when it holds no image (or the
  /// platform exposes no clipboard). Returning `null` rather than throwing is
  /// the common case — "the user pressed Ctrl+V with text on the clipboard" is
  /// ordinary, not an error.
  Future<Uint8List?> readImage();
}

/// Real reader, backed by `super_clipboard` (Linux + Android from one API).
class SuperClipboardImageReader implements ClipboardImageReader {
  const SuperClipboardImageReader({this.timeout = const Duration(seconds: 5)});

  /// Cap on a single format read. The clipboard owner supplies the data on
  /// demand, so a wedged or slow source would otherwise hang the paste
  /// indefinitely — and this runs on the keystroke path.
  final Duration timeout;

  /// Formats to try, in order. This is the **one place** the server's allow-list
  /// is mirrored outside Rust: it must stay in step with
  /// `ImageFormat` in `crates/claude-commander-protocol/src/paste.rs`, which
  /// carries a matching note pointing back here. Anything offered that the
  /// contract doesn't accept would be refused after the read; anything omitted
  /// silently can't be pasted. PNG first — it is what screenshot tools produce
  /// and it is lossless.
  static const _formats = <FileFormat>[
    Formats.png,
    Formats.jpeg,
    Formats.webp,
    Formats.gif,
    Formats.bmp,
  ];

  @override
  Future<Uint8List?> readImage() async {
    final clipboard = SystemClipboard.instance;
    if (clipboard == null) return null;
    final reader = await clipboard.read();
    for (final format in _formats) {
      // `canProvide` is a best guess — the docs warn it can say yes when the
      // data turns out to be unavailable — so a miss here just moves on to the
      // next format rather than failing the paste.
      if (!reader.canProvide(format)) continue;
      final bytes = await _readFile(reader, format);
      if (bytes != null && bytes.isNotEmpty) return bytes;
    }
    return null;
  }

  /// Bridge `getFile`'s callback API to a `Future`. A `null` `ReadProgress` means
  /// the format wasn't actually available, in which case the callback never
  /// fires — so that case must be detected from the return value, not waited on.
  Future<Uint8List?> _readFile(ClipboardReader reader, FileFormat format) {
    final done = Completer<Uint8List?>();
    void finish(Uint8List? value) {
      if (!done.isCompleted) done.complete(value);
    }

    final progress = reader.getFile(format, (file) async {
      try {
        finish(await file.readAll());
      } catch (_) {
        finish(null);
      }
    }, onError: (_) => finish(null));
    if (progress == null) return Future.value(null);
    return done.future.timeout(timeout, onTimeout: () => null);
  }
}
