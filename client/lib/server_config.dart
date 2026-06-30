import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// The server the client talks to: base URL + bearer token.
class ServerConfig {
  final String baseUrl;
  final String token;
  const ServerConfig({required this.baseUrl, required this.token});
}

/// Persists [ServerConfig] in the platform secure store (Android Keystore /
/// Keychain / libsecret). The token never touches plain shared-prefs.
class ServerConfigStore {
  ServerConfigStore._();

  static const _storage = FlutterSecureStorage();
  static const _kBaseUrl = 'base_url';
  static const _kToken = 'token';

  /// Load the saved config, or null if none/incomplete.
  static Future<ServerConfig?> load() async {
    final baseUrl = await _storage.read(key: _kBaseUrl);
    final token = await _storage.read(key: _kToken);
    if (baseUrl == null || baseUrl.isEmpty || token == null || token.isEmpty) {
      return null;
    }
    return ServerConfig(baseUrl: baseUrl, token: token);
  }

  static Future<void> save(ServerConfig config) async {
    await _storage.write(key: _kBaseUrl, value: config.baseUrl);
    await _storage.write(key: _kToken, value: config.token);
  }

  static Future<void> clear() async {
    await _storage.delete(key: _kBaseUrl);
    await _storage.delete(key: _kToken);
  }
}
