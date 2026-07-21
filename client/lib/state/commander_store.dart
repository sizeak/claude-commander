import 'dart:async';

import 'package:flutter/foundation.dart';

import '../server_config.dart';
import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/simple.dart' show ScanResultDto;

/// One project paired with the sessions that belong to it, in the workspace's
/// project order. Used by the grouped session view.
class ProjectSessions {
  final ProjectInfoDto project;
  final List<SessionInfo> sessions;
  const ProjectSessions({required this.project, required this.sessions});
}

/// The single reactive source of truth for a connected server. It owns the
/// opaque server [handle] for its whole lifetime — [connect] acquires it,
/// [reconnect] swaps it (tearing down the old one first), and [dispose] releases
/// it — so a handle can never be abandoned in the cdylib registry.
///
/// State is refreshed off the poller's change feed rather than a wall-clock
/// timer: every generation bump re-fetches the workspace snapshot and agent
/// states (mirroring the TUI), and every connection-feed event updates
/// [connection]. Widgets listen via `ListenableBuilder`.
class CommanderStore extends ChangeNotifier {
  CommanderStore({required CommanderApi api, required ServerConfig config})
    : _api = api,
      _config = config;

  final CommanderApi _api;
  ServerConfig _config;

  /// The seam, exposed so pages can construct sub-flows (terminal/review/create)
  /// that still take a raw [CommanderApi] + [handle].
  CommanderApi get api => _api;
  ServerConfig get config => _config;

  String? _handle;

  /// The live server handle, or null before the first [connect] completes / while
  /// a [reconnect] is in flight.
  String? get handle => _handle;

  WorkspaceSnapshotDto? _workspace;
  WorkspaceSnapshotDto? get workspace => _workspace;

  final Map<String, AgentState> _agentStates = {};
  bool _commanderRunning = false;
  bool get commanderRunning => _commanderRunning;

  ConnectionStateDto _connection = const ConnectionStateDto(
    kind: ConnectionStateKind.connecting,
    reason: '',
  );
  ConnectionStateDto get connection => _connection;

  Object? _error;
  Object? get error => _error;

  bool _loading = false;
  bool get loading => _loading;

  StreamSubscription<BigInt>? _changeSub;
  StreamSubscription<ConnectionStateDto>? _connectionSub;

  /// The id of the terminal attach the UI currently holds open, if any. The
  /// terminal body registers it via [setActiveTerminalAttach] on attach and
  /// clears it on detach, so [reconnect] / [dispose] can tear it down before
  /// releasing the handle — belt-and-braces with the cdylib's own
  /// disconnect-time teardown (`disconnect_server` also aborts in-flight
  /// attaches for the handle).
  String? _activeTerminalAttachId;

  // Coalescing guards so overlapping change-feed ticks don't stack refetches: a
  // tick arriving mid-fetch sets [_refreshQueued] and one more fetch runs after.
  bool _refreshing = false;
  bool _refreshQueued = false;
  bool _disposed = false;

  // --- convenience getters the pages render from ---------------------------

  List<SessionInfo> get sessions => _workspace?.sessions ?? const [];

  /// Sessions grouped under their project, in the workspace's project order.
  List<ProjectSessions> get sessionsByProject {
    final ws = _workspace;
    if (ws == null) return const [];
    final byProject = <ProjectId, List<SessionInfo>>{};
    for (final s in ws.sessions) {
      byProject.putIfAbsent(s.projectId, () => []).add(s);
    }
    return [
      for (final p in ws.projects)
        ProjectSessions(project: p, sessions: byProject[p.id] ?? const []),
    ];
  }

  List<OperationStatusDto> get operations =>
      _workspace?.operations ?? const [];

  List<SessionId> get pendingCommentSessions =>
      _workspace?.pendingCommentSessions ?? const [];

  /// The session whose cascade is currently paused awaiting a decision, or null
  /// when no cascade is paused. Drives the resume/abandon banner.
  SessionId? get cascadePaused => _workspace?.cascadePaused;

  /// The projects known to the server, in workspace order.
  List<ProjectInfoDto> get projects => _workspace?.projects ?? const [];

  /// The agent state for a session id (the [SessionInfo.id] string form), or
  /// [AgentState.unknown] if the snapshot has no entry for it.
  AgentState agentStateFor(String id) => _agentStates[id] ?? AgentState.unknown;

