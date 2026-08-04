import 'package:flutter/material.dart';

import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../state/workspace_store.dart';
import '../theme/app_colors.dart';
import '../theme/app_theme.dart';
import '../util/activity_feed.dart';
import '../util/format.dart';
import 'terminal_page.dart';

/// The cross-server Activity timeline — layout-agnostic (no Scaffold, no route),
/// so it embeds in both the phone bottom-nav and the wide workspace pane. A
/// vertical rail runs down the left with a coloured node per event; actionable
/// "needs you" events float to the top as amber cards with an Answer button.
///
/// Reads the [WorkspaceStore] from the enclosing [WorkspaceScope] and rebuilds
/// off its change broadcast (it re-emits every child store's ticks), deriving the
/// feed via [buildActivityFeed]. Tapping an event with a session navigates to
/// that session's agent terminal, the same route the session list uses.
class ActivityBody extends StatefulWidget {
  /// Whether to render the in-body "Activity" title + subtitle. The phone
  /// bottom-nav and the wide workspace pane keep it (true); a push wrapper whose
  /// AppBar already titles the screen passes false.
  final bool showHeader;

  const ActivityBody({super.key, this.showHeader = true});

  @override
  State<ActivityBody> createState() => _ActivityBodyState();
}

class _ActivityBodyState extends State<ActivityBody> {
  ActivityFilter _filter = ActivityFilter.all;

  @override
  Widget build(BuildContext context) {
    final workspace = WorkspaceScope.of(context)!;
    return ListenableBuilder(
      listenable: workspace,
      builder: (context, _) {
        final servers = workspace.servers;
        final events = buildActivityFeed(servers);
        final filtered = filterActivity(events, _filter);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (widget.showHeader) _header(servers.length),
            _filterChips(needsYouCount(events)),
            Expanded(
              child: RefreshIndicator(
                onRefresh: workspace.refreshAll,
                child: _timeline(workspace, servers, filtered),
              ),
            ),
          ],
        );
      },
    );
  }

  Widget _header(int serverCount) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(18, 14, 18, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Activity',
            style: Theme.of(
              context,
            ).textTheme.titleLarge?.copyWith(fontSize: 24),
          ),
          const SizedBox(height: 2),
          Text(
            'across $serverCount server${serverCount == 1 ? '' : 's'} · live',
            style: AppTheme.mono(),
          ),
        ],
      ),
    );
  }

  Widget _filterChips(int needsYou) {
    return SizedBox(
      height: 46,
      child: ListView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 6),
        children: [
          _FilterChip(
            label: 'All',
            selected: _filter == ActivityFilter.all,
            onTap: () => setState(() => _filter = ActivityFilter.all),
          ),
          _FilterChip(
            label: 'Needs you · $needsYou',
            color: AppColors.amberText,
            selected: _filter == ActivityFilter.needsYou,
            onTap: () => setState(() => _filter = ActivityFilter.needsYou),
          ),
          _FilterChip(
            label: 'PRs',
            selected: _filter == ActivityFilter.prs,
            onTap: () => setState(() => _filter = ActivityFilter.prs),
          ),
        ],
      ),
    );
  }

  Widget _timeline(
    WorkspaceStore workspace,
    List<CommanderStore> servers,
    List<ActivityEvent> events,
  ) {
    if (events.isEmpty) {
      // A single scroll child keeps pull-to-refresh working over the empty note;
      // AlwaysScrollable lets the drag engage even when the note is short.
      return ListView(
        physics: const AlwaysScrollableScrollPhysics(),
        children: [_emptyState(servers)],
      );
    }
    return ListView(
      physics: const AlwaysScrollableScrollPhysics(),
      padding: const EdgeInsets.fromLTRB(26, 20, 20, 24),
      children: [
        Container(
          decoration: const BoxDecoration(
            border: Border(
              left: BorderSide(color: AppColors.borderSubtle, width: 2),
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final e in events)
                _TimelineItem(event: e, onTap: () => _open(workspace, e)),
            ],
          ),
        ),
      ],
    );
  }

  Widget _emptyState(List<CommanderStore> servers) {
    if (servers.isEmpty) {
      return const _InlineNote(icon: Icons.dns_outlined, text: 'No servers');
    }
    final loading = servers.any((s) => s.workspace == null && s.error == null);
    if (loading) {
      return const Padding(
        padding: EdgeInsets.symmetric(vertical: 40),
        child: Center(child: CircularProgressIndicator()),
      );
    }
    final message = switch (_filter) {
      ActivityFilter.needsYou => 'Nothing needs you',
      ActivityFilter.prs => 'No open pull requests',
      ActivityFilter.all => 'No recent activity',
    };
    return _InlineNote(icon: Icons.check_circle_outline, text: message);
  }

  /// Navigate to the event's session's agent terminal, mirroring the session
  /// list's route (a store-scoped [TerminalPage]). A no-op for server-level
  /// events (no session) or while the owning server is mid-reconnect (no handle).
  void _open(WorkspaceStore workspace, ActivityEvent event) {
    final sid = event.sessionId;
    if (sid == null) return;
    final store = workspace.serverById(event.serverId);
    final session = store?.sessionById(sid);
    final handle = store?.handle;
    if (store == null || session == null || handle == null) return;
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => CommanderStoreScope(
          store: store,
          child: TerminalPage(api: store.api, handle: handle, session: session),
        ),
      ),
    );
  }
}

