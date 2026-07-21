import 'package:flutter/material.dart';

import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../state/workspace_store.dart';
import '../widgets/session_chips.dart';
import 'create_session_page.dart';
import 'programs_page.dart';
import 'projects_page.dart';
import 'servers_page.dart';
import 'session_detail_page.dart';

/// The aggregated session list, grouped by server → project — layout-agnostic
/// (no Scaffold, no route). Enumerates the servers from the [WorkspaceStore] and
/// renders each server's grouped sessions under a header (suppressed when only
/// one server is configured, so single-server trees look unchanged). Each server
/// section re-provides its own [CommanderStoreScope] so per-server consumers
/// (detail, cascade banner) keep their single-store contract.
class SessionListBody extends StatelessWidget {
  /// The id of the session shown in the detail pane, highlighted in the list.
  /// Null in the narrow (push) flow, where there is no persistent selection.
  final String? selectedId;

  /// Invoked when a session row is tapped, with the server that owns it.
  final void Function(CommanderStore store, SessionInfo session) onSelect;

  const SessionListBody({super.key, this.selectedId, required this.onSelect});

  @override
  Widget build(BuildContext context) {
    final workspace = WorkspaceScope.of(context)!;
    return ListenableBuilder(
      listenable: workspace,
      builder: (context, _) {
        final servers = workspace.servers;
        final multi = servers.length > 1;
        return RefreshIndicator(
          onRefresh: workspace.refreshAll,
          child: ListView(
            children: [
              for (final store in servers)
                _ServerSection(
                  store: store,
                  showHeader: multi,
                  selectedId: selectedId,
                  onSelect: onSelect,
                ),
            ],
          ),
        );
      },
    );
  }
}

/// One server's slice of the aggregated list: an optional header, a paused-
/// cascade banner, and its project-grouped session tiles — all under that
/// server's [CommanderStoreScope] so the banner and pushed routes resolve to it.
class _ServerSection extends StatelessWidget {
  final CommanderStore store;
  final bool showHeader;
  final String? selectedId;
  final void Function(CommanderStore store, SessionInfo session) onSelect;

  const _ServerSection({
    required this.store,
    required this.showHeader,
    required this.selectedId,
    required this.onSelect,
  });

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => CommanderStoreScope(
        store: store,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          mainAxisSize: MainAxisSize.min,
          children: [
            if (showHeader) _ServerHeader(store: store),
            ..._content(context),
          ],
        ),
      ),
    );
  }

  List<Widget> _content(BuildContext context) {
    if (store.workspace == null) {
      // This server hasn't loaded yet (or failed) — show a compact per-server
      // state so a slow/down server never blanks the whole list.
      if (store.error != null) {
        return [
          _InlineNote(
            icon: Icons.cloud_off,
            text: store.error.toString(),
            action: ('Retry', store.retry),
            color: Theme.of(context).colorScheme.error,
          ),
        ];
      }
      return const [
        Padding(
          padding: EdgeInsets.symmetric(vertical: 24),
          child: Center(child: CircularProgressIndicator()),
        ),
      ];
    }
    final groups = [
      for (final g in store.sessionsByProject)
        if (g.sessions.isNotEmpty) g,
    ];
    return [
      if (store.cascadePaused != null) const CascadeBanner(),
      if (groups.isEmpty)
        const _InlineNote(icon: Icons.inbox_outlined, text: 'No sessions')
      else
        for (final group in groups) ...[
          _ProjectHeader(name: group.project.name),
          for (final session in group.sessions)
            _SessionTile(
              session: session,
              selected: session.id == selectedId,
              onTap: () => onSelect(store, session),
            ),
        ],
    ];
  }
}

/// The narrow (phone) session list: an app bar with a refresh + settings menu, a
/// create FAB, and a [SessionListBody] whose taps push the detail route (wrapped
/// in the owning server's scope so the detail page resolves the right server).
class SessionListPage extends StatelessWidget {
  const SessionListPage({super.key});

