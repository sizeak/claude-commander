import 'package:flutter/material.dart';

import '../chrome/chrome.dart';
import '../chrome/chrome_forms.dart';
import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../state/workspace_store.dart';
import '../theme/theme_controller.dart';
import '../theme/tokens.dart';
import 'session_list_page.dart';
import 'theme_picker_page.dart';

/// Whether a server's base URL points at the local machine, driving the
/// `local` / `remote` tag on its row. Compares the parsed [Uri.host] so a name
/// like `notlocalhost.example` isn't misread as local.
///
/// The session list's server-node header makes the same call from its own
/// private copy. Kept private on both sides deliberately: it is a one-line
/// display heuristic, not a wire contract, and exporting it would invite a
/// caller to treat it as one.
bool _isLocalServer(String baseUrl) {
  final host = Uri.tryParse(baseUrl)?.host.toLowerCase() ?? '';
  return host == 'localhost' ||
      host == '127.0.0.1' ||
      host == '::1' ||
      host == '[::1]';
}

/// The settings screen: the configured servers, the per-server workspace
/// editors, and the appearance controls.
///
/// Replaces the ⚙ popup menu that offered Servers / Projects / Programs as
/// three bare text items. A real screen is what lets each entry carry its own
/// state — a server's live connection dot and session count, the active theme's
/// name — which a dropdown of labels could not show at all.
///
/// Everything here is either device-local (the theme) or a route into an
/// existing manager page; the screen owns no state of its own.
class SettingsPage extends StatelessWidget {
  const SettingsPage({super.key});

  @override
  Widget build(BuildContext context) {
    final workspace = WorkspaceScope.of(context)!;
    return ChromePage(
      title: 'Settings',
      code: '47-X',
      // The workspace re-broadcasts every child store's notifications, so this
      // single listener also covers the per-server connection dots and counts.
      body: ListenableBuilder(
        listenable: workspace,
        builder: (context, _) => ListView(
          padding: const EdgeInsets.fromLTRB(14, 4, 14, 24),
          children: [
            // Passed already uppercase, as the chrome's own examples are: an
            // eyebrow is an uppercase form by definition, and this way it does
            // not depend on whether a chrome cases the label for us.
            const ChromeEyebrow('SERVERS'),
            ..._serverRows(context, workspace),
            const SizedBox(height: 16),
            const ChromeEyebrow('WORKSPACE'),
            ..._workspaceRows(context, workspace),
            const SizedBox(height: 16),
            const ChromeEyebrow('APPEARANCE'),
            const _ThemeRow(),
          ],
        ),
      ),
    );
  }

  /// One row per configured server, or a single row inviting the first one.
  /// Both open the servers manager, which is where add / edit / remove live.
  List<Widget> _serverRows(BuildContext context, WorkspaceStore workspace) {
    final servers = workspace.servers;
    if (servers.isEmpty) {
      return [
        _SettingsRow(
          label: 'No servers configured',
          caption: 'Add one to see its sessions',
          onTap: () => openServers(context, workspace),
        ),
      ];
    }
    return [
      for (final store in servers)
        _ServerRow(store: store, onTap: () => openServers(context, workspace)),
    ];
  }

  /// The per-server editors. Both need a live server handle to load anything,
  /// so they disable together — and say why, rather than being inertly greyed.
  List<Widget> _workspaceRows(BuildContext context, WorkspaceStore workspace) {
    // Matches the popup menu this screen replaces: one connected server is
    // enough, since the pickers prompt for which one to act on.
    final enabled = workspace.servers.any((s) => s.handle != null);
    const unavailable = 'Needs a connected server';
    return [
      _SettingsRow(
        label: 'Projects',
        caption: enabled ? 'Repositories and their branches' : unavailable,
        onTap: enabled ? () => openProjects(context, workspace) : null,
      ),
      _SettingsRow(
        label: 'Programs',
        caption: enabled
            ? 'Agents offered when creating a session'
            : unavailable,
        onTap: enabled ? () => openPrograms(context, workspace) : null,
      ),
    ];
  }
}

