import 'package:flutter/material.dart';

import '../server_config.dart';
import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../widgets/session_chips.dart';
import 'connection_page.dart';
import 'create_session_page.dart';
import 'programs_page.dart';
import 'projects_page.dart';
import 'session_detail_page.dart';

/// The session list content, grouped by project — layout-agnostic (no Scaffold,
/// no route). Rendered reactively from the [CommanderStore]: the poller's change
/// feed refreshes it, so there's no local timer or FutureBuilder. The narrow
/// [SessionListPage] wraps this in a Scaffold and pushes a detail route on tap;
/// the wide shell places it in the master column and updates a detail pane in
/// place (so it passes [selectedId] to highlight the open session).
class SessionListBody extends StatelessWidget {
  /// The id of the session shown in the detail pane, highlighted in the list.
  /// Null in the narrow (push) flow, where there is no persistent selection.
  final String? selectedId;

  /// Invoked when a session row is tapped.
  final ValueChanged<SessionInfo> onSelect;

  const SessionListBody({
    super.key,
    this.selectedId,
    required this.onSelect,
  });

  @override
  Widget build(BuildContext context) {
    final store = CommanderStoreScope.of(context)!;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => RefreshIndicator(
        onRefresh: store.refresh,
        child: _content(context, store),
      ),
    );
  }

  Widget _content(BuildContext context, CommanderStore store) {
    if (store.workspace == null) {
      if (store.error != null) {
        return _ErrorView(error: store.error.toString(), onRetry: store.retry);
      }
      return const Center(child: CircularProgressIndicator());
    }
    // A paused cascade blocks the whole workspace, so surface it at the top of
    // the list (shared by both layouts, which both render this body).
    final banner = store.cascadePaused == null
        ? null
        : const CascadeBanner();
    // Group by project (workspace order); drop projects with no sessions.
    final groups = [
      for (final g in store.sessionsByProject)
        if (g.sessions.isNotEmpty) g,
    ];
    if (groups.isEmpty) return _emptyState(banner);
    return ListView(
      children: [
        ?banner,
        for (final group in groups) ...[
          _ProjectHeader(name: group.project.name),
          for (final session in group.sessions)
            _SessionTile(
              session: session,
              selected: session.id == selectedId,
              onTap: () => onSelect(session),
            ),
        ],
      ],
    );
  }

  Widget _emptyState([Widget? banner]) {
    // ListView so pull-to-refresh still works when empty.
    return ListView(
      children: [
        ?banner,
        const SizedBox(height: 120),
        const Center(child: Icon(Icons.inbox_outlined, size: 48)),
        const SizedBox(height: 12),
        const Center(child: Text('No sessions')),
      ],
    );
  }
}

/// Lists the server's sessions in the phone (stacked-navigation) layout: an app
/// bar with the live connection state and a settings link, a create FAB, and a
/// [SessionListBody] whose taps push the detail route.
class SessionListPage extends StatelessWidget {
  /// The config store, threaded through so the settings route can re-open the
  /// connection page with the same (possibly in-memory) store.
  final ServerConfigStore configStore;

  /// Handed to the settings connection page so a reconnect goes through the app
  /// (which reconnects the shared store rather than minting a new handle).
  final Future<void> Function(ServerConfig config) onConnected;

  const SessionListPage({
    super.key,
    required this.configStore,
    required this.onConnected,
  });

  @override
  Widget build(BuildContext context) {
    final store = CommanderStoreScope.of(context)!;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        return Scaffold(
          appBar: AppBar(
            title: const Text('Sessions'),
            bottom: connectionBar(context, store.connection),
            actions: [
              IconButton(
                onPressed: store.refresh,
                icon: const Icon(Icons.refresh),
                tooltip: 'Refresh',
              ),
              SettingsMenu(
                store: store,
                configStore: configStore,
                onConnected: onConnected,
              ),
            ],
          ),
          floatingActionButton: store.handle == null
              ? null
              : FloatingActionButton(
                  onPressed: () => openCreateSession(context, store),
                  tooltip: 'New session',
                  child: const Icon(Icons.add),
                ),
          body: SessionListBody(
            onSelect: (session) => _openDetail(context, session),
          ),
        );
      },
    );
  }

  Future<void> _openDetail(BuildContext context, SessionInfo session) async {
    await Navigator.of(context).push<bool>(
      MaterialPageRoute(
        builder: (_) => SessionDetailPage(session: session),
      ),
    );
    // A lifecycle action bumps the change feed, so the list refreshes itself.
  }
}

/// Push the create-session route. Shared by the narrow app bar and the wide
/// shell's create action. The new session arrives via the change feed.
Future<void> openCreateSession(
  BuildContext context,
  CommanderStore store,
) async {
  await Navigator.of(context).push<String>(
    MaterialPageRoute(
      builder: (_) => CreateSessionPage(api: store.api, handle: store.handle!),
    ),
  );
}

/// Push the server-settings connection page. Shared by both layouts.
void openServerSettings(
  BuildContext context,
  CommanderStore store,
  ServerConfigStore configStore,
  Future<void> Function(ServerConfig config) onConnected,
) {
  Navigator.of(context).push(
    MaterialPageRoute(
      builder: (_) => ConnectionPage(
        api: store.api,
        store: configStore,
        existing: store.config,
        onConnected: onConnected,
      ),
    ),
  );
}

/// Push the program-list editor (`PUT /api/config/programs`). Shared by both
/// layouts; disabled while disconnected (no handle).
void openPrograms(BuildContext context, CommanderStore store) {
  final handle = store.handle;
  if (handle == null) return;
  Navigator.of(context).push(
    MaterialPageRoute(
      builder: (_) => ProgramsPage(api: store.api, handle: handle),
    ),
  );
}

