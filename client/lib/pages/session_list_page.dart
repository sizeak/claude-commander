import 'package:flutter/material.dart';

import '../server_config.dart';
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/simple.dart' as rust;
import 'connection_page.dart';

/// Lists the server's sessions. Pull to refresh; tapping a session is wired in
/// Phase 2 (detail). The app bar links back to connection settings.
class SessionListPage extends StatefulWidget {
  final ServerConfig config;
  const SessionListPage({super.key, required this.config});

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

  Future<List<SessionInfo>> _load() => rust.listSessions(
    baseUrl: widget.config.baseUrl,
    token: widget.config.token,
    includeStopped: true,
  );

  Future<void> _refresh() async {
    final next = _load();
    setState(() => _future = next);
    await next.catchError((_) => <SessionInfo>[]);
  }

  void _openSettings() {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => ConnectionPage(existing: widget.config),
      ),
    );
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
              itemBuilder: (_, i) => _SessionTile(session: sessions[i]),
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
  const _SessionTile({required this.session});

  @override
  Widget build(BuildContext context) {
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
      child: ListTile(
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
                _statusChip(context, session.status),
                if (session.prNumber != null)
                  _prChip(context, session.prNumber!, session.prState),
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

  Widget _statusChip(BuildContext context, SessionStatus status) {
    final (label, color) = switch (status) {
      SessionStatus.creating => ('creating', Colors.blue),
      SessionStatus.running => ('running', Colors.green),
      SessionStatus.stopped => ('stopped', Colors.grey),
      SessionStatus.merging => ('merging', Colors.orange),
      SessionStatus.cascadePaused => ('cascade paused', Colors.deepOrange),
      SessionStatus.pushing => ('pushing', Colors.teal),
    };
    return _chip(label, color);
  }

  Widget _prChip(BuildContext context, int number, PrState state) {
    final color = switch (state) {
      PrState.open => Colors.green,
      PrState.closed => Colors.red,
      PrState.merged => Colors.purple,
    };
    return _chip('PR #$number ${state.name}', color);
  }

  Widget _chip(String label, Color color) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.18),
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: color.withValues(alpha: 0.5)),
      ),
      child: Text(label, style: TextStyle(color: color, fontSize: 12)),
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
