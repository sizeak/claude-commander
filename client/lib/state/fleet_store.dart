import 'dart:async';

import 'package:flutter/foundation.dart';

import '../server_config.dart';
import '../services/commander_api.dart';
import '../services/pref_store.dart';
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/workspace.dart';
import '../util/error_text.dart';
import 'commander_store.dart';

/// Builds the per-server [CommanderStore] for one [ServerConfig]. Injected so
/// tests can back each server with its own fake — the direct analogue of the
/// TUI's `RemoteBackendFactory`.
typedef CommanderStoreFactory = CommanderStore Function(ServerConfig config);

/// One merged workspace with how many of its sessions are waiting for input —
/// the number the workspace picker shows beside each entry.
class WorkspaceWaiting {
  final MergedWorkspace workspace;
  final int waiting;
  const WorkspaceWaiting(this.workspace, this.waiting);
}

/// A server that refused (or could not be sent) a workspace edit. The edit is
/// best-effort across servers, so the rest applied; the UI names these.
class WorkspaceEditFailure {
  final String server;
  final Object error;
  const WorkspaceEditFailure(this.server, this.error);

  @override
  String toString() => '$server: ${errorText(error, capitalize: false)}';
}

/// The one-line summary a snackbar shows for a partly-failed workspace edit,
/// or null when every server took it.
String? describeWorkspaceFailures(List<WorkspaceEditFailure> failures) {
  if (failures.isEmpty) return null;
  return "Couldn't update ${failures.join('; ')}";
}

/// The reactive aggregator over every configured server. It owns one
/// [CommanderStore] per server (each holding its own opaque handle, poller, and
/// change/connection feeds) and re-broadcasts whenever any of them moves, so the
/// aggregated session list rebuilds on any server's update.
///
/// Sessions are rendered grouped by server, so each row already sits under its
/// owning [CommanderStore] — there is no global id namespacing; a widget acts on
/// the store for its group. This mirrors the TUI's `backends: Vec<BackendHandle>`
/// model (minus the local backend — a phone is a pure remote client).
class FleetStore extends ChangeNotifier {
  FleetStore({
    required CommanderApi api,
    required ServerListStore listStore,
    CommanderStoreFactory? storeFactory,
    PrefStore? prefs,
  }) : _api = api,
       _listStore = listStore,
       _prefs = prefs,
       _storeFactory =
           storeFactory ?? ((cfg) => CommanderStore(api: api, config: cfg));

  /// Test seam: wrap a fleet around already-constructed [stores] without
  /// loading from disk or auto-connecting, so a widget test can drive each
  /// store's lifecycle (connect timing, injected errors) directly.
  @visibleForTesting
  FleetStore.withStores(List<CommanderStore> stores, {PrefStore? prefs})
    : assert(stores.isNotEmpty, 'withStores needs at least one store'),
      _api = stores.first.api,
      _listStore = InMemoryServerListStore(),
      _prefs = prefs,
      _storeFactory = ((_) =>
          throw UnsupportedError('withStores does not add servers')) {
    for (final s in stores) {
      s.addListener(_onChildChanged);
      _stores.add(s);
    }
  }

  final CommanderApi _api;
  final ServerListStore _listStore;
  final CommanderStoreFactory _storeFactory;

  /// Where the device's last active workspace is kept. Null (tests that don't
  /// care) keeps it in memory only.
  final PrefStore? _prefs;

  /// The preference key for the last active workspace. The value is the name,
  /// or empty for Main.
  static const lastWorkspaceKey = 'commander.workspace.last';

  /// The shared bridge seam, exposed so the add-server form can probe a server
  /// (`health`) before any per-server store exists.
  CommanderApi get api => _api;

  final List<CommanderStore> _stores = [];
  bool _disposed = false;

  /// The connected servers, in configured order. Each carries its own identity
  /// (`config.id`/`config.name`), connection state, snapshot, and mutations.
  List<CommanderStore> get servers => List.unmodifiable(_stores);

  /// True once at least one server is configured. When false the UI shows the
  /// add-server screen (first run).
  bool get isEmpty => _stores.isEmpty;

  /// The store for a server id, or null if none matches.
  CommanderStore? serverById(String id) {
    for (final s in _stores) {
      if (s.config.id == id) return s;
    }
    return null;
  }

