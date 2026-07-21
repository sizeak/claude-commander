import 'dart:async';

import 'package:flutter/foundation.dart';

import '../server_config.dart';
import '../services/commander_api.dart';
import 'commander_store.dart';

/// Builds the per-server [CommanderStore] for one [ServerConfig]. Injected so
/// tests can back each server with its own fake — the direct analogue of the
/// TUI's `RemoteBackendFactory`.
typedef CommanderStoreFactory = CommanderStore Function(ServerConfig config);

/// The reactive aggregator over every configured server. It owns one
/// [CommanderStore] per server (each holding its own opaque handle, poller, and
/// change/connection feeds) and re-broadcasts whenever any of them moves, so the
/// aggregated session list rebuilds on any server's update.
///
/// Sessions are rendered grouped by server, so each row already sits under its
/// owning [CommanderStore] — there is no global id namespacing; a widget acts on
/// the store for its group. This mirrors the TUI's `backends: Vec<BackendHandle>`
/// model (minus the local backend — a phone is a pure remote client).
class WorkspaceStore extends ChangeNotifier {
  WorkspaceStore({
    required CommanderApi api,
    required ServerListStore listStore,
    CommanderStoreFactory? storeFactory,
  }) : _api = api,
       _listStore = listStore,
       _storeFactory =
           storeFactory ?? ((cfg) => CommanderStore(api: api, config: cfg));

  /// Test seam: wrap a workspace around already-constructed [stores] without
  /// loading from disk or auto-connecting, so a widget test can drive each
  /// store's lifecycle (connect timing, injected errors) directly.
  @visibleForTesting
  WorkspaceStore.withStores(List<CommanderStore> stores)
    : assert(stores.isNotEmpty, 'withStores needs at least one store'),
      _api = stores.first.api,
      _listStore = InMemoryServerListStore(),
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

  // --- lifecycle ----------------------------------------------------------

  /// Load the saved server list and connect every server. Fire-and-forget per
  /// server: each surfaces its own connect progress/errors as state, so a slow
  /// or failing server never blocks the others (it shows degraded in its group).
  Future<void> loadAndConnectAll() async {
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
  Future<void> updateServer(ServerConfig config) async {
    final store = serverById(config.id);
    if (store == null) return addServer(config);
    // Apply first so the store carries the new config before we persist — else
    // a concurrent add/remove persist would read the pre-edit config and write
    // it back over the edit.
    store.applyConfig(config);
    await _persist();
    if (!_disposed) notifyListeners();
    await store.reconnect(config);
  }

  /// Remove a server: drop it from the list, persist, and dispose its store
  /// (which releases the handle and tears down its feeds).
  Future<void> removeServer(String id) async {
    final idx = _stores.indexWhere((s) => s.config.id == id);
    if (idx < 0) return;
    final store = _stores.removeAt(idx);
    store.removeListener(_onChildChanged);
    await _persist();
    if (!_disposed) notifyListeners();
    store.dispose();
  }

  /// Refresh every connected server (pull-to-refresh at the workspace level).
  Future<void> refreshAll() async {
    await Future.wait([for (final s in _stores) s.refresh()]);
  }

  CommanderStore _spinUp(ServerConfig config) {
    final store = _storeFactory(config);
    store.addListener(_onChildChanged);
    _stores.add(store);
    return store;
  }

  /// Persist the current server list from each store's live config.
  Future<void> _persist() =>
      _listStore.save([for (final s in _stores) s.config]);

  void _onChildChanged() {
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
