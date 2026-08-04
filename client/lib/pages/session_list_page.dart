import 'package:flutter/material.dart';

import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../state/workspace_store.dart';
import '../theme/app_colors.dart';
import '../theme/app_theme.dart';
import '../theme/agent_glyphs.dart';
import '../util/format.dart';
import '../util/session_filter.dart';
import '../widgets/brand_mark.dart';
import '../widgets/session_chips.dart';
import 'create_session_page.dart';
import 'programs_page.dart';
import 'projects_page.dart';
import 'servers_page.dart';
import 'session_detail_page.dart';

/// Which slice of the sessions the list is showing: everything (grouped by
/// server → project) or the active ones in MRU order (most recently attached,
/// or created for one not yet attached).
enum _SessionView { recent, all }

/// A one-tap "attention" filter layered on top of the search box: the sessions
/// that need input, the ones actively working, and the ones with an open PR
/// awaiting review. Mirrors the deck's quick-filter chip row.
enum _Quick { needsInput, working, review }

/// True when [store]'s session [s] is asking for a human decision — an agent
/// waiting for input, or a cascade paused mid-stack.
bool _isNeedsInput(CommanderStore store, SessionInfo s) =>
    s.status == SessionStatus.cascadePaused ||
    store.agentStateFor(s.id) == AgentState.waitingForInput;

/// True when [s]'s agent is actively working.
bool _isWorking(CommanderStore store, SessionInfo s) =>
    store.agentStateFor(s.id) == AgentState.working;

/// True when [s] carries an open PR — a candidate for review.
bool _isReview(SessionInfo s) =>
    s.prNumber != null && s.prState == PrState.open;

bool _matchesQuick(_Quick q, CommanderStore store, SessionInfo s) =>
    switch (q) {
      _Quick.needsInput => _isNeedsInput(store, s),
      _Quick.working => _isWorking(store, s),
      _Quick.review => _isReview(s),
    };

/// Whether a server's base URL points at the local machine (drives the
/// `local` / `remote` tag in the server node header). Compares the parsed
/// [Uri.host] so a name like `notlocalhost.example` isn't misread as local.
bool _isLocalServer(String baseUrl) {
  final host = Uri.tryParse(baseUrl)?.host.toLowerCase() ?? '';
  return host == 'localhost' ||
      host == '127.0.0.1' ||
      host == '::1' ||
      host == '[::1]';
}

/// The aggregated session list — layout-agnostic (no Scaffold, no route). A
/// pinned header carries a live search box (fuzzy-filtering the list in place),
/// a Recent/All toggle, and a row of quick-filter chips; below it the body is
/// either the servers' sessions grouped by project (All) or a flat, cross-server
/// most-recently-attached list (Recent). Enumerates the servers from the
/// [WorkspaceStore]; in All mode each server section re-provides its own
/// [CommanderStoreScope] so per-server consumers (detail, cascade banner) keep
/// their single-store contract, and its header is suppressed when only one
/// server is configured.
class SessionListBody extends StatefulWidget {
  /// The id of the session shown in the detail pane, highlighted in the list.
  /// Null in the narrow (push) flow, where there is no persistent selection.
  final String? selectedId;

  /// Invoked when a session row is tapped, with the server that owns it.
  final void Function(CommanderStore store, SessionInfo session) onSelect;

  /// Whether to render the branded "Fleet" header (BrandMark + counts + a
  /// settings button). The phone [PhoneShell] turns this on; the wide layout's
  /// rail supplies its own header and footer, so it leaves this off.
  final bool showFleetHeader;

  const SessionListBody({
    super.key,
    this.selectedId,
    required this.onSelect,
    this.showFleetHeader = false,
  });

  @override
  State<SessionListBody> createState() => _SessionListBodyState();
}

class _SessionListBodyState extends State<SessionListBody> {
  final TextEditingController _search = TextEditingController();
  String _query = '';
  _SessionView _view = _SessionView.all;

  /// The active quick filter, or null for none. Tapping the active chip clears
  /// it, so it round-trips off.
  _Quick? _quick;

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  void _setQuery(String value) => setState(() => _query = value.trim());

  void _toggleQuick(_Quick q) =>
      setState(() => _quick = _quick == q ? null : q);

