import 'package:flutter/material.dart';

import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../state/workspace_store.dart';
import '../theme/app_colors.dart';
import '../theme/app_theme.dart';
import '../theme/agent_glyphs.dart';
import '../util/session_filter.dart';
import '../widgets/brand_mark.dart';
import '../widgets/session_chips.dart';
import 'activity_page.dart';
import 'phone_shell.dart';
import 'review_page.dart';
import 'session_detail_page.dart';
import 'session_list_page.dart';
import 'terminal_page.dart';

/// Logical width at or above which the app switches from the stacked phone
/// layout to the desktop/tablet rail + workspace layout.
const double kWideBreakpoint = 900;

/// Which surface the rail's footer toggle is driving the workspace to: the
/// selected session's [_DetailPane] (fleet), or the cross-server [ActivityBody]
/// feed (activity). The rail itself stays visible in both.
enum _RailMode { fleet, activity }

/// The responsive home. Below [kWideBreakpoint] it is the [PhoneShell] (a
/// bottom-nav Fleet + Activity shell over a stacked `Navigator.push` flow). At
/// or above it, the deck's two-pane layout: a persistent **Fleet rail** on the
/// left (brand + counts, search/segmented/list via [SessionListBody], and a
/// FLEET/ACTIVITY + settings + new-session footer) and a **workspace** on the
/// right whose Overview / Agent / Shell / Changes tabs switch in place.
///
/// The same page *bodies* ([SessionListBody], [SessionDetailBody], [TerminalBody],
/// [ReviewBody], [ActivityBody]) serve both layouts; only the surrounding shell
/// differs.
class AdaptiveShell extends StatefulWidget {
  const AdaptiveShell({super.key});

  @override
  State<AdaptiveShell> createState() => _AdaptiveShellState();
}

class _AdaptiveShellState extends State<AdaptiveShell> {
  /// The server that owns [_selected]. Held alongside the session so the
  /// workspace can be scoped to (and driven by) the right server.
  CommanderStore? _selectedStore;

  /// The session shown in the wide layout's workspace, or null when nothing is
  /// selected. Re-resolved from its owning store on every build so it tracks
  /// live updates and survives a session vanishing (the workspace then shows its
  /// gone-state until dismissed).
  SessionInfo? _selected;

  /// Whether the workspace shows the selected session (fleet) or the Activity
  /// feed. Selecting a session from the rail always snaps back to fleet.
  _RailMode _mode = _RailMode.fleet;

  void _select(CommanderStore store, SessionInfo session) => setState(() {
    _selectedStore = store;
    _selected = session;
    _mode = _RailMode.fleet;
  });

  void _clear() => setState(() {
    _selectedStore = null;
    _selected = null;
  });