/// A server's row: a live connection dot, its name, and an `N · local/remote`
/// caption. Degraded servers keep their row (they must stay visible to be
/// fixed) but read as inert.
class _ServerRow extends StatelessWidget {
  final CommanderStore store;
  final VoidCallback onTap;

  const _ServerRow({required this.store, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final conn = store.connection;
    final (dotColor, degraded) = switch (conn.kind) {
      ConnectionStateKind.connected => (t.working, false),
      ConnectionStateKind.connecting => (t.attention, false),
      ConnectionStateKind.degraded => (t.idle, true),
    };
    final tag = _isLocalServer(store.config.baseUrl) ? 'local' : 'remote';
    return _SettingsRow(
      label: store.config.name,
      // A degraded server's session count is whatever was last seen, which
      // would read as live status it isn't — say it's unreachable instead, as
      // the session list's server header does.
      caption: degraded
          ? (conn.reason.isEmpty ? 'unreachable' : conn.reason)
          : '${store.sessions.length} · $tag',
      captionColor: degraded ? t.danger : null,
      accent: dotColor,
      leading: _ConnectionDot(color: dotColor, glow: !degraded),
      onTap: onTap,
    );
  }
}

/// The theme row, showing the active theme's name and opening the picker.
class _ThemeRow extends StatelessWidget {
  const _ThemeRow();

  @override
  Widget build(BuildContext context) {
    final controller = ThemeScope.of(context)!;
    // Listen so the caption follows a selection made in the picker: popping
    // back to a stale theme name would be the one place in the app where the
    // setting and its label disagree.
    return ListenableBuilder(
      listenable: controller,
      builder: (context, _) => _SettingsRow(
        label: 'Theme',
        caption: controller.id.label,
        onTap: () => Navigator.of(
          context,
        ).push(MaterialPageRoute(builder: (_) => const ThemePickerPage())),
      ),
    );
  }
}

/// A live connection dot. Glows while healthy, flat once the server is inert.
class _ConnectionDot extends StatelessWidget {
  final Color color;
  final bool glow;

  const _ConnectionDot({required this.color, required this.glow});

  @override
  Widget build(BuildContext context) => Container(
    width: 8,
    height: 8,
    decoration: BoxDecoration(
      color: color,
      shape: BoxShape.circle,
      boxShadow: glow ? [BoxShadow(color: color, blurRadius: 7)] : null,
    ),
  );
}

/// One tappable settings row: an optional leading indicator, a label, a mono
/// caption underneath, and a trailing chevron.
///
/// Built on [ChromePanel] rather than a hand-rolled `Container` so each theme
/// gives the row its own shape — a rounded bordered card under Mission Control,
/// a top-ruled hard-edged panel under LCARS — without this page branching on
/// which theme is active.
///
/// A null [onTap] renders disabled: dimmed text and no chevron, so the row
/// still explains itself (via [caption]) instead of silently ignoring taps.
class _SettingsRow extends StatelessWidget {
  final String label;
  final String caption;

  /// Overrides the caption colour, for a caption that is a warning rather than
  /// metadata.
  final Color? captionColor;

  /// Tints the panel's rule/border. Null uses the theme's neutral.
  final Color? accent;

  final Widget? leading;
  final VoidCallback? onTap;

  const _SettingsRow({
    required this.label,
    required this.caption,
    this.captionColor,
    this.accent,
    this.leading,
    this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final enabled = onTap != null;
    final captionTint = captionColor ?? (enabled ? t.textMuted : t.textDim);
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: ChromePanel(
        ChromePanelSpec(
          accent: accent,
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 11),
          onTap: onTap,
          child: Row(
            children: [
              if (leading case final indicator?) ...[
                indicator,
                const SizedBox(width: 11),
              ],
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      t.caseLabel(label),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontFamily: t.sans,
                        fontSize: 13.5,
                        fontWeight: FontWeight.w600,
                        letterSpacing: t.uppercaseLabels ? 0.6 : -0.1,
                        color: enabled ? t.text : t.textDim,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      caption,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: t.meta(size: 10, color: captionTint),
                    ),
                  ],
                ),
              ),
              if (enabled) ...[
                const SizedBox(width: 8),
                Icon(Icons.chevron_right, size: 18, color: t.textFaint),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
