import 'package:flutter/material.dart';

import '../server_config.dart';
import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';
import '../widgets/session_chips.dart';
import 'connection_page.dart';
import 'create_session_page.dart';
import 'session_detail_page.dart';

/// Lists the server's sessions. Pull to refresh; tap a session for its detail
/// view. The app bar links back to connection settings; the FAB creates a
/// session.
class SessionListPage extends StatefulWidget {
  final CommanderApi api;

  /// The config store, threaded through so the app-bar settings route can
  /// re-open the connection page with the same (possibly in-memory) store.
  final ServerConfigStore store;
  final ServerConfig config;
  const SessionListPage({
    super.key,
    required this.api,
    required this.store,
    required this.config,
  });

  @override
  State<SessionListPage> createState() => _SessionListPageState();
}

class _SessionListPageState extends State<SessionListPage> {
  late Future<List<SessionInfo>> _future;

  @override
  void initState() {
    super.initState();
    _future = _load();
  }

  Future<List<SessionInfo>> _load() => widget.api.listSessions(
    baseUrl: widget.config.baseUrl,
    token: widget.config.token,
    includeStopped: true,
  );

  Future<void> _refresh() async {
    final next = _load();
    // Block body (not `=> _future = next`): the arrow form returns the
    // assignment's value — a Future — which trips setState's "callback returned
    // a Future" assertion.
    setState(() {
      _future = next;
    });
    await next.catchError((_) => <SessionInfo>[]);
  }

  void _openSettings() {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => ConnectionPage(
          api: widget.api,
          store: widget.store,
          existing: widget.config,
        ),
      ),
    );
  }

  Future<void> _openDetail(SessionInfo session) async {
    await Navigator.of(context).push<bool>(
      MaterialPageRoute(
        builder: (_) => SessionDetailPage(
          api: widget.api,
          config: widget.config,
          session: session,
        ),
      ),
    );
    // A lifecycle action (delete/kill/restart) may have changed the list.
    await _refresh();
  }

  Future<void> _createSession() async {
    final id = await Navigator.of(context).push<String>(
      MaterialPageRoute(
        builder: (_) =>
            CreateSessionPage(api: widget.api, config: widget.config),
      ),
    );
    if (id != null) await _refresh();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Sessions'),
        actions: [
          IconButton(
            onPressed: _refresh,
            icon: const Icon(Icons.refresh),
            tooltip: 'Refresh',
          ),
          IconButton(
            onPressed: _openSettings,
            icon: const Icon(Icons.settings),
            tooltip: 'Server settings',
          ),
        ],
      ),
      floatingActionButton: FloatingActionButton(
        onPressed: _createSession,
        tooltip: 'New session',
        child: const Icon(Icons.add),
      ),
      body: RefreshIndicator(
        onRefresh: _refresh,
        child: FutureBuilder<List<SessionInfo>>(
          future: _future,
          builder: (context, snapshot) {
            if (snapshot.connectionState == ConnectionState.waiting) {
              return const Center(child: CircularProgressIndicator());
            }
            if (snapshot.hasError) {
              return _ErrorView(
                error: snapshot.error.toString(),
                onRetry: _refresh,
              );
            }
            final sessions = snapshot.data ?? const [];
            if (sessions.isEmpty) {
              return _emptyState();
            }
            return ListView.builder(
              itemCount: sessions.length,
              itemBuilder: (_, i) => _SessionTile(
                session: sessions[i],
                onTap: () => _openDetail(sessions[i]),
              ),
            );
          },
        ),
      ),
    );
  }

  Widget _emptyState() {
    // ListView so pull-to-refresh still works when empty.
    return ListView(
      children: const [
        SizedBox(height: 120),
        Center(child: Icon(Icons.inbox_outlined, size: 48)),
        SizedBox(height: 12),
        Center(child: Text('No sessions')),
      ],
    );
  }
}

class _SessionTile extends StatelessWidget {
  final SessionInfo session;
  final VoidCallback onTap;
  const _SessionTile({required this.session, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
      child: ListTile(
        onTap: onTap,
        title: Text(
          session.title,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '${session.projectName} · ${session.branch}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.bodySmall,
            ),
            const SizedBox(height: 6),
            Wrap(
              spacing: 6,
              runSpacing: 4,
              children: [
                statusChip(context, session.status),
                if (session.prNumber != null)
                  prChip(context, session.prNumber!, session.prState),
              ],
            ),
          ],
        ),
        trailing: Text(
          session.program,
          style: Theme.of(context).textTheme.labelSmall,
        ),
      ),
    );
  }
}

class _ErrorView extends StatelessWidget {
  final String error;
  final Future<void> Function() onRetry;
  const _ErrorView({required this.error, required this.onRetry});

  @override
  Widget build(BuildContext context) {
    return ListView(
      children: [
        const SizedBox(height: 80),
        Icon(
          Icons.cloud_off,
          size: 48,
          color: Theme.of(context).colorScheme.error,
        ),
        const SizedBox(height: 12),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24),
          child: Text(error, textAlign: TextAlign.center),
        ),
        const SizedBox(height: 16),
        Center(
          child: FilledButton.icon(
            onPressed: onRetry,
            icon: const Icon(Icons.refresh),
            label: const Text('Retry'),
          ),
        ),
      ],
    );
  }
}