  /// The active quick filter, or null when it no longer matches anything (its
  /// chip has dropped out of the row, so it can't be cleared by hand).
  _Quick? _liveQuick({
    required int needs,
    required int working,
    required int review,
  }) {
    final count = switch (_quick) {
      null => 0,
      _Quick.needsInput => needs,
      _Quick.working => working,
      _Quick.review => review,
    };
    return count > 0 ? _quick : null;
  }

  @override
  Widget build(BuildContext context) {
    final workspace = WorkspaceScope.of(context)!;
    return ListenableBuilder(
      listenable: workspace,
      builder: (context, _) {
        final servers = workspace.servers;
        final multi = servers.length > 1;

        // A single cross-server pass powers the header counts and the chip row.
        var active = 0, total = 0, needs = 0, working = 0, review = 0;
        for (final store in servers) {
          for (final s in store.sessions) {
            total++;
            if (s.status.isActive) active++;
            if (_isNeedsInput(store, s)) needs++;
            if (_isWorking(store, s)) working++;
            if (_isReview(s)) review++;
          }
        }
        // A chip only renders while its count is non-zero, and tapping it is the
        // only way to clear the filter — so once the count reaches zero (the
        // agent stopped waiting, the PR merged) a still-applied filter is
        // unreachable, and every session stays hidden behind a bare "No
        // matches" with nothing on screen to explain why. Drop it instead. Safe
        // to assign during build: the value we render with is computed here, so
        // this needs no rebuild of its own.
        _quick = _liveQuick(needs: needs, working: working, review: review);

        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (widget.showFleetHeader)
              _FleetHeader(
                workspace: workspace,
                active: active,
                total: total,
                serverCount: servers.length,
              ),
            _buildControls(needs: needs, working: working, review: review),
            // With several servers each group header carries its own connection
            // dot; a lone server has no group header (nor, in the phone shell,
            // an AppBar), so surface its connection state here when it isn't
            // healthy — otherwise a degraded/reconnecting sole server is silent.
            if (servers.length == 1 &&
                servers.single.connection.kind != ConnectionStateKind.connected)
              _ConnectionStrip(connection: servers.single.connection),
            Expanded(
              child: RefreshIndicator(
                onRefresh: workspace.refreshAll,
                child: _view == _SessionView.recent
                    ? _buildRecent(context, servers)
                    : _buildAll(servers, multi),
              ),
            ),
          ],
        );
      },
    );
  }

  /// The search box, the Recent/All segmented toggle with a mode indicator, and
  /// the quick-filter chip row.
  Widget _buildControls({
    required int needs,
    required int working,
    required int review,
  }) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 6, 16, 0),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          TextField(
            controller: _search,
            onChanged: _setQuery,
            textInputAction: TextInputAction.search,
            style: const TextStyle(fontSize: 13.5, color: AppColors.text),
            decoration: InputDecoration(
              isDense: true,
              prefixIcon: const Icon(Icons.search, size: 18),
              prefixIconColor: AppColors.textFaint,
              hintText: 'Filter by name, branch, program…',
              suffixIcon: _search.text.isEmpty
                  ? null
                  : IconButton(
                      icon: const Icon(Icons.clear, size: 18),
                      tooltip: 'Clear',
                      onPressed: () {
                        _search.clear();
                        _setQuery('');
                      },
                    ),
            ),
          ),
          const SizedBox(height: 9),
          _segmented(),
          _quickChips(needs: needs, working: working, review: review),
        ],
      ),
    );
  }

  Widget _segmented() {
    return Row(
      children: [
        Expanded(
          child: Container(
            decoration: BoxDecoration(
              color: AppColors.surface,
              borderRadius: BorderRadius.circular(9),
              border: Border.all(color: AppColors.border),
            ),
            padding: const EdgeInsets.all(3),
            child: Row(
              children: [
                _segment('Recent', _SessionView.recent),
                _segment('All', _SessionView.all),
              ],
            ),
          ),
        ),
        const SizedBox(width: 9),
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 9),
          decoration: BoxDecoration(
            color: AppColors.surface,
            borderRadius: BorderRadius.circular(9),
            border: Border.all(color: AppColors.border),
          ),
          child: Text(
            _view == _SessionView.recent ? '↓ recency' : 'grouped',
            style: AppTheme.mono(
              size: 10,
              weight: FontWeight.w600,
              color: AppColors.textBright,
            ),
          ),
        ),
      ],
    );
  }

  Widget _segment(String label, _SessionView view) {
    final selected = _view == view;
    return Expanded(
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => setState(() => _view = view),
        child: Container(
          padding: const EdgeInsets.symmetric(vertical: 6),
          decoration: selected
              ? BoxDecoration(
                  color: AppColors.surfaceSel,
                  borderRadius: BorderRadius.circular(6),
                )
              : null,
          child: Text(
            label,
            textAlign: TextAlign.center,
            style: AppTheme.mono(
              size: 11,
              weight: FontWeight.w600,
              color: selected ? AppColors.text : AppColors.textMuted,
            ),
          ),
        ),
      ),
    );
  }

  /// The quick-filter chip row. Only chips with a non-zero count show, so a
  /// filter never dead-ends on an empty set. The needs-input chip is always
  /// amber-tinted (it's the one that wants attention).
  Widget _quickChips({
    required int needs,
    required int working,
    required int review,
  }) {
    final chips = <Widget>[
      if (needs > 0)
        _QuickChip(
          label: '? needs input · $needs',
          amber: true,
          selected: _quick == _Quick.needsInput,
          onTap: () => _toggleQuick(_Quick.needsInput),
        ),
      if (working > 0)
        _QuickChip(
          label: 'working · $working',
          selected: _quick == _Quick.working,
          onTap: () => _toggleQuick(_Quick.working),
        ),
      if (review > 0)
        _QuickChip(
          label: 'review · $review',
          selected: _quick == _Quick.review,
          onTap: () => _toggleQuick(_Quick.review),
        ),
    ];
    if (chips.isEmpty) return const SizedBox(height: 10);
    return Padding(
      padding: const EdgeInsets.only(top: 10, bottom: 2),
      child: SizedBox(
        height: 26,
        child: ListView.separated(
          scrollDirection: Axis.horizontal,
          itemCount: chips.length,
          separatorBuilder: (_, _) => const SizedBox(width: 6),
          itemBuilder: (_, i) => chips[i],
        ),
      ),
    );
  }

  Widget _buildAll(List<CommanderStore> servers, bool multi) {
    return ListView(
      padding: const EdgeInsets.only(top: 6, bottom: 12),
      children: [
        for (final store in servers)
          _ServerSection(
            store: store,
            showHeader: multi,
            selectedId: widget.selectedId,
            onSelect: widget.onSelect,
            query: _query,
            quick: _quick,
          ),
      ],
    );
  }

  /// The Recent tab: every server's sessions flattened to (store, session)
  /// pairs, active-and-attached-only, newest-attach first (the TUI's MRU
  /// order). A live query filters that set by fuzzy score, ranking best matches
  /// first while keeping recency as the stable tie-break; an active quick filter
  /// narrows it further.
  ///
  /// Two deliberate differences from the TUI's pinned recents block, and they
  /// go together: this tab is uncapped (the TUI caps at `recent_sessions_limit`)
  /// *and* it excludes stopped sessions (the TUI shows any attached session
  /// regardless of status). The TUI's fixed cap already bounds how much stale
  /// history shows; an uncapped list has no such bound, so it filters to active
  /// sessions to avoid accumulating dead ones indefinitely.
  ///
  /// A third: the ordering key is [sessionRecency], not `lastAttachedAt`, so a
  /// never-attached session sorts by its creation time instead of dropping out.
  /// The TUI can drop those, because its recents block is pinned *above* the
  /// full tree; here Recent is one of two exclusive tabs, so dropping a session
  /// hides it outright — including the one the user has only just created.
  Widget _buildRecent(BuildContext context, List<CommanderStore> servers) {
    var pairs = <(CommanderStore, SessionInfo)>[
      for (final store in servers)
        for (final s in store.sessions)
          if (s.status.isActive &&
              (_quick == null || _matchesQuick(_quick!, store, s)))
            (store, s),
    ];
    pairs = mostRecent(pairs, (p) => sessionRecency(p.$2));
    if (_query.isNotEmpty) {
      pairs = rankByScore(pairs, (p) => sessionFuzzyScore(p.$2, _query));
    }

    if (pairs.isEmpty) {
      return ListView(children: [_recentEmptyState(context, servers)]);
    }
    return ListView(
      padding: const EdgeInsets.fromLTRB(12, 2, 12, 12),
      children: [
        for (final (store, session) in pairs)
          _RecentTile(
            store: store,
            session: session,
            selected: session.id == widget.selectedId,
            onTap: () => widget.onSelect(store, session),
          ),
      ],
    );
  }

  /// What to show when the flattened recent list is empty. A bare "No recent
  /// sessions" would hide a server that is merely still connecting or down, so
  /// mirror All mode: surface a spinner while any server is loading and an
  /// error+Retry when one has failed, before falling back to the empty note.
  Widget _recentEmptyState(BuildContext context, List<CommanderStore> servers) {
    // Loading/error take priority over the query notes, so typing while the
    // only server is still connecting shows the spinner (as All mode does),
    // not a misleading "No matches".
    final loading = servers.any((s) => s.workspace == null && s.error == null);
    if (loading) {
      return const Padding(
        padding: EdgeInsets.symmetric(vertical: 24),
        child: Center(child: CircularProgressIndicator()),
      );
    }
    final failed = servers.where((s) => s.error != null).toList();
    if (failed.isNotEmpty) {
      final store = failed.first;
      return _InlineNote(
        icon: Icons.cloud_off,
        text: store.error.toString(),
        action: ('Retry', store.retry),
        color: AppColors.red,
      );
    }
    if (_query.isNotEmpty || _quick != null) {
      return const _InlineNote(icon: Icons.search_off, text: 'No matches');
    }
    return const _InlineNote(icon: Icons.history, text: 'No recent sessions');
  }
}