  @override
  Widget build(BuildContext context) {
    final workspace = WorkspaceScope.of(context)!;
    return ListenableBuilder(
      listenable: workspace,
      builder: (context, _) {
        final servers = workspace.servers;
        // A lone server shows its connection state in the app bar (no header);
        // with several, each header carries its own dot.
        final soleConnection = servers.length == 1
            ? servers.single.connection
            : null;
        return Scaffold(
          appBar: AppBar(
            title: const Text('Sessions'),
            bottom: soleConnection == null
                ? null
                : connectionBar(context, soleConnection),
            actions: [
              IconButton(
                onPressed: workspace.refreshAll,
                icon: const Icon(Icons.refresh),
                tooltip: 'Refresh',
              ),
              SettingsMenu(workspace: workspace),
            ],
          ),
          floatingActionButton: FloatingActionButton(
            onPressed: () => openCreateSession(context, workspace),
            tooltip: 'New session',
            child: const Icon(Icons.add),
          ),
          body: SessionListBody(
            onSelect: (store, session) => _openDetail(context, store, session),
          ),
        );
      },
    );
  }

  Future<void> _openDetail(
    BuildContext context,
    CommanderStore store,
    SessionInfo session,
  ) async {
    await Navigator.of(context).push<bool>(
      MaterialPageRoute(
        // Re-provide the owning server's scope so the detail page's
        // markRead/cascade/terminal/review calls hit the right server.
        builder: (_) => CommanderStoreScope(
          store: store,
          child: SessionDetailPage(session: session),
        ),
      ),
    );
    // A lifecycle action bumps the change feed, so the list refreshes itself.
  }
}

/// Resolve the server to act on for a per-server action (create/projects/
/// programs). Returns it directly when there is one server; otherwise prompts.
/// Null means "no server / user cancelled".
Future<CommanderStore?> pickServer(
  BuildContext context,
  WorkspaceStore workspace, {
  String title = 'Choose a server',
}) async {
  final servers = workspace.servers;
  if (servers.isEmpty) return null;
  if (servers.length == 1) return servers.single;
  return showModalBottomSheet<CommanderStore>(
    context: context,
    builder: (context) => SafeArea(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Padding(
            padding: const EdgeInsets.all(16),
            child: Text(title, style: Theme.of(context).textTheme.titleMedium),
          ),
          for (final store in servers)
            ListTile(
              leading: const Icon(Icons.dns_outlined),
              title: Text(store.config.name),
              subtitle: Text(
                store.config.baseUrl,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              onTap: () => Navigator.of(context).pop(store),
            ),
        ],
      ),
    ),
  );
}

/// Push the create-session route for a chosen server. Shared by both layouts.
Future<void> openCreateSession(
  BuildContext context,
  WorkspaceStore workspace,
) async {
  final store = await pickServer(context, workspace, title: 'Create on…');
  if (store == null || store.handle == null || !context.mounted) return;
  await Navigator.of(context).push<String>(
    MaterialPageRoute(
      builder: (_) => CreateSessionPage(api: store.api, handle: store.handle!),
    ),
  );
}

/// Push the servers manager (add/edit/remove).
void openServers(BuildContext context, WorkspaceStore workspace) {
  Navigator.of(context).push(
    MaterialPageRoute(builder: (_) => ServersPage(workspace: workspace)),
  );
}

/// Push the program-list editor for a chosen server (`PUT /api/config/programs`).
Future<void> openPrograms(
  BuildContext context,
  WorkspaceStore workspace,
) async {
  final store = await pickServer(context, workspace, title: 'Programs on…');
  final handle = store?.handle;
  if (store == null || handle == null || !context.mounted) return;
  Navigator.of(context).push(
    MaterialPageRoute(builder: (_) => ProgramsPage(api: store.api, handle: handle)),
  );
}

