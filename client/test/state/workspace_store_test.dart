import 'dart:async';

import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_commander_api.dart';
import '../support/fixtures.dart';

ServerConfig _cfg(String id, String name) =>
    ServerConfig(id: id, name: name, baseUrl: 'http://$name:7878', token: 't-$id');

void main() {
  // A workspace whose per-server stores are each backed by their own fake, so
  // the aggregator can be exercised without a live bridge.
  late Map<String, FakeCommanderApi> fakes;

  WorkspaceStore build(List<ServerConfig> initial) {
    fakes = {for (final c in initial) c.id: FakeCommanderApi()};
    return WorkspaceStore(
      api: FakeCommanderApi(),
      listStore: InMemoryServerListStore(),
      storeFactory: (cfg) {
        final api = fakes.putIfAbsent(cfg.id, FakeCommanderApi.new);
        return CommanderStore(api: api, config: cfg);
      },
    );
  }

  test('aggregates the sessions of every connected server', () async {
    final a = _cfg('id-a', 'laptop');
    final b = _cfg('id-b', 'codespace');
    final ws = build([a, b]);
    fakes[a.id]!.listSessionsResponse = [
      sessionInfo(id: '11111111-1111-1111-1111-111111111111', projectName: 'A'),
    ];
    fakes[b.id]!.listSessionsResponse = [
      sessionInfo(id: '22222222-2222-2222-2222-222222222222', projectName: 'B'),
    ];

    await ws.addServer(a);
    await ws.addServer(b);

    expect(ws.servers.map((s) => s.config.name), ['laptop', 'codespace']);
    expect(ws.serverById('id-a')!.sessions.single.projectName, 'A');
    expect(ws.serverById('id-b')!.sessions.single.projectName, 'B');
    // The owning store is resolvable from a bare session id.
    expect(
      ws.storeForSession('22222222-2222-2222-2222-222222222222')!.config.id,
      'id-b',
    );
  });

  test('re-broadcasts when any child server moves', () async {
    final a = _cfg('id-a', 'laptop');
    final ws = build([a]);
    await ws.addServer(a);
    var notified = 0;
    ws.addListener(() => notified++);

    fakes[a.id]!.emitChange(); // poller change-feed tick on server a
    await Future<void>.delayed(Duration.zero);
    await Future<void>.delayed(Duration.zero);

    expect(notified, greaterThan(0));
  });

  test('removeServer disposes the store and releases its handle', () async {
    final a = _cfg('id-a', 'laptop');
    final b = _cfg('id-b', 'codespace');
    final ws = build([a, b]);
    await ws.addServer(a);
    await ws.addServer(b);

    await ws.removeServer('id-a');
    await Future<void>.delayed(Duration.zero);

    expect(ws.servers.map((s) => s.config.id), ['id-b']);
    // dispose() tears down the connection (analogue of the TUI's handle drop).
    expect(fakes['id-a']!.countOf('disconnectServer'), 1);
    // The surviving server is untouched.
    expect(fakes['id-b']!.countOf('disconnectServer'), 0);
  });

  test('loadAndConnectAll connects every saved server on relaunch', () async {
    final a = _cfg('id-a', 'laptop');
    final b = _cfg('id-b', 'codespace');
    final localFakes = {a.id: FakeCommanderApi(), b.id: FakeCommanderApi()};
    localFakes[a.id]!.listSessionsResponse = [
      sessionInfo(id: '11111111-1111-1111-1111-111111111111', projectName: 'A'),
    ];
    final ws = WorkspaceStore(
      api: FakeCommanderApi(),
      listStore: InMemoryServerListStore([a, b]),
      storeFactory: (cfg) => CommanderStore(api: localFakes[cfg.id]!, config: cfg),
    );

    await ws.loadAndConnectAll();
    await Future<void>.delayed(Duration.zero);
    await Future<void>.delayed(Duration.zero);

    // Both saved servers actually connected (not left as eternal spinners).
    expect(localFakes[a.id]!.countOf('connectServer'), 1);
    expect(localFakes[b.id]!.countOf('connectServer'), 1);
    expect(ws.serverById('id-a')!.handle, isNotNull);
    expect(ws.serverById('id-a')!.sessions.single.projectName, 'A');
  });

  test('disposing a store mid-connect releases the handle and wires no feeds',
      () async {
    final a = _cfg('id-a', 'laptop');
    final fake = FakeCommanderApi()..connectGate = Completer<void>();
    final ws = WorkspaceStore(
      api: FakeCommanderApi(),
      listStore: InMemoryServerListStore([a]),
      storeFactory: (cfg) => CommanderStore(api: fake, config: cfg),
    );

    await ws.loadAndConnectAll(); // connect starts, parks on the gate
    await Future<void>.delayed(Duration.zero);
    await ws.removeServer('id-a'); // dispose while connect is in flight
    fake.connectGate!.complete(); // connect resumes on the disposed store
    await Future<void>.delayed(Duration.zero);
    await Future<void>.delayed(Duration.zero);

    // The freshly-acquired handle is released, and no feed was ever subscribed.
    expect(fake.countOf('connectServer'), 1);
    expect(fake.countOf('disconnectServer'), 1);
    expect(fake.countOf('changeFeed'), 0);
  });

  test('a server that fails to connect stays visible without sinking others',
      () async {
    final a = _cfg('id-a', 'laptop');
    final b = _cfg('id-b', 'codespace');
    final ws = build([a, b]);
    fakes[a.id]!.workspaceSnapshotError = StateError('unreachable');
    fakes[b.id]!.listSessionsResponse = [
      sessionInfo(id: '22222222-2222-2222-2222-222222222222', projectName: 'B'),
    ];

    await ws.addServer(a);
    await ws.addServer(b);

    // Both servers are present; the failed one carries an error, the healthy one
    // still has its sessions.
    expect(ws.servers, hasLength(2));
    expect(ws.serverById('id-a')!.error, isNotNull);
    expect(ws.serverById('id-b')!.sessions, hasLength(1));
  });
}
