import 'package:flutter/material.dart';

import '../chrome/chrome.dart';
import '../chrome/chrome_forms.dart';
import '../chrome/chrome_wide.dart';
import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../state/workspace_store.dart';
import '../theme/agent_glyphs.dart';
import '../theme/tokens.dart';
import '../util/session_filter.dart';
import '../util/viewport.dart';
import '../widgets/brand_mark.dart';
import '../widgets/session_chips.dart';
import 'activity_page.dart';
import 'phone_shell.dart';
import 'review_page.dart';
import 'session_detail_page.dart';
import 'session_list_page.dart';
import 'terminal_page.dart';

/// Logical width at or above which the app switches from the stacked phone
/// layout to the desktop/tablet rail + workspace layout — provided the viewport
/// is also tall enough. See [useWideLayout].
const double kWideBreakpoint = 900;

/// Whether [constraints] should get the rail + workspace layout rather than the
/// stacked [PhoneShell].
///
/// Width alone is not the question, and taking it as such put a phone on the
/// wrong shell: a Pixel 8a is 914 × 411dp held sideways, clearing
/// [kWideBreakpoint] by 14dp with barely 400dp of height. Measured on that
/// device, the wide layout spends 71dp on the workspace header and 40 on the
/// tab strip before the pane gets a row; it resizes for the soft keyboard
/// instead of panning, so the pane disappears entirely when the keyboard opens
/// (and the fleet column overflowed its box by 126px); and it suppresses the
/// on-screen modifier bar on the reasoning that a wide layout implies a
/// physical keyboard, leaving a phone with no Esc, Ctrl or arrows. The stacked
/// shell has none of those problems and gives the pane the full 914dp width.
///
/// The height comes from [isShortViewport] — `MediaQuery.sizeOf` — rather than
/// from [constraints], for two reasons, and *not* the tempting third one. It is
/// the same predicate the two chromes compact themselves on, so the shell and
/// the chrome inside it cannot disagree about whether the viewport is short. And
/// it is keyboard-invariant at any position in the tree, whereas [constraints]
/// are only keyboard-invariant *here*: nothing above this `LayoutBuilder`
/// consumes `viewInsets` today (`MaterialApp.builder` wraps the navigator in
/// [WindowFrame], a plain `Column`, and every `Scaffold` is built by a chrome
/// below the shells), so `constraints.maxHeight` measures 900 at a 900dp window
/// with a 600dp keyboard up — verified. Reading it would therefore work now and
/// silently start swapping the whole shell mid-keystroke the day an ancestor
/// that rewrites the height appears.
///
/// Crossing this threshold does swap shells, which tears down the routes and
/// the live attach beneath them — the same cost the 900dp width crossing has
/// always had. It is reachable by rotating a tablet or dragging a desktop
/// window's height past 500; the attach re-opens on the other side.
bool useWideLayout(BuildContext context, BoxConstraints constraints) =>
    constraints.maxWidth >= kWideBreakpoint && !isShortViewport(context);

/// Which surface the shell's FLEET/ACTIVITY toggle is driving the workspace to:
/// the selected session's [_DetailPane] (fleet), or the cross-server
/// [ActivityBody] feed (activity). The fleet list stays visible in both.
enum _RailMode { fleet, activity }

