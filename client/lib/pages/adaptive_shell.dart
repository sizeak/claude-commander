import 'package:flutter/material.dart';

import '../server_config.dart';
import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import 'review_page.dart';
import 'session_detail_page.dart';
import 'session_list_page.dart';
import 'terminal_page.dart';

/// Logical width at or above which the app switches from the stacked phone
/// layout to the desktop/tablet master-detail layout.
const double kWideBreakpoint = 900;

/// The responsive home. Below [kWideBreakpoint] it is the phone
/// [SessionListPage] (a stacked `Navigator.push` flow: list → detail →
/// terminal/review). At or above it, a master-detail desktop layout: a server
/// sidebar + session list on the left, and a persistent detail pane on the right
/// whose detail / terminal / review views are switched in place.
///
/// The same page *bodies* ([SessionListBody], [SessionDetailBody], [TerminalBody],
/// [ReviewBody]) serve both layouts; only the surrounding shell differs.
class AdaptiveShell extends StatefulWidget {
  /// The config store, threaded through so the settings route can re-open the
  /// connection page with the same (possibly in-memory) store.
  final ServerConfigStore configStore;

  /// Handed to the settings connection page so a reconnect goes through the app
  /// (which reconnects the shared store rather than minting a new handle).
  final Future<void> Function(ServerConfig config) onConnected;

  const AdaptiveShell({
    super.key,
    required this.configStore,
    required this.onConnected,
  });

  @override
  State<AdaptiveShell> createState() => _AdaptiveShellState();
}

class _AdaptiveShellState extends State<AdaptiveShell> {
  /// The session shown in the wide layout's detail pane, or null when nothing is
  /// selected. Re-resolved from the store on every build so it tracks live
  /// updates and survives a session vanishing (the detail body then shows its
  /// gone-state until dismissed).
  SessionInfo? _selected;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        if (constraints.maxWidth < kWideBreakpoint) {
          return SessionListPage(
            configStore: widget.configStore,
            onConnected: widget.onConnected,
          );
        }
        return _wide(context);
      },
    );
  }

  Widget _wide(BuildContext context) {
    final store = CommanderStoreScope.of(context)!;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        // Re-resolve the selection against the latest snapshot: pick up fresh
        // info, and fall back to the last-known info if the session vanished so
        // the detail pane can show its gone-state rather than blanking.
        final sel = _selected;
        final resolved = sel == null
            ? null
            : (store.sessionById(sel.id) ?? sel);
        return Scaffold(
          appBar: AppBar(
            title: const Text('Claude Commander'),
            actions: [
              IconButton(
                onPressed: store.refresh,
                icon: const Icon(Icons.refresh),
                tooltip: 'Refresh',
              ),
              SettingsMenu(
                store: store,
                configStore: widget.configStore,
                onConnected: widget.onConnected,
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
          body: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                width: 340,
                child: _MasterColumn(
                  store: store,
                  selectedId: resolved?.id,
                  onSelect: (s) => setState(() => _selected = s),
                ),
              ),
              const VerticalDivider(width: 1),
              Expanded(
                child: resolved == null
                    ? const _EmptyDetail()
                    : _DetailPane(
                        // Rekey per session so switching selection rebuilds the
                        // pane (resets the tab + tears down any live terminal).
                        key: ValueKey(resolved.id),
                        session: resolved,
                        api: store.api,
                        handle: store.handle,
                        onDismiss: () => setState(() => _selected = null),
                      ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// The wide layout's left column: a server sidebar slot on top (single server for
/// now) and the grouped session list below.
class _MasterColumn extends StatelessWidget {
  final CommanderStore store;
  final String? selectedId;
  final ValueChanged<SessionInfo> onSelect;

  const _MasterColumn({
    required this.store,
    required this.selectedId,
    required this.onSelect,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _ServerSidebar(store: store),
        const Divider(height: 1),
        Expanded(
          child: SessionListBody(selectedId: selectedId, onSelect: onSelect),
        ),
      ],
    );
  }
}

/// The reserved server-sidebar slot: the single connected server's name plus a
/// live connection indicator. Shaped as one row so a future multi-server list
/// drops a column of these in unchanged.
class _ServerSidebar extends StatelessWidget {
  final CommanderStore store;
  const _ServerSidebar({required this.store});

  @override
  Widget build(BuildContext context) {
    final (color, label) = _indicator(context, store.connection);
    return ListTile(
      dense: true,
      leading: Icon(Icons.dns_outlined, color: color),
      title: Text(
        store.config.baseUrl,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: Theme.of(context).textTheme.bodyMedium,
      ),
      subtitle: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(color: color, shape: BoxShape.circle),
          ),
          const SizedBox(width: 6),
          Text(label, style: Theme.of(context).textTheme.labelSmall),
        ],
      ),
    );
  }

  (Color, String) _indicator(BuildContext context, ConnectionStateDto conn) {
    final scheme = Theme.of(context).colorScheme;
    return switch (conn.kind) {
      ConnectionStateKind.connected => (Colors.green, 'Connected'),
      ConnectionStateKind.connecting => (scheme.tertiary, 'Connecting…'),
      ConnectionStateKind.degraded => (
        scheme.error,
        conn.reason.isEmpty ? 'Degraded' : 'Degraded: ${conn.reason}',
      ),
    };
  }
}

/// Placeholder shown in the wide detail pane when no session is selected.
class _EmptyDetail extends StatelessWidget {
  const _EmptyDetail();

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.touch_app_outlined,
            size: 48,
            color: Theme.of(context).colorScheme.outline,
          ),
          const SizedBox(height: 12),
          Text(
            'Select a session',
            style: Theme.of(context).textTheme.titleMedium,
          ),
        ],
      ),
    );
  }
}