  /// The owning store for a session id, found by scanning each server's
  /// snapshot. Session ids are globally-unique UUIDs, so at most one matches.
  CommanderStore? storeForSession(String sessionId) {
    for (final s in _stores) {
      if (s.sessionById(sessionId) != null) return s;
    }
    return null;
  }

  // --- workspaces ---------------------------------------------------------
  //
  // A workspace is a label on a project, stored by each server for its own
  // projects. The app shows one list merged across servers (by name, first
  // configured server first) and filters its views by the active one. Which one
  // is active is this device's business: remembered in [_prefs], and chosen at
  // launch by the servers' `startup_workspace`.

  /// The last workspace this device had active, as loaded from / written to
  /// [_prefs]. Null is Main (or nothing stored).
  String? _last;

  /// Whether the user has picked a workspace since launch. Until they do, the
  /// servers' `startup_workspace` decides; after, their pick does.
  bool _pickedThisRun = false;

  List<MergedWorkspace>? _merged;
  String? _active;
  bool _activeResolved = false;

  /// Read the stored last-active workspace. Called by [loadAndConnectAll]; a
  /// test built with [FleetStore.withStores] calls it directly. A preference
  /// store that fails to read leaves the device on Main rather than failing.
  Future<void> loadWorkspacePref() async {
    try {
      final stored = await _prefs?.read(lastWorkspaceKey);
      _last = (stored == null || stored.isEmpty) ? null : stored;
    } catch (_) {
      _last = null;
    }
    _invalidateWorkspaces();
  }

  /// Every workspace across the connected servers, Main first. Merged by the
  /// shared viewmodel rule (through [CommanderApi.mergeWorkspaces]) so the app
  /// lists them exactly as the TUI does.
  List<MergedWorkspace> get workspaces => _merged ??= _api.mergeWorkspaces([
    for (final s in _stores)
      if (s.snapshot case final snap?)
        WorkspaceSourceDto(
          defs: snap.workspaces,
          main: snap.mainWorkspace,
          projectTags: [for (final p in snap.projects) ?p.workspace],
        ),
  ]);

  /// Whether there is anything to switch between. Everything workspace-shaped
  /// in the UI (the picker, the header suffix) hides until there are two.
  bool get workspacesVisible => workspaces.length >= 2;

  /// The first loaded server's `startup_workspace` (`last`, `main` or a name);
  /// `last` until one has loaded. The first server leads for the same reason
  /// its definitions order the merged list.
  String get startupWorkspace {
    for (final s in _stores) {
      if (s.snapshot case final snap?) return snap.startupWorkspace;
    }
    return 'last';
  }

  /// The active workspace's name, or null for Main. A workspace that no longer
  /// exists (deleted, or its server removed) reads as Main.
  String? get activeWorkspace {
    if (!_activeResolved) {
      _active = _api.resolveStartupWorkspace(
        startup: _pickedThisRun ? 'last' : _renamed(startupWorkspace)!,
        last: _renamed(_last),
        workspaces: workspaces,
      );
      _activeResolved = true;
    }
    return _active;
  }

  /// A rename [renameWorkspace] is fanning out, if any.
  ({String from, String to})? _renaming;

  /// [name], or the name it is being renamed to once the merged list has
  /// dropped it for that name. The servers' refreshes land one at a time and
  /// each notifies, so without this the active workspace would read as Main
  /// from the first refresh until the fan-out finished — and the app would
  /// pass through Main's theme on a rename that changes no colours.
  String? _renamed(String? name) {
    final r = _renaming;
    if (r == null || name != r.from) return name;
    final names = workspaces.map((w) => w.name);
    return !names.contains(r.from) && names.contains(r.to) ? r.to : name;
  }

  /// The merged entry for [activeWorkspace] (Main when it resolves to none).
  MergedWorkspace get activeWorkspaceEntry {
    final active = activeWorkspace;
    return workspaces.firstWhere(
      (w) => w.name == active,
      orElse: () => workspaces.first,
    );
  }

  /// Switch the active workspace (null = Main) and remember it on this device.
  Future<void> selectWorkspace(String? name) async {
    _pickedThisRun = true;
    await _rememberWorkspace(name);
  }

  Future<void> _rememberWorkspace(String? name) async {
    _last = name;
    _invalidateWorkspaces();
    if (!_disposed) notifyListeners();
    try {
      await _prefs?.write(lastWorkspaceKey, name ?? '');
    } catch (_) {
      // Best-effort: failing to remember is not failing to switch.
    }
  }

