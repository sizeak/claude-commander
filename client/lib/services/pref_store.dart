import 'package:shared_preferences/shared_preferences.dart';

/// A minimal string key-value store for **non-secret** device preferences — the
/// seam under [ThemeController] so its load/persist logic is testable without a
/// platform preference service.
///
/// Deliberately not [SecretKeyStore] (`server_config.dart`), which is backed by
/// the Android Keystore / libsecret and is right for bearer tokens. Routing a UI
/// preference through it would gate the first frame's theme choice on the
/// desktop keyring being unlocked.
abstract class PrefStore {
  Future<String?> read(String key);
  Future<void> write(String key, String value);
}

/// The platform preference store (`SharedPreferences` / `NSUserDefaults`).
class SharedPrefStore implements PrefStore {
  const SharedPrefStore();

  @override
  Future<String?> read(String key) async =>
      (await SharedPreferences.getInstance()).getString(key);

  @override
  Future<void> write(String key, String value) async =>
      (await SharedPreferences.getInstance()).setString(key, value);
}

/// An in-memory [PrefStore] for tests.
class InMemoryPrefStore implements PrefStore {
  final Map<String, String> _m;

  InMemoryPrefStore([Map<String, String>? initial])
    : _m = Map.of(initial ?? const {});

  @override
  Future<String?> read(String key) async => _m[key];

  @override
  Future<void> write(String key, String value) async => _m[key] = value;
}
