import 'dart:convert';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:uuid/uuid.dart';

/// One server the client talks to: a stable [id], a display [name] (shown as the
/// group header in the aggregated session list), the [baseUrl], and the bearer
/// [token]. The app keeps N of these connected at once (see `WorkspaceStore`).
class ServerConfig {
  /// Stable identity, minted once when the server is first added. Survives
  /// URL/token/name edits so the live connection can be reconciled in place.
  final String id;

  /// Human label shown as the session-list group header.
  final String name;

  final String baseUrl;
  final String token;

  const ServerConfig({
    required this.id,
    required this.name,
    required this.baseUrl,
    required this.token,
  });

  ServerConfig copyWith({
    String? id,
    String? name,
    String? baseUrl,
    String? token,
  }) => ServerConfig(
    id: id ?? this.id,
    name: name ?? this.name,
    baseUrl: baseUrl ?? this.baseUrl,
    token: token ?? this.token,
  );

  /// A sensible default display name derived from a URL's host (+ port) when the
  /// user hasn't typed one — e.g. `http://100.1.2.3:7878` → `100.1.2.3:7878`.
  static String nameFromUrl(String url) {
    final uri = Uri.tryParse(url.trim());
    if (uri == null || uri.host.isEmpty) return url.trim();
    return uri.hasPort ? '${uri.host}:${uri.port}' : uri.host;
  }
}

/// A minimal keyed secret store: the seam under [ServerListStore] so its
/// load/save/migration logic is testable against an in-memory backend without a
/// platform secret service.
abstract class SecretKeyStore {
  Future<String?> read(String key);
  Future<void> write(String key, String value);
  Future<void> delete(String key);
  Future<Map<String, String>> readAll();
}

/// The platform secret store (Android Keystore / Keychain / libsecret).
class FlutterSecretKeyStore implements SecretKeyStore {
  const FlutterSecretKeyStore();
  static const _storage = FlutterSecureStorage();

  @override
  Future<String?> read(String key) => _storage.read(key: key);
  @override
  Future<void> write(String key, String value) =>
      _storage.write(key: key, value: value);
  @override
  Future<void> delete(String key) => _storage.delete(key: key);
  @override
  Future<Map<String, String>> readAll() => _storage.readAll();
}

/// An in-memory [SecretKeyStore] for tests/e2e.
class InMemorySecretKeyStore implements SecretKeyStore {
  final Map<String, String> _m;
  InMemorySecretKeyStore([Map<String, String>? initial])
    : _m = Map.of(initial ?? const {});

  @override
  Future<String?> read(String key) async => _m[key];
  @override
  Future<void> write(String key, String value) async => _m[key] = value;
  @override
  Future<void> delete(String key) async => _m.remove(key);
  @override
  Future<Map<String, String>> readAll() async => Map.of(_m);
}

/// Persists the configured [ServerConfig] list. An interface so the app can
/// inject an in-memory store for tests/e2e.
abstract class ServerListStore {
  /// Load all saved servers (empty on first run). Migrates any legacy
  /// single-server config into a one-entry list on first read.
  Future<List<ServerConfig>> load();

  /// Persist the full server list (metadata + per-server tokens), replacing
  /// whatever was stored.
  Future<void> save(List<ServerConfig> servers);
}

/// Persists the server list into a [SecretKeyStore]. Server metadata (id/name/
/// url) lives under one JSON key; each token lives under its own `token:<id>`
/// key so a token never shares a blob with non-secret data and is dropped
/// precisely on removal. [SecureServerListStore] wires this to the platform
/// store; tests wire it to an [InMemorySecretKeyStore].
class KeyedServerListStore implements ServerListStore {
  final SecretKeyStore _secrets;
  final Uuid _uuid;
  KeyedServerListStore(this._secrets, {Uuid uuid = const Uuid()}) : _uuid = uuid;

  /// The JSON metadata list: `[{id,name,baseUrl}]`.
  static const _kServers = 'servers_v1';
  static const _tokenPrefix = 'token:';
  static String _kToken(String id) => '$_tokenPrefix$id';

  // Legacy single-server keys, migrated into the list on first read.
  static const _kLegacyBaseUrl = 'base_url';
  static const _kLegacyToken = 'token';

  @override
  Future<List<ServerConfig>> load() async {
    final raw = await _secrets.read(_kServers);
    if (raw != null && raw.isNotEmpty) {
      return _parse(raw);
    }
    // Migration: fold a legacy single-server config into a one-entry list, then
    // delete the legacy keys so this runs exactly once (shape-consumed).
    final migrated = await _migrateLegacy();
    if (migrated != null) {
      await save([migrated]);
      await _secrets.delete(_kLegacyBaseUrl);
      await _secrets.delete(_kLegacyToken);
      return [migrated];
    }
    return const [];
  }

  Future<List<ServerConfig>> _parse(String raw) async {
    final list = (jsonDecode(raw) as List).cast<Map<String, dynamic>>();
    final servers = <ServerConfig>[];
    for (final m in list) {
      final id = m['id'] as String;
      final baseUrl = m['baseUrl'] as String;
      final token = await _secrets.read(_kToken(id)) ?? '';
      servers.add(
        ServerConfig(
          id: id,
          name: m['name'] as String? ?? ServerConfig.nameFromUrl(baseUrl),
          baseUrl: baseUrl,
          token: token,
        ),
      );
    }
    return servers;
  }

  Future<ServerConfig?> _migrateLegacy() async {
    final baseUrl = await _secrets.read(_kLegacyBaseUrl);
    final token = await _secrets.read(_kLegacyToken);
    if (baseUrl == null || baseUrl.isEmpty || token == null || token.isEmpty) {
      return null;
    }
    return ServerConfig(
      id: _uuid.v4(),
      name: ServerConfig.nameFromUrl(baseUrl),
      baseUrl: baseUrl,
      token: token,
    );
  }

  @override
  Future<void> save(List<ServerConfig> servers) async {
    final meta = [
      for (final s in servers)
        {'id': s.id, 'name': s.name, 'baseUrl': s.baseUrl},
    ];
    await _secrets.write(_kServers, jsonEncode(meta));
    for (final s in servers) {
      await _secrets.write(_kToken(s.id), s.token);
    }
    // Drop tokens for any server no longer present, so a removed server leaves
    // nothing behind.
    final liveIds = {for (final s in servers) s.id};
    final all = await _secrets.readAll();
    for (final key in all.keys) {
      if (key.startsWith(_tokenPrefix) &&
          !liveIds.contains(key.substring(_tokenPrefix.length))) {
        await _secrets.delete(key);
      }
    }
  }
}

/// The production [ServerListStore]: the keyed store backed by the platform
/// secret service.
class SecureServerListStore extends KeyedServerListStore {
  SecureServerListStore() : super(const FlutterSecretKeyStore());
}

/// A simple in-memory [ServerListStore] for widget tests and the Linux e2e run
/// that only need to hold a list (no migration behaviour).
class InMemoryServerListStore implements ServerListStore {
  List<ServerConfig> _servers;

  InMemoryServerListStore([List<ServerConfig>? servers])
    : _servers = List.of(servers ?? const []);

  @override
  Future<List<ServerConfig>> load() async => List.of(_servers);

  @override
  Future<void> save(List<ServerConfig> servers) async =>
      _servers = List.of(servers);
}