  /// Per merged workspace (same order), how many sessions are waiting for
  /// input. Counts the agent state alone, as `viewmodel::waiting_counts` does.
  List<WorkspaceWaiting> get waitingCounts {
    final counts = <String?, int>{};
    for (final store in _stores) {
      for (final s in store.sessions) {
        if (store.agentStateFor(s.id) != AgentState.waitingForInput) continue;
        final ws = store.workspaceOfSession(s);
        counts[ws] = (counts[ws] ?? 0) + 1;
      }
    }
    return [
      for (final w in workspaces) WorkspaceWaiting(w, counts[w.name] ?? 0),
    ];
  }

  void _invalidateWorkspaces() {
    _merged = null;
    _activeResolved = false;
  }

  /// The merged user workspaces as the definition list a server stores, in
  /// display order. Includes a tag no server defines yet, which sending it
  /// defines — so an orphaned tag heals the first time anything is edited.
  List<WorkspaceDef> get _definitions => [
    for (final w in workspaces)
      if (w.name case final name?) WorkspaceDef(name: name),
  ];

  /// Run [op] against every server at once. Best effort: each server's failure
  /// is collected rather than thrown, and a server with no live handle counts
  /// as one. Successful servers are refreshed so the edit shows immediately
  /// rather than a poll interval later.
  Future<List<WorkspaceEditFailure>> _fanOut(
    Future<void> Function(CommanderStore store) op,
  ) async {
    final failures = <WorkspaceEditFailure>[];
    await Future.wait([
      for (final store in List.of(_stores))
        () async {
          if (store.handle == null) {
            failures.add(
              WorkspaceEditFailure(store.config.name, 'not connected'),
            );
            return;
          }
          try {
            await op(store);
            await store.refresh();
          } catch (e) {
            failures.add(WorkspaceEditFailure(store.config.name, e));
          }
        }(),
    ]);
    return failures;
  }

  /// Replace every server's definitions with [defs] — each server getting it
  /// narrowed to what it accepts (`definitionsForServer`: its own spellings
  /// kept, another server's case-insensitive clash dropped), so two servers
  /// that disagree never make an edit fail on both.
  Future<List<WorkspaceEditFailure>> _putDefinitions(
    List<WorkspaceDef> defs, {
    WorkspaceDef? main,
    String? startup,
  }) => _fanOut((store) {
    final snap = store.snapshot;
    return store.setWorkspaces(
      SetWorkspacesRequestDto(
        workspaces: _api.definitionsForServer(
          wanted: defs,
          own: snap?.workspaces ?? const [],
          mainLabel: (main ?? snap?.mainWorkspace)?.name,
        ),
        main: main,
        startupWorkspace: startup,
      ),
    );
  });

  /// Define a new workspace on every server, after the existing ones.
  Future<List<WorkspaceEditFailure>> createWorkspace(String name) =>
      _putDefinitions([..._definitions, WorkspaceDef(name: name.trim())]);

  /// Put the user workspaces in [names]' order on every server. Names not
  /// listed keep their relative order after the listed ones.
  Future<List<WorkspaceEditFailure>> reorderWorkspaces(List<String> names) {
    final defs = _definitions;
    final ordered = [
      for (final n in names) ...defs.where((d) => d.name == n),
      ...defs.where((d) => !names.contains(d.name)),
    ];
    return _putDefinitions(ordered);
  }

  /// Relabel the built-in Main workspace (display only — Main has no name).
  Future<List<WorkspaceEditFailure>> renameMainWorkspace(String label) =>
      _putDefinitions(_definitions, main: WorkspaceDef(name: label.trim()));

  /// Set every server's `startup_workspace`: `last`, `main` or a name.
  Future<List<WorkspaceEditFailure>> setStartupWorkspace(String startup) =>
      _putDefinitions(_definitions, startup: startup);

