import 'package:flutter/foundation.dart';
// Also the source of `XFile` (re-exported), the type this seam returns.
import 'package:image_picker/image_picker.dart';

/// Where an attached image comes from.
enum ImagePickSource {
  /// The platform photo/file picker.
  gallery,

  /// A live camera capture. Only offered where [ImagePickerService.supportsCamera]
  /// is true.
  camera,
}

/// Picks an image file from the platform.
///
/// Behind an interface because `image_picker` drives real platform channels a
/// widget test cannot exercise. Returns [XFile] — `image_picker`'s own type —
/// rather than inventing a parallel model: it exposes `length()` separately from
/// `readAsBytes()`, which lets a caller reject an oversized pick without pulling
/// a large phone photo into memory first. Tests substitute a fake built from
/// `XFile.fromData`.
abstract class ImagePickerService {
  /// Whether a camera capture is available. False on desktop, where
  /// `image_picker` delegates to a file dialog and explicitly does not support
  /// `ImageSource.camera`.
  bool get supportsCamera;

  /// The picked file, or `null` if the user cancelled.
  Future<XFile?> pick(ImagePickSource source);
}

/// Real picker: the native Android photo picker / camera, or a GTK file dialog
/// on Linux (via `image_picker`'s endorsed `image_picker_linux`, which delegates
/// to `file_selector`).
class PlatformImagePicker implements ImagePickerService {
  PlatformImagePicker();

  final ImagePicker _picker = ImagePicker();

  @override
  bool get supportsCamera =>
      defaultTargetPlatform == TargetPlatform.android ||
      defaultTargetPlatform == TargetPlatform.iOS;

  @override
  Future<XFile?> pick(ImagePickSource source) => _picker.pickImage(
    source: source == ImagePickSource.camera
        ? ImageSource.camera
        : ImageSource.gallery,
    // Deliberately no `imageQuality`/`maxWidth`: those force a lossy re-encode,
    // and these images are usually screenshots of code or terminals that an
    // agent has to *read*. JPEG artefacts on small text are exactly what we
    // must not introduce; an oversized pick is rejected instead.
  );
}
