import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/services/pref_store.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/fleet_store.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_commander_api.dart';
import '../support/fixtures.dart';

ServerConfig _cfg(String id, String name) => ServerConfig(
  id: id,
  name: name,
  baseUrl: 'http://$name:7878',
  token: 't-$id',
);

const _pa = 'aaaaaaaa-0000-0000-0000-000000000001';
const _pb = 'aaaaaaaa-0000-0000-0000-000000000002';
const _pc = 'aaaaaaaa-0000-0000-0000-000000000003';

/// The fleet's workspace layer: the merged list across servers, the active
/// workspace (per device, honouring the servers' `startup_workspace`), the
/// scoped views each store offers, and the eager fan-out of every edit.
void main() {
  late Map<String, FakeCommanderApi> fakes;
  late InMemoryPrefStore prefs;

  final a = _cfg('id-a', 'laptop');
  final b = _cfg('id-b', 'codespace');

  FleetStore build({Map<String, String>? stored}) {
    fakes = {a.id: FakeCommanderApi(), b.id: FakeCommanderApi()};
    prefs = InMemoryPrefStore(stored);
    return FleetStore(
      api: FakeCommanderApi(),
      listStore: InMemoryServerListStore(),
      prefs: prefs,
      storeFactory: (cfg) => CommanderStore(api: fakes[cfg.id]!, config: cfg),
    );
  }

  /// Two servers: `laptop` defines Work (orange) and has a Work project and a
  /// Main one; `codespace` defines Personal and Work (no colour) with a
  /// Personal project.
  Future<FleetStore> twoServers({Map<String, String>? stored}) async {
    final fleet = build(stored: stored);
    await fleet.loadWorkspacePref();
    fakes[a.id]!
      ..workspacesResponse = const [WorkspaceDef(name: 'Work')]
      ..projectsResponse = [
        projectInfo(id: _pa, name: 'work-repo', workspace: 'Work'),
        projectInfo(id: _pb, name: 'main-repo'),
      ]
      ..listSessionsResponse = [
        sessionInfo(
          id: '11111111-0000-0000-0000-000000000001',
          title: 'work session',
          projectId: _pa,
        ),
        sessionInfo(
          id: '11111111-0000-0000-0000-000000000002',
          title: 'main session',
          projectId: _pb,
        ),
      ];
    fakes[b.id]!
      ..workspacesResponse = const [
        WorkspaceDef(name: 'Personal'),
        WorkspaceDef(name: 'Work'),
      ]
      ..projectsResponse = [
        projectInfo(id: _pc, name: 'side-repo', workspace: 'Personal'),
      ]
      ..listSessionsResponse = [
        sessionInfo(
          id: '22222222-0000-0000-0000-000000000001',
          title: 'side session',
          projectId: _pc,
        ),
      ];
    await fleet.addServer(a);
    await fleet.addServer(b);
    return fleet;
  }

  test(
    'merges workspaces across servers, Main first, first server first',
    () async {
      final fleet = await twoServers();
      expect(fleet.workspaces.map((w) => w.name), [null, 'Work', 'Personal']);
      expect(fleet.workspaces.first.label, 'Main');
      expect(fleet.workspacesVisible, isTrue);
    },
  );

  test('a lone Main workspace keeps the switcher hidden', () async {
    final fleet = build();
    await fleet.addServer(a);
    expect(fleet.workspaces.map((w) => w.name), [null]);
    expect(fleet.workspacesVisible, isFalse);
    expect(fleet.activeWorkspace, isNull);
  });

  test('opens on Main when nothing was stored', () async {
    final fleet = await twoServers();
    expect(fleet.activeWorkspace, isNull);
  });

  test('startup "last" reopens the workspace this device last had', () async {
    final fleet = await twoServers(
      stored: {FleetStore.lastWorkspaceKey: 'Personal'},
    );
    expect(fleet.activeWorkspace, 'Personal');
  });

  test('a server pinning startup to a name wins over the stored one', () async {
    final fleet = build(stored: {FleetStore.lastWorkspaceKey: 'Personal'});
    await fleet.loadWorkspacePref();
    fakes[a.id]!
      ..workspacesResponse = const [WorkspaceDef(name: 'Work')]
      ..startupWorkspaceResponse = 'Work';
    fakes[b.id]!.workspacesResponse = const [WorkspaceDef(name: 'Personal')];
    await fleet.addServer(a);
    await fleet.addServer(b);
    expect(fleet.activeWorkspace, 'Work');
  });

  test('a server pinning startup to main opens on Main', () async {
    final fleet = build(stored: {FleetStore.lastWorkspaceKey: 'Work'});
    await fleet.loadWorkspacePref();
    fakes[a.id]!
      ..workspacesResponse = const [WorkspaceDef(name: 'Work')]
      ..startupWorkspaceResponse = 'main';
    await fleet.addServer(a);
    expect(fleet.activeWorkspace, isNull);
  });

  test(
    'selecting a workspace persists it and outranks the startup pin',
    () async {
      final fleet = await twoServers();
      fakes[a.id]!.startupWorkspaceResponse = 'Work';
      await fleet.refreshAll();
      expect(fleet.activeWorkspace, 'Work');

      await fleet.selectWorkspace('Personal');
      expect(fleet.activeWorkspace, 'Personal');
      expect(await prefs.read(FleetStore.lastWorkspaceKey), 'Personal');

      await fleet.selectWorkspace(null);
      expect(fleet.activeWorkspace, isNull);
      expect(await prefs.read(FleetStore.lastWorkspaceKey), '');
    },
  );

  test('an active workspace that disappears falls back to Main', () async {
    final fleet = await twoServers();
    await fleet.selectWorkspace('Personal');
    fakes[b.id]!
      ..workspacesResponse = const []
      ..projectsResponse = [projectInfo(id: _pc, name: 'side-repo')];
    await fleet.refreshAll();
    expect(fleet.activeWorkspace, isNull);
  });

  test('stores scope their projects and sessions to a workspace', () async {
    final fleet = await twoServers();
    final laptop = fleet.serverById(a.id)!;
    expect(laptop.projectsIn('Work').map((p) => p.name), ['work-repo']);
    expect(laptop.projectsIn(null).map((p) => p.name), ['main-repo']);
    expect(laptop.sessionsIn('Work').map((s) => s.title), ['work session']);
    expect(laptop.sessionsByProjectIn(null).map((g) => g.project.name), [
      'main-repo',
    ]);
    expect(fleet.serverById(b.id)!.sessionsIn('Work'), isEmpty);
  });

  test('counts sessions waiting for input per workspace', () async {
    final fleet = await twoServers();
    fakes[a.id]!.agentStatesResponse = AgentStatesSnapshotDto(
      states: [
        AgentStateEntryDto(
          sessionId: sessionInfo(
            id: '11111111-0000-0000-0000-000000000001',
          ).sessionId,
          state: AgentState.waitingForInput,
        ),
      ],
      commanderRunning: false,
    );
    fakes[b.id]!.agentStatesResponse = AgentStatesSnapshotDto(
      states: [
        AgentStateEntryDto(
          sessionId: sessionInfo(
            id: '22222222-0000-0000-0000-000000000001',
          ).sessionId,
          state: AgentState.waitingForInput,
        ),
      ],
      commanderRunning: false,
    );
    await fleet.refreshAll();
    expect(
      {for (final w in fleet.waitingCounts) w.workspace.name: w.waiting},
      {null: 0, 'Work': 1, 'Personal': 1},
    );
  });

  group('edits fan out to every server', () {
    test('creating a workspace sends the whole merged list to each', () async {
      final fleet = await twoServers();
      final failures = await fleet.createWorkspace('Side');
      expect(failures, isEmpty);
      for (final fake in fakes.values) {
        final req =
            fake.lastCall('setWorkspaces')!.args['request']!
                as SetWorkspacesRequestDto;
        expect(req.workspaces.map((w) => w.name), ['Work', 'Personal', 'Side']);
        // Main and the startup choice are left alone unless edited.
        expect(req.main, isNull);
        expect(req.startupWorkspace, isNull);
      }
      expect(fleet.workspaces.map((w) => w.name), contains('Side'));
    });

    test(
      'a refusing server is named in the failures; the rest apply',
      () async {
        final fleet = await twoServers();
        fakes[b.id]!.workspaceMutationError = Exception('boom');
        final failures = await fleet.createWorkspace('Side');
        expect(failures.map((f) => f.server), ['codespace']);
        expect(fakes[a.id]!.countOf('setWorkspaces'), 1);
      },
    );

    test('rename goes to every server and follows the active one', () async {
      final fleet = await twoServers();
      await fleet.selectWorkspace('Work');
      final failures = await fleet.renameWorkspace('Work', 'Job');
      expect(failures, isEmpty);
      for (final fake in fakes.values) {
        expect(
          fake.lastCall('renameWorkspace')!.args,
          containsPair('to', 'Job'),
        );
      }
      expect(fleet.activeWorkspace, 'Job');
      expect(fleet.serverById(a.id)!.projectsIn('Job').map((p) => p.name), [
        'work-repo',
      ]);
    });

    test(
      'the active workspace never reads as Main while a rename lands',
      () async {
        // Each server's refresh notifies; the first one to land drops the old
        // name. The active workspace must follow the rename at once rather than
        // fall back to Main until the fan-out finishes.
        final fleet = await twoServers();
        await fleet.selectWorkspace('Work');
        final seen = <String?>[];
        fleet.addListener(() => seen.add(fleet.activeWorkspace));
        await fleet.renameWorkspace('Work', 'Job');
        expect(seen, isNotEmpty);
        expect(
          seen.where((w) => w != 'Work' && w != 'Job'),
          isEmpty,
          reason: '$seen',
        );
        expect(fleet.activeWorkspace, 'Job');
      },
    );

    test(
      'a rename every server refuses leaves the active workspace put',
      () async {
        final fleet = await twoServers();
        await fleet.selectWorkspace('Work');
        for (final fake in fakes.values) {
          fake.workspaceMutationError = Exception('boom');
        }
        final failures = await fleet.renameWorkspace('Work', 'Job');
        expect(failures, hasLength(2));
        expect(fleet.activeWorkspace, 'Work');
        expect(await prefs.read(FleetStore.lastWorkspaceKey), 'Work');
      },
    );

    test('delete goes to every server and drops back to Main', () async {
      final fleet = await twoServers();
      await fleet.selectWorkspace('Work');
      await fleet.deleteWorkspace('Work');
      expect(
        fakes.values.every((f) => f.countOf('deleteWorkspace') == 1),
        isTrue,
      );
      expect(fleet.activeWorkspace, isNull);
      expect(fleet.serverById(a.id)!.projectsIn(null).map((p) => p.name), [
        'work-repo',
        'main-repo',
      ]);
    });

    test('reorder, relabel Main and pin the startup', () async {
      final fleet = await twoServers();
      await fleet.reorderWorkspaces(['Personal', 'Work']);
      expect(
        (fakes[a.id]!.lastCall('setWorkspaces')!.args['request']!
                as SetWorkspacesRequestDto)
            .workspaces
            .map((w) => w.name),
        ['Personal', 'Work'],
      );

      await fleet.renameMainWorkspace('Home');
      final mainReq =
          fakes[b.id]!.lastCall('setWorkspaces')!.args['request']!
              as SetWorkspacesRequestDto;
      expect(mainReq.main?.name, 'Home');
      expect(fleet.workspaces.first.label, 'Home');

      await fleet.setStartupWorkspace('Personal');
      final startupReq =
          fakes[a.id]!.lastCall('setWorkspaces')!.args['request']!
              as SetWorkspacesRequestDto;
      expect(startupReq.startupWorkspace, 'Personal');
      expect(fleet.startupWorkspace, 'Personal');
    });

    test('moving a project writes only to its owning server', () async {
      final fleet = await twoServers();
      final laptop = fleet.serverById(a.id)!;
      final failures = await fleet.moveProject(laptop, _pb, 'Personal');
      expect(failures, isEmpty);
      expect(
        fakes[a.id]!.lastCall('setProjectWorkspace')!.args,
        containsPair('workspace', 'Personal'),
      );
      expect(fakes[b.id]!.countOf('setProjectWorkspace'), 0);
      expect(laptop.projectsIn('Personal').map((p) => p.name), ['main-repo']);
    });
  });
}