  /// The live [SessionInfo] for an id from the latest snapshot, or null if the
  /// session is no longer present (deleted/stopped-and-removed).
  SessionInfo? sessionById(String id) {
    for (final s in sessions) {
      if (s.id == id) return s;
    }
    return null;
  }

  // --- lifecycle ----------------------------------------------------------

  /// Acquire the handle, wire up the feeds, and load the first snapshot.
  Future<void> connect() async {
    _loading = true;
    _error = null;
    if (!_disposed) notifyListeners();
    try {
      final h = await _api.connectServer(
        baseUrl: _config.baseUrl,
        token: _config.token,
      );
      // The store may have been disposed (server removed) while connectServer
      // was in flight. Release the freshly-acquired handle and bail rather than
      // subscribing feeds that would never be torn down.
      if (_disposed) {
        unawaited(_api.disconnectServer(handle: h));
        return;
      }
      _handle = h;
      _changeSub = _api
          .changeFeed(handle: h)
          .listen(_onChange, onError: (_) {});
      _connectionSub = _api
          .connectionFeed(handle: h)
          .listen(_onConnection, onError: (_) {});
      await _refresh();
    } catch (e) {
      _error = e;
    } finally {
      _loading = false;
      if (!_disposed) notifyListeners();
    }
  }

  /// Swap to a new server: tear down the old feeds, release the old handle
  /// (BEFORE opening the new one, so the cdylib registry never grows), then
  /// connect afresh. This is the fix for the reconnect leak — the store owns the
  /// handle so a settings change can't abandon it.
  Future<void> reconnect(ServerConfig next) async {
    await _teardownSubs();
    final old = _handle;
    _handle = null;
    if (old != null) {
      // Detach any open terminal before releasing the handle so the persistent
      // pane doesn't outlive its server.
      await _detachActiveTerminal();
      try {
        await _api.disconnectServer(handle: old);
      } catch (_) {
        // Best-effort: a failed disconnect must not block the new connection.
      }
    }
    _config = next;
    _workspace = null;
    _agentStates.clear();
    _commanderRunning = false;
    _connection = const ConnectionStateDto(
      kind: ConnectionStateKind.connecting,
      reason: '',
    );
    if (!_disposed) notifyListeners();
    await connect();
  }

  /// Update the stored config (name/URL/token) synchronously, ahead of a
  /// [reconnect]. Lets the workspace persist the edited config immediately —
  /// `reconnect` only assigns `_config` after several awaits, so a concurrent
  /// save would otherwise write the pre-edit config back to disk.
  void applyConfig(ServerConfig config) {
    _config = config;
    if (!_disposed) notifyListeners();
  }

  /// Force a snapshot refetch (pull-to-refresh); a no-op reconnect if the handle
  /// was lost.
  Future<void> refresh() => _refresh();

  /// Retry after a connect/refresh failure: re-establish the handle if it was
  /// never acquired, else just refetch.
  Future<void> retry() => _handle == null ? connect() : _refresh();

  // --- mutations (thin wrappers; the next change-feed tick refreshes state) --

  Future<void> killSession(String id) =>
      _api.killSession(handle: _requireHandle, id: id);

  Future<void> restartSession(String id) =>
      _api.restartSession(handle: _requireHandle, id: id);

  Future<void> deleteSession(String id) =>
      _api.deleteSession(handle: _requireHandle, id: id);

  Future<void> renameSession(String id, String title) =>
      _api.renameSession(handle: _requireHandle, id: id, title: title);

  Future<void> setSection(String id, String? section) =>
      _api.setSection(handle: _requireHandle, id: id, section: section);

  Future<void> markRead(String id) =>
      _api.markRead(handle: _requireHandle, id: id);

  Future<void> markUnread(List<String> ids) =>
      _api.markUnread(handle: _requireHandle, ids: ids);

  Future<bool> toggleKeepAlive(String id) =>
      _api.toggleKeepAlive(handle: _requireHandle, id: id);

  /// Cascade-merge this session's stack. Returns the terminal operation status
  /// (succeeded / paused / failed) for the caller to surface.
  Future<OperationStatusDto> cascadeMerge(String id) =>
      _api.cascadeMerge(handle: _requireHandle, id: id);

  /// Push this session's stack. Returns the terminal operation status.
  Future<OperationStatusDto> pushStack(String id) =>
      _api.pushStack(handle: _requireHandle, id: id);

