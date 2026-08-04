import 'dart:async';
import 'dart:typed_data';

import 'package:claude_commander_client/services/clipboard_image_reader.dart';
import 'package:claude_commander_client/services/image_picker_service.dart';
import 'package:image_picker/image_picker.dart';

/// A minimal but valid 1×1 PNG. Mirrors the fixture used by the Rust tests so
/// both sides exercise the same bytes.
final tinyPng = Uint8List.fromList(const [
  0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, //
  0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
  0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
  0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
  0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41,
  0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
  0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
  0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
  0x42, 0x60, 0x82,
]);

/// Stands in for the platform picker. Real `image_picker` needs platform
/// channels a widget test can't drive.
class FakeImagePicker implements ImagePickerService {
  FakeImagePicker({this.supportsCamera = false, this.file});

  @override
  final bool supportsCamera;

  /// What [pick] returns. `null` models the user cancelling.
  XFile? file;

  /// Sources [pick] was called with, in order.
  final List<ImagePickSource> picked = [];

  /// Build a picked file from `bytes`. `reportedLength` overrides what
  /// `length()` returns, so the size cap can be exercised without allocating
  /// tens of megabytes (`XFile.length()` prefers an explicitly supplied length).
  static XFile fileOf(
    Uint8List bytes, {
    String name = 'shot.png',
    int? reportedLength,
  }) =>
      XFile.fromData(bytes, name: name, length: reportedLength ?? bytes.length);

  @override
  Future<XFile?> pick(ImagePickSource source) async {
    picked.add(source);
    return file;
  }
}

/// Stands in for the system clipboard.
class FakeClipboardImageReader implements ClipboardImageReader {
  FakeClipboardImageReader({this.image, this.error});

  /// Bytes to return; `null` models a clipboard holding no image.
  Uint8List? image;

  /// When set, [readImage] throws this instead of returning.
  Object? error;

  /// When set, [readImage] suspends on this before returning — so a test can
  /// hold the read open and model a *second* paste arriving while the first is
  /// still mid-read. Without it, `tester.sendKeyEvent` drains microtasks between
  /// presses and the first flow completes before the second starts, which does
  /// not exercise the re-entrancy guard at all.
  Completer<void>? gate;

  int readCount = 0;

  @override
  Future<Uint8List?> readImage() async {
    readCount++;
    if (gate != null) await gate!.future;
    if (error != null) throw error!;
    return image;
  }
}