/// Push the projects manager for a chosen server (add/remove/scan + branches).
Future<void> openProjects(
  BuildContext context,
  WorkspaceStore workspace,
) async {
  final store = await pickServer(context, workspace, title: 'Projects on…');
  if (store == null || store.handle == null || !context.mounted) return;
  Navigator.of(context).push(
    MaterialPageRoute(builder: (_) => ProjectsPage(store: store)),
  );
}

/// True when at least one server has a live handle (per-server actions need one).
bool _anyConnected(WorkspaceStore workspace) =>
    workspace.servers.any((s) => s.handle != null);

/// The app-bar overflow menu shared by both shells: manage servers, plus the
/// per-server projects/programs editors (which prompt for a server when more
/// than one is configured).
class SettingsMenu extends StatelessWidget {
  final WorkspaceStore workspace;

  const SettingsMenu({super.key, required this.workspace});

  @override
  Widget build(BuildContext context) {
    final enabled = _anyConnected(workspace);
    return PopupMenuButton<String>(
      icon: const Icon(Icons.settings),
      tooltip: 'Settings',
      onSelected: (value) {
        switch (value) {
          case 'servers':
            openServers(context, workspace);
          case 'projects':
            openProjects(context, workspace);
          case 'programs':
            openPrograms(context, workspace);
        }
      },
      itemBuilder: (context) => [
        const PopupMenuItem(value: 'servers', child: Text('Servers')),
        PopupMenuItem(
          value: 'projects',
          enabled: enabled,
          child: const Text('Projects'),
        ),
        PopupMenuItem(
          value: 'programs',
          enabled: enabled,
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
/// a double-tap can't fire twice. Reads the server from the enclosing scope, so
/// it acts on the server whose group it is rendered in.
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

/// A server-group header: the server name plus a live connection dot. A degraded
/// server is greyed + dimmed with its failure reason shown (mirrors the TUI), so
/// a down server reads as inert but never vanishes from the list.
class _ServerHeader extends StatelessWidget {
  final CommanderStore store;
  const _ServerHeader({required this.store});

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final conn = store.connection;
    final (color, note, degraded) = switch (conn.kind) {
      ConnectionStateKind.connected => (Colors.green, null, false),
      ConnectionStateKind.connecting => (
        scheme.tertiary,
        'connecting…',
        false,
      ),
      ConnectionStateKind.degraded => (
        scheme.error,
        conn.reason.isEmpty ? 'degraded' : conn.reason,
        true,
      ),
    };
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 16, 12, 4),
      child: Row(
        children: [
          Container(
            width: 10,
            height: 10,
            decoration: BoxDecoration(color: color, shape: BoxShape.circle),
          ),
          const SizedBox(width: 8),
          Flexible(
            child: Text(
              store.config.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.titleSmall?.copyWith(
                fontWeight: FontWeight.w700,
                color: degraded ? scheme.onSurfaceVariant : scheme.onSurface,
              ),
            ),
          ),
          if (note != null) ...[
            const SizedBox(width: 8),
            Flexible(
              child: Text(
                note,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: Theme.of(context).textTheme.labelSmall?.copyWith(color: color),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// A compact inline note (loading-failed / empty) rendered inside a server
/// section, with an optional action button.
class _InlineNote extends StatelessWidget {
  final IconData icon;
  final String text;
  final (String, Future<void> Function())? action;
  final Color? color;
  const _InlineNote({
    required this.icon,
    required this.text,
    this.action,
    this.color,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 16),
      child: Column(
        children: [
          Icon(icon, color: color ?? Theme.of(context).colorScheme.outline),
          const SizedBox(height: 8),
          Text(text, textAlign: TextAlign.center),
          if (action != null) ...[
            const SizedBox(height: 8),
            FilledButton.icon(
              onPressed: action!.$2,
              icon: const Icon(Icons.refresh),
              label: Text(action!.$1),
            ),
          ],
        ],
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
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
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
