import 'package:flutter/material.dart';

import '../chrome/chrome_forms.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../state/workspace_store.dart';
import '../theme/tokens.dart';
import '../util/activity_feed.dart';
import '../util/format.dart';
import 'terminal_page.dart';

/// The cross-server Activity timeline — layout-agnostic (no Scaffold, no route),
/// so it embeds in both the phone bottom-nav and the wide workspace pane. A
/// vertical rail runs down the left with a coloured node per event; actionable
/// "needs you" events float to the top as attention-tinted cards with an Answer
/// button.
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
            style: Theme.of(context).textTheme.titleLarge?.copyWith(fontSize: 24),
          ),
          const SizedBox(height: 2),
          Text(
            'across $serverCount server${serverCount == 1 ? '' : 's'} · live',
            style: CommanderTokens.of(context).meta(),
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
            color: CommanderTokens.of(context).attentionOn,
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
    final t = CommanderTokens.of(context);
    return ListView(
      physics: const AlwaysScrollableScrollPhysics(),
      padding: const EdgeInsets.fromLTRB(26, 20, 20, 24),
      children: [
        Container(
          decoration: BoxDecoration(
            border: Border(
              left: BorderSide(color: t.borderSubtle, width: 2),
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final e in events) _TimelineItem(event: e, onTap: () => _open(workspace, e)),
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
          child: TerminalPage(
            api: store.api,
            handle: handle,
            session: session,
          ),
        ),
      ),
    );
  }
}

/// The node colour for an event kind, matching the deck's timeline dots.
Color _nodeColor(BuildContext context, ActivityKind kind) {
  final t = CommanderTokens.of(context);
  return switch (kind) {
    ActivityKind.needsYou => t.attention,
    ActivityKind.paused => t.held,
    ActivityKind.working => t.working,
    ActivityKind.prReady => t.success,
    ActivityKind.prMerged => t.info,
    ActivityKind.pushed => t.info,
    ActivityKind.finishedUnread => t.info,
  };
}

/// One row on the timeline: a coloured node sitting on the rail, then either an
/// attention-tinted "NEEDS YOU" card (actionable events) or a compact event row.
class _TimelineItem extends StatelessWidget {
  final ActivityEvent event;
  final VoidCallback onTap;

  const _TimelineItem({required this.event, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final actionable = event.actionable;
    final color = _nodeColor(context, event.kind);
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

  /// The actionable event's card. Its tint is a [SessionTone], not a colour
  /// derived here, so it reads the same as the row of the session it is about:
  /// an amber-tinted rounded card in Mission Control, a salmon- (or, for a held
  /// cascade, tan-) top-bordered panel in LCARS.
  ///
  /// The "NEEDS YOU" / "PAUSED" label stays inline beside the title rather than
  /// becoming the panel's `eyebrow`: the eyebrow slot renders *above* the
  /// content (outside the box entirely in Mission Control), which would pull the
  /// label out of the title row it belongs to.
  Widget _needsYouCard(BuildContext context) {
    final t = CommanderTokens.of(context);
    // Paused is the held tone, which Mission Control paints identically to
    // waiting (both are its one amber) and LCARS distinguishes.
    final tone = event.kind == ActivityKind.paused
        ? SessionTone.held
        : SessionTone.waiting;
    final toneStyle = t.toneStyle(tone);
    return ChromePanel(
      ChromePanelSpec(
        tone: tone,
        padding: const EdgeInsets.fromLTRB(14, 11, 12, 11),
        onTap: event.sessionId == null ? null : onTap,
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
                        style: t.meta(
                          size: 9,
                          weight: FontWeight.w700,
                          color: toneStyle.onTint,
                          letterSpacing: 0.8,
                        ),
                      ),
                      const SizedBox(width: 9),
                      Flexible(
                        child: Text(
                          event.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w600,
                            color: t.text,
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
                    style: t.meta(size: 10.5),
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
                    style: t.meta(size: 10, color: t.textFaint),
                  ),
                const SizedBox(height: 8),
                if (event.sessionId != null) _answerButton(context),
              ],
            ),
          ],
        ),
      ),
    );
  }

  /// The card's inline call to action. Not a [ChromeButtonBar]: it is decoration
  /// rather than a control (the whole panel is the tap target), and Mission
  /// Control renders a bar cell as either an icon over a caption or a flat
  /// surface key pill — neither of which is this filled accent chip.
  Widget _answerButton(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 7),
      decoration: BoxDecoration(
        color: t.primary,
        borderRadius: BorderRadius.circular(9),
      ),
      child: Text(
        'Answer ›',
        style: TextStyle(
          fontSize: 12,
          fontWeight: FontWeight.w700,
          color: t.canvas,
        ),
      ),
    );
  }

  /// A non-actionable event: two lines of text and a trailing age, with no fill,
  /// border or radius of its own.
  ///
  /// Deliberately **not** a [ChromeListRow]. The chrome's two-line shape is a
  /// faithful copy of the fleet list's recents tile, which is a denser row than
  /// this one and is ruled with a bottom divider — adopting it would put a
  /// divider under every event on a timeline that is already delimited by its
  /// rail, shrink both lines by a point, and inset the text away from the node
  /// dot that is positioned against it. Being frameless, this row is already
  /// theme-neutral: every colour is a token and the face follows the theme.
  Widget _eventRow(BuildContext context) {
    final t = CommanderTokens.of(context);
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
                      style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                        color: t.text,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      event.description,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: t.meta(size: 10.5),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 10),
              Padding(
                padding: const EdgeInsets.only(top: 1),
                child: Text(
                  trailing,
                  style: t.meta(size: 10, color: t.textFaint),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A pill toggle for the filter row. Selected fills the selected-surface token;
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
    final t = CommanderTokens.of(context);
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
              color: selected ? t.surfaceSelected : t.surface,
              borderRadius: BorderRadius.circular(20),
              border: Border.all(
                color: selected ? t.surfaceSelected : t.border,
              ),
            ),
            child: Text(
              label,
              style: t.meta(
                size: 10.5,
                weight: FontWeight.w600,
                color: selected ? t.text : (color ?? t.textMuted),
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
    final t = CommanderTokens.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 48),
      child: Column(
        children: [
          Icon(icon, color: t.textFaint),
          const SizedBox(height: 10),
          Text(
            text,
            textAlign: TextAlign.center,
            style: TextStyle(color: t.textMuted),
          ),
        ],
      ),
    );
  }
}