/// The responsive home. Where [useWideLayout] says no — a narrow viewport, or a
/// wide but short one such as a phone held sideways — it is the [PhoneShell] (a
/// bottom-nav Fleet + Activity shell over a stacked `Navigator.push` flow).
/// Otherwise [ChromeWide]: the fleet list, a **workspace** whose Overview /
/// Agent / Shell / Changes tabs switch in place, and the shell's navigation —
/// the FLEET/ACTIVITY toggle, the needs-input count, new-session and settings.
///
/// How many columns those become, and where the navigation sits, is the active
/// theme's business, not this page's: Mission Control renders two panes with the
/// nav in the rail's footer, while LCARS renders three above
/// [kLcarsThreeColumnWidth] with an elbow nav rail of its own. This page only
/// supplies the bodies and the live data.
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
      builder: (context, constraints) => useWideLayout(context, constraints)
          ? _wide(context)
          : const PhoneShell(),
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

        // A single cross-server pass powers the shell's counts: the fleet
        // header's totals and the nav's needs-input badge.
        var active = 0, total = 0, needsInput = 0;
        for (final s in workspace.servers) {
          for (final x in s.sessions) {
            total++;
            if (x.status.isActive) active++;
            // The same union the list rows' attention glyph uses — an agent
            // waiting for an answer, or a cascade paused mid-stack.
            if (sessionDescriptor(x, s.agentStateFor(x.id)).wantsAttention) {
              needsInput++;
            }
          }
        }

        return ChromeWide(
          ChromeWideSpec(
            fleetList: SessionListBody(
              selectedId: resolved?.id,
              onSelect: _select,
            ),
            workspace: _workspace(context, workspace, store, resolved),
            modes: [
              ChromeNavItem(
                label: 'FLEET',
                glyph: '▤',
                selected: _mode == _RailMode.fleet,
                onTap: () => _setMode(_RailMode.fleet),
              ),
              ChromeNavItem(
                label: 'ACTIVITY',
                glyph: '≋',
                selected: _mode == _RailMode.activity,
                onTap: () => _setMode(_RailMode.activity),
              ),
            ],
            // The LCARS frame's accent follows the active view, amber on Fleet
            // and lilac on Activity — the same split the phone frames make
            // through `ChromeViewRailSpec.style`.
            style: _mode == _RailMode.fleet
                ? ChromeViewRailStyle.branded
                : ChromeViewRailStyle.plain,
            needsInputCount: needsInput,
            activeCount: active,
            totalCount: total,
            serverCount: workspace.servers.length,
            newSession: ChromeButtonAction(
              icon: Icons.add,
              label: 'New session',
              kind: ChromeActionKind.primary,
              onPressed: () => openCreateSession(context, workspace),
            ),
            settings: ChromeButtonAction(
              icon: Icons.settings,
              label: 'Settings',
              onPressed: () => openSettings(context),
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
      // (back to the Agent tab + tears down the outgoing session's attach).
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

/// Placeholder shown in the workspace when no session is selected.
class _EmptyDetail extends StatelessWidget {
  const _EmptyDetail();

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Container(
      color: t.canvas,
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
            style: t.meta(size: 11, color: t.textFaint),
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

/// The wide layout's workspace: a header (state glyph + title + PR badge + meta
/// + refresh), the tab strip, and the active tab's body switched in place (no
/// route push). [ChromeWideDetail] frames all three — the tab strip is a
/// horizontal underline row in Mission Control and a column of elbow blocks in
/// LCARS — so this only says what the tabs are and what each one shows.
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
  /// Selecting a session lands on the live agent pane — the thing you almost
  /// always came to look at. Overview is one tab away for the metadata.
  _DetailTab _tab = _DetailTab.terminal;

  void _go(_DetailTab tab) => setState(() => _tab = tab);

  @override
  Widget build(BuildContext context) {
    final session = widget.session;
    final store = CommanderStoreScope.of(context);
    final descriptor = sessionDescriptor(
      session,
      store?.agentStateFor(session.id) ?? AgentState.unknown,
    );
    final serverName = store?.config.name;
    return ChromeWideDetail(
      ChromeWideDetailSpec(
        glyph: SessionGlyph(descriptor, size: 12),
        title: session.title,
        meta: [
          session.projectName,
          session.branch,
          session.program,
          ?serverName,
        ].join(' · '),
        badge: session.prNumber == null
            ? null
            : prChip(context, session.prNumber!, session.prState),
        refresh: ChromeButtonAction(
          icon: Icons.refresh,
          label: 'Refresh',
          onPressed: () => widget.onRefresh(),
        ),
        tabs: [
          for (final tab in _DetailTab.values)
            ChromeWideTab(
              tabKey: ValueKey('ws-tab-${tab.name}'),
              label: tab.label,
            ),
        ],
        selected: _tab.index,
        onSelect: (i) => _go(_DetailTab.values[i]),
        content: _content(context),
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