/// The palette node colour for an event kind, matching the deck's timeline dots.
Color _nodeColor(ActivityKind kind) => switch (kind) {
  ActivityKind.needsYou => AppColors.amber,
  ActivityKind.paused => AppColors.amber,
  ActivityKind.working => AppColors.teal,
  ActivityKind.prReady => AppColors.green,
  ActivityKind.prMerged => AppColors.accentSoft,
  ActivityKind.pushed => AppColors.accentSoft,
  ActivityKind.finishedUnread => AppColors.accentSoft,
};

/// One row on the timeline: a coloured node sitting on the rail, then either an
/// amber "NEEDS YOU" card (actionable events) or a compact event row.
class _TimelineItem extends StatelessWidget {
  final ActivityEvent event;
  final VoidCallback onTap;

  const _TimelineItem({required this.event, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final actionable = event.actionable;
    final color = _nodeColor(event.kind);
    // Node radius + its vertical offset differ between the card and a plain row
    // so the dot lands on the title baseline in both.
    final radius = actionable ? 7.0 : 4.5;
    final top = actionable ? 4.0 : 5.0;
    return Padding(
      padding: const EdgeInsets.only(left: 20, bottom: 18),
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          actionable ? _needsYouCard(context) : _eventRow(context),
          Positioned(
            left: -20 - radius + 1,
            top: top,
            child: _node(color, radius, glow: actionable),
          ),
        ],
      ),
    );
  }

  Widget _node(Color color, double radius, {required bool glow}) => Container(
    width: radius * 2,
    height: radius * 2,
    decoration: BoxDecoration(
      color: color,
      shape: BoxShape.circle,
      boxShadow: glow
          ? [BoxShadow(color: color, blurRadius: 9, spreadRadius: 0.5)]
          : null,
    ),
  );

  Widget _needsYouCard(BuildContext context) {
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: event.sessionId == null ? null : onTap,
        borderRadius: BorderRadius.circular(12),
        child: Container(
          padding: const EdgeInsets.fromLTRB(14, 11, 12, 11),
          decoration: BoxDecoration(
            color: AppColors.amber.withValues(alpha: 0.09),
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: AppColors.amber.withValues(alpha: 0.4)),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Text(
                          event.kind == ActivityKind.paused
                              ? 'PAUSED'
                              : 'NEEDS YOU',
                          style: AppTheme.mono(
                            size: 9,
                            weight: FontWeight.w700,
                            color: AppColors.amberText,
                            letterSpacing: 0.8,
                          ),
                        ),
                        const SizedBox(width: 9),
                        Flexible(
                          child: Text(
                            event.title,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              fontSize: 14,
                              fontWeight: FontWeight.w600,
                              color: AppColors.text,
                            ),
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 3),
                    Text(
                      '${event.description} · ${event.serverName} / ${event.location}',
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: AppTheme.mono(size: 10.5),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 10),
              Column(
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  if (event.at != null)
                    Text(
                      relativeAge(event.at!),
                      style: AppTheme.mono(
                        size: 10,
                        color: AppColors.textFaint,
                      ),
                    ),
                  const SizedBox(height: 8),
                  if (event.sessionId != null) _answerButton(),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _answerButton() => Container(
    padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 7),
    decoration: BoxDecoration(
      color: AppColors.accent,
      borderRadius: BorderRadius.circular(9),
    ),
    child: const Text(
      'Answer ›',
      style: TextStyle(
        fontSize: 12,
        fontWeight: FontWeight.w700,
        color: AppColors.bg,
      ),
    ),
  );

  Widget _eventRow(BuildContext context) {
    final trailing = event.at == null
        ? event.location
        : '${relativeAge(event.at!)} · ${event.location}';
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: event.sessionId == null ? null : onTap,
        borderRadius: BorderRadius.circular(8),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 2),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      event.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                        color: AppColors.text,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      event.description,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: AppTheme.mono(size: 10.5),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 10),
              Padding(
                padding: const EdgeInsets.only(top: 1),
                child: Text(
                  trailing,
                  style: AppTheme.mono(size: 10, color: AppColors.textFaint),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A pill toggle for the filter row. Selected fills [AppColors.surfaceSel];
/// unselected is a bordered surface pill with muted (or [color]-tinted) text.
class _FilterChip extends StatelessWidget {
  final String label;
  final bool selected;
  final VoidCallback onTap;
  final Color? color;

  const _FilterChip({
    required this.label,
    required this.selected,
    required this.onTap,
    this.color,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(right: 7),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(20),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 13, vertical: 7),
            decoration: BoxDecoration(
              color: selected ? AppColors.surfaceSel : AppColors.surface,
              borderRadius: BorderRadius.circular(20),
              border: Border.all(
                color: selected ? AppColors.surfaceSel : AppColors.border,
              ),
            ),
            child: Text(
              label,
              style: AppTheme.mono(
                size: 10.5,
                weight: FontWeight.w600,
                color: selected
                    ? AppColors.text
                    : (color ?? AppColors.textMuted),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A centered icon + message for the empty / no-servers / no-matches states.
class _InlineNote extends StatelessWidget {
  final IconData icon;
  final String text;
  const _InlineNote({required this.icon, required this.text});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 48),
      child: Column(
        children: [
          Icon(icon, color: AppColors.textFaint),
          const SizedBox(height: 10),
          Text(
            text,
            textAlign: TextAlign.center,
            style: const TextStyle(color: AppColors.textMuted),
          ),
        ],
      ),
    );
  }
}