/// The branded Fleet header: the [BrandMark], a "Fleet" title, a mono line of
/// aggregate counts, and a settings button that opens the shared [SettingsMenu].
/// Shown only in the phone shell (the wide/legacy AppBar carries these instead).
class _FleetHeader extends StatelessWidget {
  final WorkspaceStore workspace;
  final int active;
  final int total;
  final int serverCount;

  const _FleetHeader({
    required this.workspace,
    required this.active,
    required this.total,
    required this.serverCount,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 10, 16, 6),
      child: Row(
        children: [
          const BrandMark(size: 32),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'Fleet',
                  style: TextStyle(
                    fontSize: 23,
                    fontWeight: FontWeight.w700,
                    letterSpacing: -0.4,
                    color: AppColors.text,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  '$active active · $total total · $serverCount '
                  'server${serverCount == 1 ? '' : 's'}',
                  style: AppTheme.mono(size: 10.5),
                ),
              ],
            ),
          ),
          SettingsMenu(
            workspace: workspace,
            button: Container(
              width: 34,
              height: 34,
              decoration: BoxDecoration(
                color: AppColors.surface,
                borderRadius: BorderRadius.circular(10),
                border: Border.all(color: AppColors.border),
              ),
              child: const Icon(
                Icons.settings,
                size: 16,
                color: AppColors.textMuted,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A tappable quick-filter pill. Selected fills with its active colour (amber
/// for the needs-input chip, accent otherwise); an unselected needs-input chip
/// keeps a faint amber tint, other unselected chips are a neutral surface pill.
class _QuickChip extends StatelessWidget {
  final String label;
  final bool selected;
  final bool amber;
  final VoidCallback onTap;

  const _QuickChip({
    required this.label,
    required this.selected,
    required this.onTap,
    this.amber = false,
  });

  @override
  Widget build(BuildContext context) {
    final active = amber ? AppColors.amber : AppColors.accent;
    final Color bg, borderColor, textColor;
    if (selected) {
      bg = active.withValues(alpha: 0.2);
      borderColor = active.withValues(alpha: 0.7);
      textColor = amber ? AppColors.amberText : AppColors.accentSoft;
    } else if (amber) {
      bg = AppColors.amber.withValues(alpha: 0.14);
      borderColor = AppColors.amber.withValues(alpha: 0.4);
      textColor = AppColors.amberText;
    } else {
      bg = AppColors.surface;
      borderColor = AppColors.border;
      textColor = AppColors.textMuted;
    }
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(20),
        child: Container(
          alignment: Alignment.center,
          padding: const EdgeInsets.symmetric(horizontal: 11),
          decoration: BoxDecoration(
            color: bg,
            borderRadius: BorderRadius.circular(20),
            border: Border.all(color: borderColor),
          ),
          child: Text(
            label,
            style: AppTheme.mono(
              size: 10,
              weight: FontWeight.w600,
              color: textColor,
            ),
          ),
        ),
      ),
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

  /// The active search query. Empty shows the full grouped list; otherwise each
  /// group is fuzzy-filtered and emptied groups drop out.
  final String query;

  /// The active quick filter (or null), applied on top of [query].
  final _Quick? quick;

  const _ServerSection({
    required this.store,
    required this.showHeader,
    required this.selectedId,
    required this.onSelect,
    required this.query,
    required this.quick,
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
            color: AppColors.red,
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
    final groups = <ProjectSessions>[];
    for (final g in store.sessionsByProject) {
      var sessions = matchingSessions(g.sessions, query);
      if (quick != null) {
        sessions = [
          for (final s in sessions)
            if (_matchesQuick(quick!, store, s)) s,
        ];
      }
      if (sessions.isNotEmpty) {
        groups.add(ProjectSessions(project: g.project, sessions: sessions));
      }
    }
    final filtering = query.isNotEmpty || quick != null;
    return [
      if (store.cascadePaused != null) const CascadeBanner(),
      if (groups.isEmpty)
        _InlineNote(
          icon: filtering ? Icons.search_off : Icons.inbox_outlined,
          text: filtering ? 'No matches' : 'No sessions',
        )
      else
        for (final group in groups) ...[
          _ProjectHeader(
            name: group.project.name,
            count: group.sessions.length,
          ),
          for (final session in group.sessions)
            Padding(
              padding: const EdgeInsets.fromLTRB(14, 0, 12, 6),
              child: _GroupedTile(
                store: store,
                session: session,
                selected: session.id == selectedId,
                onTap: () => onSelect(store, session),
              ),
            ),
        ],
    ];
  }
}

/// Push a session's detail route, re-providing the owning server's scope so the
/// detail page's markRead/cascade/terminal/review calls hit the right server.
/// Shared by the redesigned [PhoneShell] and the wide shell's list body.
Future<void> openSessionDetail(
  BuildContext context,
  CommanderStore store,
  SessionInfo session,
) async {
  await Navigator.of(context).push<bool>(
    MaterialPageRoute(
      builder: (_) => CommanderStoreScope(
        store: store,
        child: SessionDetailPage(session: session),
      ),
    ),
  );
  // A lifecycle action bumps the change feed, so the list refreshes itself.
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
/// The page refetches the snapshot itself before popping, so this route returns
/// to a list that already holds the new session.
Future<void> openCreateSession(
  BuildContext context,
  WorkspaceStore workspace,
) async {
  final store = await pickServer(context, workspace, title: 'Create on…');
  if (store == null || store.handle == null || !context.mounted) return;
  await Navigator.of(context).push<String>(
    MaterialPageRoute(builder: (_) => CreateSessionPage(store: store)),
  );
}

/// Push the servers manager (add/edit/remove).
void openServers(BuildContext context, WorkspaceStore workspace) {
  Navigator.of(
    context,
  ).push(MaterialPageRoute(builder: (_) => ServersPage(workspace: workspace)));
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
    MaterialPageRoute(
      builder: (_) => ProgramsPage(api: store.api, handle: handle),
    ),
  );
}

/// Push the projects manager for a chosen server (add/remove/scan + branches).
Future<void> openProjects(
  BuildContext context,
  WorkspaceStore workspace,
) async {
  final store = await pickServer(context, workspace, title: 'Projects on…');
  if (store == null || store.handle == null || !context.mounted) return;
  Navigator.of(
    context,
  ).push(MaterialPageRoute(builder: (_) => ProjectsPage(store: store)));
}

/// True when at least one server has a live handle (per-server actions need one).
bool _anyConnected(WorkspaceStore workspace) =>
    workspace.servers.any((s) => s.handle != null);

/// The settings menu shared by both shells: manage servers, plus the per-server
/// projects/programs editors (which prompt for a server when more than one is
/// configured). Renders as a plain settings icon by default; pass [button] to
/// substitute a bespoke trigger (the Fleet header's rounded ⚙ tile).
class SettingsMenu extends StatelessWidget {
  final WorkspaceStore workspace;

  /// An optional custom trigger widget. When null the menu shows the default
  /// settings icon.
  final Widget? button;

  const SettingsMenu({super.key, required this.workspace, this.button});

  @override
  Widget build(BuildContext context) {
    final enabled = _anyConnected(workspace);
    return PopupMenuButton<String>(
      icon: button == null ? const Icon(Icons.settings) : null,
      tooltip: 'Settings',
      position: PopupMenuPosition.under,
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
      child: button,
    );
  }
}

/// A slim in-body status strip for the lone-server case, shown while connecting
/// or degraded (a healthy connection needs no chrome). Rendered by
/// [SessionListBody] between the controls and the list, so a sole server's
/// connection state is visible in both the Recent and All views even without an
/// AppBar or a per-server group header.
class _ConnectionStrip extends StatelessWidget {
  final ConnectionStateDto connection;
  const _ConnectionStrip({required this.connection});

  @override
  Widget build(BuildContext context) {
    final (label, color) = switch (connection.kind) {
      // The call site only renders this strip when kind != connected, so this
      // arm is for exhaustiveness — the strip never shows for a healthy server.
      ConnectionStateKind.connected => ('Connected', AppColors.teal),
      ConnectionStateKind.connecting => ('Connecting…', AppColors.amber),
      ConnectionStateKind.degraded => (
        connection.reason.isEmpty
            ? 'Connection degraded'
            : 'Degraded: ${connection.reason}',
        AppColors.red,
      ),
    };
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.fromLTRB(16, 8, 16, 0),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(9),
        border: Border.all(color: color.withValues(alpha: 0.4)),
      ),
      child: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(color: color, shape: BoxShape.circle),
          ),
          const SizedBox(width: 9),
          Expanded(
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: AppTheme.mono(
                size: 10.5,
                weight: FontWeight.w600,
                color: color,
              ),
            ),
          ),
        ],
      ),
    );
  }
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
    return Container(
      margin: const EdgeInsets.fromLTRB(12, 12, 12, 4),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: AppColors.amber.withValues(alpha: 0.09),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: AppColors.amber.withValues(alpha: 0.4)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.pause_circle_outline, color: AppColors.amber),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  'Cascade paused — awaiting a decision',
                  style: Theme.of(
                    context,
                  ).textTheme.titleSmall?.copyWith(color: AppColors.amberText),
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
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
    );
  }
}