  /// Rename a workspace on every server (each rewrites its own projects' tags).
  /// If it was the active one, the device follows it to the new name — as each
  /// server's refresh lands, never by way of Main ([_renamed]).
  Future<List<WorkspaceEditFailure>> renameWorkspace(
    String from,
    String to,
  ) async {
    final target = to.trim();
    final follow = activeWorkspace == from || _last == from;
    _renaming = (from: from, to: target);
    _invalidateWorkspaces();
    final List<WorkspaceEditFailure> failures;
    try {
      failures = await _fanOut((store) => store.renameWorkspace(from, target));
    } finally {
      _renaming = null;
      _invalidateWorkspaces();
    }
    // Follow only a rename some server took: one they all refused changed
    // nothing, and following it would strand the device on Main.
    final accepted = workspaces.any((w) => w.name == target);
    if (follow && accepted) await _rememberWorkspace(target);
    return failures;
  }

  /// Delete a workspace on every server, which moves its projects to Main. If
  /// it was the active one, the device drops back to Main.
  Future<List<WorkspaceEditFailure>> deleteWorkspace(String name) async {
    final wasActive = activeWorkspace == name;
    final failures = await _fanOut((store) => store.deleteWorkspace(name));
    if (wasActive || _last == name) await _rememberWorkspace(null);
    return failures;
  }

  /// Move one project to [workspace] (null = Main). Only [store] — the server
  /// that owns the project — is written; it defines the workspace for itself if
  /// it has not got it yet.
  Future<List<WorkspaceEditFailure>> moveProject(
    CommanderStore store,
    String projectId,
    String? workspace,
  ) async {
    try {
      await store.setProjectWorkspace(projectId, workspace);
      await store.refresh();
      return const [];
    } catch (e) {
      return [WorkspaceEditFailure(store.config.name, e)];
    }
  }

  // --- lifecycle ----------------------------------------------------------

  /// Load the saved server list and connect every server. Fire-and-forget per
  /// server: each surfaces its own connect progress/errors as state, so a slow
  /// or failing server never blocks the others (it shows degraded in its group).
  Future<void> loadAndConnectAll() async {
    await loadWorkspacePref();
    final configs = await _listStore.load();
    for (final cfg in configs) {
      unawaited(_spinUp(cfg).connect());
    }
    if (!_disposed) notifyListeners();
  }

  /// Add a new server: persist it, spin up its store, and connect. The change
  /// feed will fill in its sessions as they arrive.
  Future<void> addServer(ServerConfig config) async {
    final store = _spinUp(config);
    await _persist();
    if (!_disposed) notifyListeners();
    await store.connect();
  }

  /// Update an existing server in place (URL/token/name edit): apply the config
  /// synchronously, persist, then reconnect its store (releasing the old handle
  /// first). A no-op add if the id isn't known.
  ///
  /// Completes at the *persist* commit point, which is what "saved" means here —
  /// the reconnect runs in the background. It is several network round trips
  /// against a server that may be slow, wedged, or down (30s per request), and
  /// awaiting it left the Edit server form spinning long after the edit had
  /// taken effect. The servers list reports reconnection progress through its
  /// live connection dot instead.
  Future<void> updateServer(ServerConfig config) async {
    final store = serverById(config.id);
    if (store == null) return addServer(config);
    // Apply first so the store carries the new config before we persist — else
    // a concurrent add/remove persist would read the pre-edit config and write
    // it back over the edit.
    store.applyConfig(config);
    await _persist();
    if (!_disposed) notifyListeners();
    unawaited(store.reconnect(config));
  }

  /// Remove a server: drop it from the list, persist, and dispose its store
  /// (which releases the handle and tears down its feeds).
  Future<void> removeServer(String id) async {
    final idx = _stores.indexWhere((s) => s.config.id == id);
    if (idx < 0) return;
    final store = _stores.removeAt(idx);
    store.removeListener(_onChildChanged);
    _invalidateWorkspaces();
    await _persist();
    if (!_disposed) notifyListeners();
    store.dispose();
  }

  /// Refresh every connected server (pull-to-refresh at the fleet level).
  Future<void> refreshAll() async {
    await Future.wait([for (final s in _stores) s.refresh()]);
  }

  CommanderStore _spinUp(ServerConfig config) {
    final store = _storeFactory(config);
    store.addListener(_onChildChanged);
    _stores.add(store);
    _invalidateWorkspaces();
    return store;
  }

  /// Persist the current server list from each store's live config.
  Future<void> _persist() =>
      _listStore.save([for (final s in _stores) s.config]);

  void _onChildChanged() {
    _invalidateWorkspaces();
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    for (final s in _stores) {
      s.removeListener(_onChildChanged);
      s.dispose();
    }
    _stores.clear();
    super.dispose();
  }
}