  /// Resume a paused cascade. Returns the next terminal operation status.
  Future<OperationStatusDto> cascadeResume() =>
      _api.cascadeResume(handle: _requireHandle);

  /// Abandon a paused cascade, leaving the stack where it stopped.
  Future<void> cascadeAbandon() =>
      _api.cascadeAbandon(handle: _requireHandle);

  /// Register a new project by its server-side repo path; returns its new id.
  Future<String> addProject(String path) =>
      _api.addProject(handle: _requireHandle, path: path);

  /// Deregister a project by id (does not touch the repo on disk).
  Future<void> removeProject(String id) =>
      _api.removeProject(handle: _requireHandle, id: id);

  /// Scan a server-side directory for git repos and register any it finds.
  Future<ScanResultDto> scanDirectory(String path) =>
      _api.scanDirectory(handle: _requireHandle, path: path);

  /// List a project's branches (local, plus remotes when [fetch] is set).
  Future<List<BranchInfo>> listBranches(String projectId, {bool fetch = false}) =>
      _api.listBranches(
        handle: _requireHandle,
        projectId: projectId,
        fetch: fetch,
      );

  /// Fetch a single session's detail (pane snapshot / diff stat) — data the
  /// snapshot doesn't carry, so the detail page fetches it on demand.
  Future<SessionDetail?> sessionDetail(String id, {int? lines}) =>
      _api.getSessionDetail(handle: _requireHandle, query: id, lines: lines);

  /// Register (or clear, with null) the terminal attach the UI currently holds
  /// open, so [reconnect] / [dispose] can detach it before releasing the handle.
  void setActiveTerminalAttach(String? attachId) =>
      _activeTerminalAttachId = attachId;

  /// Clear the registered attach only if it's still [attachId]. A disposing
  /// terminal body calls this instead of `setActiveTerminalAttach(null)`, because
  /// when the wide detail pane switches agent↔shell the INCOMING body's
  /// `initState` registers its new attach BEFORE the outgoing body's `dispose`
  /// runs — an unconditional clear would then null out the live new attach.
  void clearActiveTerminalAttach(String attachId) {
    if (_activeTerminalAttachId == attachId) _activeTerminalAttachId = null;
  }

  /// Detach the currently registered terminal attach (if any) and forget it.
  /// Best-effort: the cdylib's disconnect teardown is the primary path.
  Future<void> _detachActiveTerminal() async {
    final attach = _activeTerminalAttachId;
    _activeTerminalAttachId = null;
    if (attach == null) return;
    try {
      await _api.terminalDetach(attachId: attach);
    } catch (_) {
      // Ignore: disconnect_server also aborts the attach server-side.
    }
  }

  // --- internals ----------------------------------------------------------

  String get _requireHandle =>
      _handle ?? (throw StateError('CommanderStore is not connected'));

  void _onChange(BigInt _) => unawaited(_refresh());

  void _onConnection(ConnectionStateDto state) {
    _connection = state;
    if (!_disposed) notifyListeners();
  }

  Future<void> _refresh() async {
    final h = _handle;
    if (h == null) return;
    if (_refreshing) {
      _refreshQueued = true;
      return;
    }
    _refreshing = true;
    try {
      final ws = await _api.workspaceSnapshot(handle: h);
      final states = await _api.agentStates(handle: h, fresh: false);
      _workspace = ws;
      _agentStates
        ..clear()
        ..addEntries(
          states.states.map((e) => MapEntry(e.sessionId.field0.uuid, e.state)),
        );
      _commanderRunning = states.commanderRunning;
      _error = null;
    } catch (e) {
      _error = e;
    } finally {
      _refreshing = false;
      if (!_disposed) notifyListeners();
      if (_refreshQueued && !_disposed) {
        _refreshQueued = false;
        await _refresh();
      }
    }
  }

  Future<void> _teardownSubs() async {
    await _changeSub?.cancel();
    await _connectionSub?.cancel();
    _changeSub = null;
    _connectionSub = null;
  }

  @override
  void dispose() {
    _disposed = true;
    // cancel() returns a Future; nothing awaits it during teardown.
    unawaited(_teardownSubs());
    final h = _handle;
    if (h != null) {
      unawaited(_detachActiveTerminal());
      unawaited(_api.disconnectServer(handle: h));
    }
    super.dispose();
  }
}
