import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:uuid/uuid.dart';

import '../support/fake_commander_api.dart';
import '../support/fixtures.dart';

void main() {
  late FakeCommanderApi api;

  setUp(() => api = FakeCommanderApi());

  const otherConfig = ServerConfig(
    id: 'other-server',
    name: 'other',
    baseUrl: 'http://other.test:9999',
    token: 'other-token',
  );

  const id = '11111111-2222-3333-4444-555555555555';

  AgentStatesSnapshotDto statesWith(AgentState state) => AgentStatesSnapshotDto(
    states: [
      AgentStateEntryDto(
        sessionId: SessionId(field0: UuidValue.fromString(id)),
        state: state,
      ),
    ],
    commanderRunning: true,
  );

  CommanderStore build() => CommanderStore(api: api, config: testConfig);

  test('connect() acquires the handle and populates workspace + agent states', () async {
    api.connectServerResponse = 'handle-1';
    api.listSessionsResponse = [sessionInfo(id: id, title: 'Alpha')];
    api.agentStatesResponse = statesWith(AgentState.working);

    final store = build();
    addTearDown(store.dispose);
    await store.connect();

    expect(store.handle, 'handle-1');
    expect(store.sessions.map((s) => s.title), ['Alpha']);
    expect(store.agentStateFor(id), AgentState.working);
    expect(store.commanderRunning, isTrue);
    expect(store.loading, isFalse);
    expect(store.error, isNull);
    // The feeds were subscribed with the acquired handle.
    expect(api.lastCall('changeFeed')!.args['handle'], 'handle-1');
    expect(api.lastCall('connectionFeed')!.args['handle'], 'handle-1');
  });

  test('a change-feed tick triggers a refetch and notifies listeners', () async {
    api.listSessionsResponse = [sessionInfo(id: id, title: 'Before')];
    final store = build();
    addTearDown(store.dispose);
    await store.connect();

    var notifications = 0;
    store.addListener(() => notifications++);
    final refetchesBefore = api.countOf('workspaceSnapshot');

    // The server state moved: the next snapshot has a new title.
    api.listSessionsResponse = [sessionInfo(id: id, title: 'After')];
    api.emitChange();
    await pumpEventQueue();

    expect(api.countOf('workspaceSnapshot'), greaterThan(refetchesBefore));
    expect(store.sessions.single.title, 'After');
    expect(notifications, greaterThan(0));
  });

  test('a connection-feed event updates connection and notifies', () async {
    final store = build();
    addTearDown(store.dispose);
    await store.connect();

    var notified = false;
    store.addListener(() => notified = true);

    api.emitConnection(
      const ConnectionStateDto(
        kind: ConnectionStateKind.degraded,
        reason: 'slow link',
      ),
    );
    await pumpEventQueue();

    expect(store.connection.kind, ConnectionStateKind.degraded);
    expect(store.connection.reason, 'slow link');
    expect(notified, isTrue);
  });

  test('reconnect() disconnects the OLD handle before connecting the new one', () async {
    api.connectServerResponse = 'handle-1';
    final store = build();
    addTearDown(store.dispose);
    await store.connect();
    expect(store.handle, 'handle-1');

    api.connectServerResponse = 'handle-2';
    await store.reconnect(otherConfig);

    expect(store.handle, 'handle-2');
    expect(store.config, otherConfig);

    // The old handle was released, and that release happened BEFORE the second
    // connect — so no handle is ever left dangling in the cdylib registry.
    final disconnectIdx = api.calls.indexWhere(
      (c) =>
          c.method == 'disconnectServer' && c.args['handle'] == 'handle-1',
    );
    final connectIdxs = [
      for (var i = 0; i < api.calls.length; i++)
        if (api.calls[i].method == 'connectServer') i,
    ];
    expect(disconnectIdx, isNonNegative);
    expect(connectIdxs.length, 2);
    expect(disconnectIdx, lessThan(connectIdxs[1]));
    // The new handle's feeds are live; the old one's are gone.
    expect(api.lastCall('changeFeed')!.args['handle'], 'handle-2');
  });

  test('dispose() releases the handle', () async {
    api.connectServerResponse = 'handle-1';
    final store = build();
    await store.connect();

    store.dispose();
    await pumpEventQueue();

    expect(
      api.calls.any(
        (c) =>
            c.method == 'disconnectServer' && c.args['handle'] == 'handle-1',
      ),
      isTrue,
    );
  });
}
