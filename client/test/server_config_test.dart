import 'dart:convert';

import 'package:claude_commander_client/server_config.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('KeyedServerListStore migration', () {
    test('folds a legacy base_url/token into a one-entry list', () async {
      final secrets = InMemorySecretKeyStore({
        'base_url': 'http://100.1.2.3:7878',
        'token': 'legacy-secret',
      });
      final store = KeyedServerListStore(secrets);

      final servers = await store.load();

      expect(servers, hasLength(1));
      expect(servers.single.baseUrl, 'http://100.1.2.3:7878');
      expect(servers.single.token, 'legacy-secret');
      // Name derived from host:port when none was stored.
      expect(servers.single.name, '100.1.2.3:7878');
      expect(servers.single.id, isNotEmpty);
    });

    test('clears the legacy keys after migrating (runs once)', () async {
      final secrets = InMemorySecretKeyStore({
        'base_url': 'http://host:7878',
        'token': 'legacy-secret',
      });
      final store = KeyedServerListStore(secrets);

      await store.load();

      expect(await secrets.read('base_url'), isNull);
      expect(await secrets.read('token'), isNull);
      // A second load reads the migrated list, not the (now absent) legacy keys.
      final second = await store.load();
      expect(second, hasLength(1));
      expect(second.single.token, 'legacy-secret');
    });

    test('returns empty on a truly fresh install', () async {
      final store = KeyedServerListStore(InMemorySecretKeyStore());
      expect(await store.load(), isEmpty);
    });
  });

  group('KeyedServerListStore round-trip', () {
    test('saves metadata as JSON and tokens under per-id keys', () async {
      final secrets = InMemorySecretKeyStore();
      final store = KeyedServerListStore(secrets);
      final servers = [
        const ServerConfig(
          id: 'id-a',
          name: 'laptop',
          baseUrl: 'http://a:7878',
          token: 'tok-a',
        ),
        const ServerConfig(
          id: 'id-b',
          name: 'codespace',
          baseUrl: 'http://b:7878',
          token: 'tok-b',
        ),
      ];

      await store.save(servers);

      final meta = jsonDecode((await secrets.read('servers_v1'))!) as List;
      expect(meta, hasLength(2));
      expect(meta.first['name'], 'laptop');
      // Tokens live in their own keys, not in the metadata blob.
      expect(meta.first.containsKey('token'), isFalse);
      expect(await secrets.read('token:id-a'), 'tok-a');
      expect(await secrets.read('token:id-b'), 'tok-b');

      final loaded = await store.load();
      expect(loaded.map((s) => s.id), ['id-a', 'id-b']);
      expect(loaded.map((s) => s.token), ['tok-a', 'tok-b']);
    });

    test('a removed server leaves no token behind', () async {
      final secrets = InMemorySecretKeyStore();
      final store = KeyedServerListStore(secrets);
      await store.save(const [
        ServerConfig(id: 'id-a', name: 'a', baseUrl: 'http://a', token: 'tok-a'),
        ServerConfig(id: 'id-b', name: 'b', baseUrl: 'http://b', token: 'tok-b'),
      ]);

      // Drop server b.
      await store.save(const [
        ServerConfig(id: 'id-a', name: 'a', baseUrl: 'http://a', token: 'tok-a'),
      ]);

      expect(await secrets.read('token:id-a'), 'tok-a');
      expect(await secrets.read('token:id-b'), isNull);
    });
  });
}