  void _setMode(_RailMode mode) => setState(() => _mode = mode);

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        if (constraints.maxWidth < kWideBreakpoint) {
          return const PhoneShell();
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
        // the workspace can show its gone-state rather than blanking.
        final sel = _selected;
        final resolved = (store == null || sel == null)
            ? null
            : (store.sessionById(sel.id) ?? sel);

        // A single cross-server pass powers the rail header's mono counts.
        var active = 0, total = 0;
        for (final s in workspace.servers) {
          for (final x in s.sessions) {
            total++;
            if (x.status.isActive) active++;
          }
        }

        return Scaffold(
          body: SafeArea(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                SizedBox(
                  width: 312,
                  child: _Rail(
                    workspace: workspace,
                    active: active,
                    total: total,
                    serverCount: workspace.servers.length,
                    mode: _mode,
                    onModeChanged: _setMode,
                    selectedId: resolved?.id,
                    onSelect: _select,
                  ),
                ),
                const VerticalDivider(width: 1, color: AppColors.borderSubtle),
                Expanded(
                  child: _workspace(context, workspace, store, resolved),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  /// The right pane: the Activity feed when the rail is toggled to activity,
  /// otherwise the selected session's workspace (or the empty-state).
  Widget _workspace(
    BuildContext context,
    WorkspaceStore workspace,
    CommanderStore? store,
    SessionInfo? resolved,
  ) {
    if (_mode == _RailMode.activity) {
      return const ActivityBody(showHeader: true);
    }
    if (resolved == null || store == null) return const _EmptyDetail();
    return CommanderStoreScope(
      store: store,
      // Rekey per (server, session) so switching selection rebuilds the pane
      // (resets the tab + tears down any live terminal).
      child: _DetailPane(
        key: ValueKey('${store.config.id}:${resolved.id}'),
        session: resolved,
        api: store.api,
        handle: store.handle,
        onRefresh: workspace.refreshAll,
        onDismiss: _clear,
      ),
    );
  }
}

/// The persistent Fleet rail: a branded header (brand mark + "Fleet" + mono
/// counts), the shared [SessionListBody] (search + Recent/All + quick-filter
/// chips + the session list), and a footer carrying the FLEET/ACTIVITY toggle,
/// the settings menu, and the new-session button.
class _Rail extends StatelessWidget {
  final WorkspaceStore workspace;
  final int active;
  final int total;
  final int serverCount;
  final _RailMode mode;
  final ValueChanged<_RailMode> onModeChanged;
  final String? selectedId;
  final void Function(CommanderStore store, SessionInfo session) onSelect;

  const _Rail({
    required this.workspace,
    required this.active,
    required this.total,
    required this.serverCount,
    required this.mode,
    required this.onModeChanged,
    required this.selectedId,
    required this.onSelect,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      color: AppColors.bg,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _header(),
          Expanded(
            child: SessionListBody(selectedId: selectedId, onSelect: onSelect),
          ),
          _footer(context),
        ],
      ),
    );
  }

  Widget _header() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 18, 16, 12),
      child: Row(
        children: [
          const BrandMark(size: 30),
          const SizedBox(width: 11),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'Fleet',
                  style: TextStyle(
                    fontSize: 19,
                    fontWeight: FontWeight.w700,
                    letterSpacing: -0.4,
                    color: AppColors.text,
                  ),
                ),
                const SizedBox(height: 1),
                Text(
                  '$active active · $total total · $serverCount '
                  'server${serverCount == 1 ? '' : 's'}',
                  style: AppTheme.mono(size: 10),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _footer(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
      decoration: const BoxDecoration(
        border: Border(top: BorderSide(color: AppColors.borderSubtle)),
      ),
      child: Row(
        children: [
          _ModeToggle(
            glyph: '▤',
            label: 'FLEET',
            selected: mode == _RailMode.fleet,
            onTap: () => onModeChanged(_RailMode.fleet),
          ),
          const SizedBox(width: 14),
          _ModeToggle(
            glyph: '≋',
            label: 'ACTIVITY',
            selected: mode == _RailMode.activity,
            onTap: () => onModeChanged(_RailMode.activity),
          ),
          const Spacer(),
          SettingsMenu(
            workspace: workspace,
            button: const _RailIconButton(icon: Icons.settings),
          ),
          const SizedBox(width: 8),
          _RailIconButton(
            icon: Icons.add,
            accent: true,
            tooltip: 'New session',
            onTap: () => openCreateSession(context, workspace),
          ),
        ],
      ),
    );
  }
}

/// One FLEET/ACTIVITY footer toggle: the deck's glyph + mono label, tinted
/// accent when active and muted otherwise.
class _ModeToggle extends StatelessWidget {
  final String glyph;
  final String label;
  final bool selected;
  final VoidCallback onTap;

  const _ModeToggle({
    required this.glyph,
    required this.label,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final color = selected ? AppColors.accent : AppColors.textFaint;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(6),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 4),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              glyph,
              style: TextStyle(fontSize: 14, color: color, height: 1),
            ),
            const SizedBox(width: 6),
            Text(
              label,
              style: AppTheme.mono(
                size: 10,
                weight: FontWeight.w600,
                color: color,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A rounded-square rail footer icon button (settings ⚙, new-session +). The
/// accent variant fills violet for the primary new-session action.
class _RailIconButton extends StatelessWidget {
  final IconData icon;
  final bool accent;
  final String? tooltip;
  final VoidCallback? onTap;

  const _RailIconButton({
    required this.icon,
    this.accent = false,
    this.tooltip,
    this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final button = InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(9),
      child: Container(
        width: 32,
        height: 32,
        decoration: BoxDecoration(
          color: accent ? AppColors.accent : AppColors.surface,
          borderRadius: BorderRadius.circular(9),
          border: accent ? null : Border.all(color: AppColors.border),
        ),
        child: Icon(
          icon,
          size: 17,
          color: accent ? AppColors.bg : AppColors.textMuted,
        ),
      ),
    );
    return tooltip == null ? button : Tooltip(message: tooltip!, child: button);
  }
}

/// Placeholder shown in the workspace when no session is selected.
class _EmptyDetail extends StatelessWidget {
  const _EmptyDetail();

  @override
  Widget build(BuildContext context) {
    return Container(
      color: AppColors.bg,
      alignment: Alignment.center,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const BrandMark(size: 44),
          const SizedBox(height: 16),
          Text(
            'Select a session',
            style: Theme.of(context).textTheme.titleMedium,
          ),
          const SizedBox(height: 6),
          Text(
            'Pick a session from the fleet to open its workspace.',
            style: AppTheme.mono(size: 11, color: AppColors.textFaint),
          ),
        ],
      ),
    );
  }
}

/// The workspace tabs. Display labels are Overview / Agent / Shell / Changes;
/// the enum spellings are kept from the previous segmented control so the wiring
/// (detail→Overview, terminal→Agent, shell→Shell, review→Changes) is unchanged.
enum _DetailTab {
  detail('Overview'),
  terminal('Agent'),
  shell('Shell'),
  review('Changes');

  const _DetailTab(this.label);
  final String label;
}

/// The wide layout's workspace: a header (state glyph + title + PR badge + mono
/// meta + action buttons), the deck's underline tab row, and the active tab's
/// body switched in place (no route push).
class _DetailPane extends StatefulWidget {
  final SessionInfo session;
  final CommanderApi api;

  /// Null only transiently mid-reconnect; the terminal/review tabs need it, so
  /// they show a hint until a handle is available.
  final String? handle;

  /// Refreshes every server (the workspace header's refresh button).
  final Future<void> Function() onRefresh;

  /// Clear the selection (used by the detail body's delete/dismiss).
  final VoidCallback onDismiss;

  const _DetailPane({
    super.key,
    required this.session,
    required this.api,
    required this.handle,
    required this.onRefresh,
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
    return Container(
      color: AppColors.bg,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _header(context),
          _tabs(context),
          Expanded(child: _content(context)),
        ],
      ),
    );
  }

  Widget _header(BuildContext context) {
    final session = widget.session;
    final store = CommanderStoreScope.of(context);
    final descriptor = sessionDescriptor(
      session,
      store?.agentStateFor(session.id) ?? AgentState.unknown,
    );
    final serverName = store?.config.name;
    final meta = [
      session.projectName,
      session.branch,
      session.program,
      ?serverName,
    ].join(' · ');
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 12, 12),
      decoration: const BoxDecoration(
        border: Border(bottom: BorderSide(color: AppColors.divider)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    SessionGlyph(descriptor, size: 12),
                    const SizedBox(width: 6),
                    Flexible(
                      child: Text(
                        session.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          fontSize: 17,
                          fontWeight: FontWeight.w700,
                          letterSpacing: -0.2,
                          color: AppColors.text,
                        ),
                      ),
                    ),
                    if (session.prNumber != null) ...[
                      const SizedBox(width: 9),
                      prChip(context, session.prNumber!, session.prState),
                    ],
                  ],
                ),
                const SizedBox(height: 5),
                Text(
                  meta,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: AppTheme.mono(size: 11),
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          _RailIconButton(
            icon: Icons.refresh,
            tooltip: 'Refresh',
            onTap: () => widget.onRefresh(),
          ),
        ],
      ),
    );
  }

  /// The deck's underline tab row: each tab is a mono label; the active one is
  /// bright with a 2px accent underline.
  Widget _tabs(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 10, 20, 0),
      decoration: const BoxDecoration(
        border: Border(bottom: BorderSide(color: AppColors.divider)),
      ),
      child: Row(
        children: [for (final tab in _DetailTab.values) _tabItem(tab)],
      ),
    );
  }

  Widget _tabItem(_DetailTab tab) {
    final selected = _tab == tab;
    return Padding(
      padding: const EdgeInsets.only(right: 22),
      child: InkWell(
        key: ValueKey('ws-tab-${tab.name}'),
        onTap: () => _go(tab),
        child: Container(
          padding: const EdgeInsets.only(bottom: 11),
          decoration: BoxDecoration(
            border: Border(
              bottom: BorderSide(
                color: selected ? AppColors.accent : Colors.transparent,
                width: 2,
              ),
            ),
          ),
          child: Text(
            tab.label,
            style: AppTheme.mono(
              size: 12,
              weight: FontWeight.w600,
              color: selected ? AppColors.text : AppColors.textMuted,
            ),
          ),
        ),
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