/// Push the projects manager (add/remove/scan + branch browsing). Shared by both
/// layouts; disabled while disconnected (no handle).
void openProjects(BuildContext context, CommanderStore store) {
  if (store.handle == null) return;
  Navigator.of(context).push(
    MaterialPageRoute(builder: (_) => ProjectsPage(store: store)),
  );
}

/// The app-bar overflow menu shared by the narrow and wide shells: server
/// settings, the program-list editor, and the projects manager. Kept in one
/// place so both layouts offer the same actions.
class SettingsMenu extends StatelessWidget {
  final CommanderStore store;
  final ServerConfigStore configStore;
  final Future<void> Function(ServerConfig config) onConnected;

  const SettingsMenu({
    super.key,
    required this.store,
    required this.configStore,
    required this.onConnected,
  });

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<String>(
      icon: const Icon(Icons.settings),
      tooltip: 'Settings',
      onSelected: (value) {
        switch (value) {
          case 'server':
            openServerSettings(context, store, configStore, onConnected);
          case 'programs':
            openPrograms(context, store);
          case 'projects':
            openProjects(context, store);
        }
      },
      itemBuilder: (context) => [
        const PopupMenuItem(value: 'server', child: Text('Server settings')),
        PopupMenuItem(
          value: 'projects',
          enabled: store.handle != null,
          child: const Text('Projects'),
        ),
        PopupMenuItem(
          value: 'programs',
          enabled: store.handle != null,
          child: const Text('Programs'),
        ),
      ],
    );
  }
}

/// A thin status bar for the app bar, shown only while connecting or degraded
/// (a healthy connection needs no chrome). Returns null when connected.
PreferredSizeWidget? connectionBar(
  BuildContext context,
  ConnectionStateDto connection,
) {
  final (label, color) = switch (connection.kind) {
    ConnectionStateKind.connected => (null, null),
    ConnectionStateKind.connecting => (
      'Connecting…',
      Theme.of(context).colorScheme.tertiary,
    ),
    ConnectionStateKind.degraded => (
      connection.reason.isEmpty
          ? 'Connection degraded'
          : 'Degraded: ${connection.reason}',
      Theme.of(context).colorScheme.error,
    ),
  };
  if (label == null) return null;
  return PreferredSize(
    preferredSize: const Size.fromHeight(24),
    child: Container(
      width: double.infinity,
      color: color?.withValues(alpha: 0.15),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      child: Text(
        label,
        style: Theme.of(context).textTheme.labelSmall?.copyWith(color: color),
      ),
    ),
  );
}

/// A prominent banner shown while a cascade is paused awaiting a decision. It
/// offers Resume (which continues the cascade and reports the next outcome) and
/// Abandon (which leaves the stack where it stopped). Owns its own busy guard so
/// a double-tap can't fire twice.
class CascadeBanner extends StatefulWidget {
  const CascadeBanner({super.key});

  @override
  State<CascadeBanner> createState() => _CascadeBannerState();
}

class _CascadeBannerState extends State<CascadeBanner> {
  bool _busy = false;

  Future<void> _run(Future<void> Function(CommanderStore store) action) async {
    final store = CommanderStoreScope.of(context);
    if (store == null || _busy) return;
    setState(() => _busy = true);
    try {
      await action(store);
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('Failed: $e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _resume() => _run((store) async {
    final status = await store.cascadeResume();
    if (!mounted) return;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(describeOperation(status))));
  });

  Future<void> _abandon() => _run((store) async {
    await store.cascadeAbandon();
    if (!mounted) return;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(const SnackBar(content: Text('Cascade abandoned')));
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Card(
      margin: const EdgeInsets.fromLTRB(12, 12, 12, 4),
      color: scheme.tertiaryContainer,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.pause_circle_outline, color: scheme.onTertiaryContainer),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    'Cascade paused — awaiting a decision',
                    style: Theme.of(context).textTheme.titleSmall?.copyWith(
                      color: scheme.onTertiaryContainer,
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              children: [
                FilledButton.icon(
                  onPressed: _busy ? null : _resume,
                  icon: const Icon(Icons.play_arrow),
                  label: const Text('Resume'),
                ),
                OutlinedButton.icon(
                  onPressed: _busy ? null : _abandon,
                  icon: const Icon(Icons.close),
                  label: const Text('Abandon'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// A subtle section header naming the project a run of session tiles belongs to.
class _ProjectHeader extends StatelessWidget {
  final String name;
  const _ProjectHeader({required this.name});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
      child: Text(
        name,
        style: Theme.of(context).textTheme.labelLarge?.copyWith(
          color: Theme.of(context).colorScheme.primary,
          fontWeight: FontWeight.w600,
        ),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
    );
  }
}

class _SessionTile extends StatelessWidget {
  final SessionInfo session;
  final bool selected;
  final VoidCallback onTap;
  const _SessionTile({
    required this.session,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
      color: selected ? scheme.primaryContainer : null,
      child: ListTile(
        selected: selected,
        onTap: onTap,
        title: Row(
          children: [
            if (session.unread) ...[
              Icon(Icons.circle, size: 10, color: scheme.primary),
              const SizedBox(width: 6),
            ],
            Expanded(
              child: Text(
                session.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ],
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
            // Program lives in the subtitle (a Column that ellipsizes) rather
            // than ListTile.trailing: a long program string in a narrow tile
            // (the 340px desktop master column) made an unconstrained trailing
            // widget throw "Trailing widget consumes the entire tile width".
            Text(
              session.program,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.labelSmall,
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
