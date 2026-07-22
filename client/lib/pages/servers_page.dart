import 'package:flutter/material.dart';

import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/workspace_store.dart';
import 'connection_page.dart';

/// Manage the configured servers: add, edit, or remove. Each row shows a live
/// connection dot. Adding/editing pushes the [ConnectionPage] form, whose
/// `onSubmit` persists + (re)connects through the [WorkspaceStore].
class ServersPage extends StatelessWidget {
  final WorkspaceStore workspace;
  const ServersPage({super.key, required this.workspace});

  Future<void> _add(BuildContext context) => Navigator.of(context).push(
    MaterialPageRoute(
      builder: (_) => ConnectionPage(
        api: workspace.api,
        onSubmit: workspace.addServer,
      ),
    ),
  );

  Future<void> _edit(BuildContext context, CommanderStore store) =>
      Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => ConnectionPage(
            api: store.api,
            existing: store.config,
            onSubmit: workspace.updateServer,
          ),
        ),
      );

  Future<void> _confirmRemove(BuildContext context, CommanderStore store) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Remove ${store.config.name}?'),
        content: const Text(
          'Disconnects and forgets this server on this device. The server '
          'itself and its sessions are untouched.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (ok ?? false) await workspace.removeServer(store.config.id);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Servers')),
      floatingActionButton: FloatingActionButton(
        onPressed: () => _add(context),
        tooltip: 'Add server',
        child: const Icon(Icons.add),
      ),
      body: ListenableBuilder(
        listenable: workspace,
        builder: (context, _) {
          final servers = workspace.servers;
          return ListView(
            children: [
              for (final store in servers)
                ListenableBuilder(
                  listenable: store,
                  builder: (context, _) => ListTile(
                    leading: _dot(context, store.connection),
                    title: Text(store.config.name),
                    subtitle: Text(
                      store.config.baseUrl,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                    onTap: () => _edit(context, store),
                    trailing: IconButton(
                      icon: const Icon(Icons.delete_outline),
                      tooltip: 'Remove',
                      onPressed: () => _confirmRemove(context, store),
                    ),
                  ),
                ),
            ],
          );
        },
      ),
    );
  }

  Widget _dot(BuildContext context, ConnectionStateDto conn) {
    final scheme = Theme.of(context).colorScheme;
    final color = switch (conn.kind) {
      ConnectionStateKind.connected => Colors.green,
      ConnectionStateKind.connecting => scheme.tertiary,
      ConnectionStateKind.degraded => scheme.error,
    };
    return Container(
      width: 12,
      height: 12,
      margin: const EdgeInsets.only(top: 4),
      decoration: BoxDecoration(color: color, shape: BoxShape.circle),
    );
  }
}
