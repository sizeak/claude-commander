import 'package:flutter/material.dart';

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
/// terminal/review). At or above it, a master-detail desktop layout: the grouped
/// (multi-server) session list on the left, and a persistent detail pane on the
/// right whose detail / terminal / review views are switched in place.
///
/// The same page *bodies* ([SessionListBody], [SessionDetailBody], [TerminalBody],
/// [ReviewBody]) serve both layouts; only the surrounding shell differs.
class AdaptiveShell extends StatefulWidget {
  const AdaptiveShell({super.key});

  @override
  State<AdaptiveShell> createState() => _AdaptiveShellState();
}

class _AdaptiveShellState extends State<AdaptiveShell> {
  /// The server that owns [_selected]. Held alongside the session so the detail
  /// pane can be scoped to (and driven by) the right server.
  CommanderStore? _selectedStore;

  /// The session shown in the wide layout's detail pane, or null when nothing is
  /// selected. Re-resolved from its owning store on every build so it tracks
  /// live updates and survives a session vanishing (the detail body then shows
  /// its gone-state until dismissed).
  SessionInfo? _selected;

  void _select(CommanderStore store, SessionInfo session) => setState(() {
    _selectedStore = store;
    _selected = session;
  });

  void _clear() => setState(() {
    _selectedStore = null;
    _selected = null;
  });

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        if (constraints.maxWidth < kWideBreakpoint) {
          return const SessionListPage();
        }
        return _wide(context);
      },
    );
  }

  Widget _wide(BuildContext context) {
    final workspace = WorkspaceScope.of(context)!;
    return ListenableBuilder(
      listenable: workspace,
      builder: (context, _) {
        // Drop a selection whose server was removed.
        var store = _selectedStore;
        if (store != null && !workspace.servers.contains(store)) {
          store = null;
          _selectedStore = null;
          _selected = null;
        }
        // Re-resolve the selection against the latest snapshot: pick up fresh
        // info, and fall back to the last-known info if the session vanished so
        // the detail pane can show its gone-state rather than blanking.
        final sel = _selected;
        final resolved = (store == null || sel == null)
            ? null
            : (store.sessionById(sel.id) ?? sel);
        return Scaffold(
          appBar: AppBar(
            title: const Text('Claude Commander'),
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
          body: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                width: 340,
                child: SessionListBody(
                  selectedId: resolved?.id,
                  onSelect: _select,
                ),
              ),
              const VerticalDivider(width: 1),
              Expanded(
                child: (resolved == null || store == null)
                    ? const _EmptyDetail()
                    : CommanderStoreScope(
                        store: store,
                        // Rekey per (server, session) so switching selection
                        // rebuilds the pane (resets the tab + tears down any
                        // live terminal).
                        child: _DetailPane(
                          key: ValueKey('${store.config.id}:${resolved.id}'),
                          session: resolved,
                          api: store.api,
                          handle: store.handle,
                          onDismiss: _clear,
                        ),
                      ),
              ),
            ],
          ),
        );
      },
    );
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
          // The wide landscape layout has room for the terminal snapshot
          // alongside everything else, so keep it.
          showPanePreview: true,
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