/// A server-group header (the deck's "server node"): a live connection dot, the
/// server name, and a `N · local/remote` count. A degraded server is greyed +
/// dimmed with an "unreachable" note in place of the count (mirrors the TUI), so
/// a down server reads as inert but never vanishes from the list.
class _ServerHeader extends StatelessWidget {
  final CommanderStore store;
  const _ServerHeader({required this.store});

  @override
  Widget build(BuildContext context) {
    final conn = store.connection;
    final (dotColor, note, noteColor, degraded) = switch (conn.kind) {
      ConnectionStateKind.connected => (AppColors.teal, null, null, false),
      ConnectionStateKind.connecting => (
        AppColors.amber,
        'connecting…',
        AppColors.amberText,
        false,
      ),
      ConnectionStateKind.degraded => (
        AppColors.idle,
        conn.reason.isEmpty ? 'unreachable' : conn.reason,
        AppColors.red,
        true,
      ),
    };
    final count = store.sessions.length;
    final tag = _isLocalServer(store.config.baseUrl) ? 'local' : 'remote';
    return Opacity(
      opacity: degraded ? 0.6 : 1,
      child: Container(
        padding: const EdgeInsets.fromLTRB(4, 10, 8, 8),
        decoration: const BoxDecoration(
          border: Border(bottom: BorderSide(color: AppColors.divider)),
        ),
        child: Row(
          children: [
            Container(
              width: 8,
              height: 8,
              decoration: BoxDecoration(
                color: dotColor,
                shape: BoxShape.circle,
                boxShadow: degraded
                    ? null
                    : [BoxShadow(color: dotColor, blurRadius: 7)],
              ),
            ),
            const SizedBox(width: 9),
            Flexible(
              child: Text(
                store.config.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  color: AppColors.text,
                ),
              ),
            ),
            const SizedBox(width: 8),
            const Spacer(),
            Text(
              note ?? '$count · $tag',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: AppTheme.mono(
                size: 10,
                color: noteColor ?? AppColors.textMuted,
              ),
            ),
          ],
        ),
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
          Icon(icon, color: color ?? AppColors.textFaint),
          const SizedBox(height: 8),
          Text(
            text,
            textAlign: TextAlign.center,
            style: const TextStyle(color: AppColors.textMuted),
          ),
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

/// A subtle project sub-header (the deck's `GENIO · 3` eyebrow) naming the
/// project a run of session tiles belongs to and how many it holds.
class _ProjectHeader extends StatelessWidget {
  final String name;
  final int count;
  const _ProjectHeader({required this.name, required this.count});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(14, 10, 12, 6),
      child: Text(
        '${name.toUpperCase()} · $count',
        style: AppTheme.eyebrow(),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
    );
  }
}

/// A dense MRU row for the Recent tab: the state glyph, the title, a mono
/// `state · project · server` subtitle, and a trailing PR badge + relative age.
/// Divider-ruled rather than carded, matching the deck's flat recents list.
class _RecentTile extends StatelessWidget {
  final CommanderStore store;
  final SessionInfo session;
  final bool selected;
  final VoidCallback onTap;

  const _RecentTile({
    required this.store,
    required this.session,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final descriptor = sessionDescriptor(
      session,
      store.agentStateFor(session.id),
    );
    final waiting = descriptor.color == AppColors.amber;
    final subtitle =
        '${descriptor.label} · ${session.projectName} · ${store.config.name}';
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 9),
          decoration: BoxDecoration(
            color: selected ? AppColors.surfaceSel : null,
            border: const Border(bottom: BorderSide(color: AppColors.divider)),
          ),
          child: Row(
            children: [
              SessionGlyph(descriptor),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      session.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                        color: AppColors.text,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      subtitle,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: AppTheme.mono(
                        size: 10,
                        color: waiting
                            ? AppColors.amberText
                            : AppColors.textMuted,
                      ),
                    ),
                  ],
                ),
              ),
              if (session.prNumber != null) ...[
                const SizedBox(width: 8),
                prChip(context, session.prNumber!, session.prState),
              ],
              const SizedBox(width: 8),
              Text(
                relativeAge(session.lastAttachedAt ?? session.createdAt),
                style: AppTheme.mono(size: 10, color: AppColors.textFaint),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A carded session row for the grouped All view: the state glyph, the title,
/// and a trailing PR badge (or the state word when there's no PR). Selected rows
/// tint accent; sessions waiting for input tint amber — both pull the eye to the
/// row that needs it.
class _GroupedTile extends StatelessWidget {
  final CommanderStore store;
  final SessionInfo session;
  final bool selected;
  final VoidCallback onTap;

  const _GroupedTile({
    required this.store,
    required this.session,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final descriptor = sessionDescriptor(
      session,
      store.agentStateFor(session.id),
    );
    final waiting = descriptor.color == AppColors.amber;
    final Color bg, borderColor;
    if (selected) {
      bg = AppColors.accent.withValues(alpha: 0.1);
      borderColor = AppColors.accent.withValues(alpha: 0.5);
    } else if (waiting) {
      bg = AppColors.amber.withValues(alpha: 0.09);
      borderColor = AppColors.amber.withValues(alpha: 0.45);
    } else {
      bg = AppColors.surface;
      borderColor = AppColors.border;
    }
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(10),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 9),
          decoration: BoxDecoration(
            color: bg,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(color: borderColor),
          ),
          child: Row(
            children: [
              SessionGlyph(descriptor, width: 12),
              const SizedBox(width: 9),
              Expanded(
                child: Text(
                  session.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w600,
                    color: AppColors.text,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              if (session.prNumber != null)
                prChip(context, session.prNumber!, session.prState)
              else
                Text(
                  descriptor.label,
                  style: AppTheme.mono(
                    size: 10,
                    color: waiting ? AppColors.amberText : AppColors.textMuted,
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}
