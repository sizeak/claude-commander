import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// The server the client talks to: base URL + bearer token.
class ServerConfig {
  final String baseUrl;
  final String token;
  const ServerConfig({required this.baseUrl, required this.token});
}

/// Persists the active [ServerConfig]. An interface so the app can inject a
/// headless in-memory store for tests/e2e — the secure store needs a running
/// platform secret service, which CI/widget tests don't have.
abstract class ServerConfigStore {
  /// Load the saved config, or null if none/incomplete.
  Future<ServerConfig?> load();

  Future<void> save(ServerConfig config);

  Future<void> clear();
}

/// Persists [ServerConfig] in the platform secure store (Android Keystore /
/// Keychain / libsecret). The token never touches plain shared-prefs.
class SecureServerConfigStore implements ServerConfigStore {
  const SecureServerConfigStore();

  static const _storage = FlutterSecureStorage();
  static const _kBaseUrl = 'base_url';
  static const _kToken = 'token';

  @override
  Future<ServerConfig?> load() async {
    final baseUrl = await _storage.read(key: _kBaseUrl);
    final token = await _storage.read(key: _kToken);
    if (baseUrl == null || baseUrl.isEmpty || token == null || token.isEmpty) {
      return null;
    }
    return ServerConfig(baseUrl: baseUrl, token: token);
  }

  @override
  Future<void> save(ServerConfig config) async {
    await _storage.write(key: _kBaseUrl, value: config.baseUrl);
    await _storage.write(key: _kToken, value: config.token);
  }

  @override
  Future<void> clear() async {
    await _storage.delete(key: _kBaseUrl);
    await _storage.delete(key: _kToken);
  }
}

/// An in-memory [ServerConfigStore] for widget tests and the Linux e2e run,
/// where no platform secret service is available.
class InMemoryServerConfigStore implements ServerConfigStore {
  ServerConfig? _config;

  InMemoryServerConfigStore([this._config]);

  @override
  Future<ServerConfig?> load() async => _config;

  @override
  Future<void> save(ServerConfig config) async => _config = config;

  @override
  Future<void> clear() async => _config = null;
}