enum _DetailTab { detail, terminal, shell, review }

/// The wide layout's right pane: a header with the session title and a
/// segmented control switching between the detail, terminal, and review bodies
/// in place (no route push).
class _DetailPane extends StatefulWidget {
  final SessionInfo session;
  final CommanderApi api;

  /// Null only transiently mid-reconnect; the terminal/review tabs need it, so
  /// they show a hint until a handle is available.
  final String? handle;

  /// Clear the selection (used by the detail body's delete/dismiss).
  final VoidCallback onDismiss;

  const _DetailPane({
    super.key,
    required this.session,
    required this.api,
    required this.handle,
    required this.onDismiss,
  });

  @override
  State<_DetailPane> createState() => _DetailPaneState();
}

class _DetailPaneState extends State<_DetailPane> {
  _DetailTab _tab = _DetailTab.detail;

  void _go(_DetailTab tab) => setState(() => _tab = tab);

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _header(context),
        const Divider(height: 1),
        Expanded(child: _content(context)),
      ],
    );
  }

  Widget _header(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
      child: Row(
        children: [
          Expanded(
            child: Text(
              widget.session.title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.titleMedium,
            ),
          ),
          const SizedBox(width: 8),
          SegmentedButton<_DetailTab>(
            showSelectedIcon: false,
            segments: const [
              ButtonSegment(
                value: _DetailTab.detail,
                icon: Icon(Icons.info_outline),
                label: Text('Detail'),
              ),
              ButtonSegment(
                value: _DetailTab.terminal,
                icon: Icon(Icons.terminal),
                label: Text('Terminal'),
              ),
              ButtonSegment(
                value: _DetailTab.shell,
                icon: Icon(Icons.code),
                label: Text('Shell'),
              ),
              ButtonSegment(
                value: _DetailTab.review,
                icon: Icon(Icons.rate_review),
                label: Text('Review'),
              ),
            ],
            selected: {_tab},
            onSelectionChanged: (s) => _go(s.first),
          ),
        ],
      ),
    );
  }

  Widget _content(BuildContext context) {
    final handle = widget.handle;
    switch (_tab) {
      case _DetailTab.detail:
        return SessionDetailBody(
          session: widget.session,
          onOpenTerminal: (kind) => _go(
            kind == AttachKind.shell ? _DetailTab.shell : _DetailTab.terminal,
          ),
          onOpenReview: () => _go(_DetailTab.review),
          onDeleted: widget.onDismiss,
          onDismiss: widget.onDismiss,
        );
      case _DetailTab.terminal:
        if (handle == null) return const _Reconnecting();
        return TerminalBody(
          api: widget.api,
          handle: handle,
          session: widget.session,
          // Desktop drives the terminal from the physical keyboard.
          showModifierBar: false,
        );
      case _DetailTab.shell:
        if (handle == null) return const _Reconnecting();
        return TerminalBody(
          // Rekey so switching agent<->shell tears down the old attach and
          // opens a fresh one against the paired shell pane.
          key: const ValueKey('shell'),
          api: widget.api,
          handle: handle,
          session: widget.session,
          kind: AttachKind.shell,
          showModifierBar: false,
        );
      case _DetailTab.review:
        if (handle == null) return const _Reconnecting();
        return ReviewBody(
          api: widget.api,
          handle: handle,
          session: widget.session,
        );
    }
  }
}

/// Shown in the terminal/review tabs while the server handle is momentarily
/// unavailable (mid-reconnect).
class _Reconnecting extends StatelessWidget {
  const _Reconnecting();

  @override
  Widget build(BuildContext context) =>
      const Center(child: Text('Reconnecting…'));
}
