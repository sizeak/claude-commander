import 'dart:async';

import 'package:flutter/material.dart';

import '../server_config.dart';
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/simple.dart' as rust;
import '../widgets/session_chips.dart';
import 'review_page.dart';
import 'terminal_page.dart';

/// Detail view for a single session: live agent state, diff summary, and a
/// pane snapshot, plus kill/restart/delete lifecycle actions. Polls the detail
/// endpoint on a timer so the agent state and pane preview stay current. The
/// live attached terminal is Phase 3 — this view shows a static snapshot.
class SessionDetailPage extends StatefulWidget {
  final ServerConfig config;

  /// The session to show. The list already has this, so the page renders
  /// immediately and refines it with polled detail.
  final SessionInfo session;

  const SessionDetailPage({
    super.key,
    required this.config,
    required this.session,
  });

  @override
  State<SessionDetailPage> createState() => _SessionDetailPageState();
}

class _SessionDetailPageState extends State<SessionDetailPage> {
  static const _pollInterval = Duration(seconds: 2);

  Timer? _timer;
  SessionDetail? _detail;
  String? _error;

  /// Set while a lifecycle action is in flight, to disable the buttons and
  /// pause polling so a refresh doesn't race the mutation.
  bool _busy = false;

  String get _id => widget.session.id;

  @override
  void initState() {
    super.initState();
    _poll();
    _timer = Timer.periodic(_pollInterval, (_) => _poll());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _poll() async {
    if (_busy) return;
    try {
      final detail = await rust.getSessionDetail(
        baseUrl: widget.config.baseUrl,
        token: widget.config.token,
        query: _id,
        lines: 200,
      );
      if (!mounted) return;
      setState(() {
        _detail = detail;
        _error = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = e.toString());
    }
  }

  /// Run a lifecycle action with a confirm dialog, a busy guard, and a
  /// success/failure snackbar. `popOnSuccess` returns to the list (for delete).
  Future<void> _runAction({
    required String title,
    required String message,
    required String confirmLabel,
    required Color confirmColor,
    required Future<void> Function() action,
    required String successMessage,
    bool popOnSuccess = false,
  }) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(title),
        content: Text(message),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: confirmColor),
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text(confirmLabel),
          ),
        ],
      ),
    );
    if (confirmed != true) return;

    setState(() => _busy = true);
    try {
      await action();
      if (!mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(successMessage)));
      if (popOnSuccess) {
        Navigator.of(context).pop(true);
        return;
      }
      setState(() => _busy = false);
      await _poll();
    } catch (e) {
      if (!mounted) return;
      setState(() => _busy = false);
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('Failed: $e')));
    }
  }

  void _kill() => _runAction(
    title: 'Kill session?',
    message: 'Stops the running program. The worktree is kept.',
    confirmLabel: 'Kill',
    confirmColor: Colors.orange,
    successMessage: 'Session killed',
    action: () => rust.killSession(
      baseUrl: widget.config.baseUrl,
      token: widget.config.token,
      id: _id,
    ),
  );

  void _restart() => _runAction(
    title: 'Restart session?',
    message: 'Restarts the program in this session.',
    confirmLabel: 'Restart',
    confirmColor: Colors.teal,
    successMessage: 'Session restarted',
    action: () => rust.restartSession(
      baseUrl: widget.config.baseUrl,
      token: widget.config.token,
      id: _id,
    ),
  );

  void _delete() => _runAction(
    title: 'Delete session?',
    message: 'Removes the session, its branch, and its worktree. '
        'This cannot be undone.',
    confirmLabel: 'Delete',
    confirmColor: Colors.red,
    successMessage: 'Session deleted',
    popOnSuccess: true,
    action: () => rust.deleteSession(
      baseUrl: widget.config.baseUrl,
      token: widget.config.token,
      id: _id,
    ),
  );

  void _openTerminal(SessionInfo info) {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => TerminalPage(config: widget.config, session: info),
      ),
    );
  }

  void _openReview(SessionInfo info) {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => ReviewPage(config: widget.config, session: info),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    // Prefer the freshest info from polling, falling back to what the list gave us.
    final info = _detail?.info ?? widget.session;
    return Scaffold(
      appBar: AppBar(
        title: Text(info.title, overflow: TextOverflow.ellipsis),
        actions: [
          IconButton(
            onPressed: () => _openReview(info),
            icon: const Icon(Icons.rate_review),
            tooltip: 'Review changes',
          ),
          IconButton(
            onPressed: () => _openTerminal(info),
            icon: const Icon(Icons.terminal),
            tooltip: 'Open live terminal',
          ),
        ],
      ),
      body: RefreshIndicator(
        onRefresh: _poll,
        child: ListView(
          padding: const EdgeInsets.all(16),
          children: [
            _header(context, info),
            const SizedBox(height: 16),
            if (_error != null) _errorBanner(context, _error!),
            _detailSection(context),
            const SizedBox(height: 16),
            _paneSection(context),
            const SizedBox(height: 24),
            _actions(context, info),
          ],
        ),
      ),
    );
  }

  Widget _header(BuildContext context, SessionInfo info) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          '${info.projectName} · ${info.branch}',
          style: Theme.of(context).textTheme.bodyMedium,
        ),
        const SizedBox(height: 4),
        Text(
          info.program,
          style: Theme.of(context).textTheme.labelSmall,
        ),
        const SizedBox(height: 10),
        Wrap(
          spacing: 6,
          runSpacing: 4,
          children: [
            statusChip(context, info.status),
            if (_detail != null && info.status == SessionStatus.running)
              agentStateChip(context, _detail!.agentState),
            if (info.prNumber != null)
              prChip(context, info.prNumber!, info.prState),
          ],
        ),
      ],
    );
  }

  Widget _detailSection(BuildContext context) {
    final detail = _detail;
    final diffStat = detail?.diffStat;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Changes', style: Theme.of(context).textTheme.titleSmall),
            const SizedBox(height: 6),
            Text(
              diffStat == null || diffStat.isEmpty
                  ? 'No changes'
                  : diffStat,
              style: Theme.of(context).textTheme.bodySmall,
            ),
          ],
        ),
      ),
    );
  }

  Widget _paneSection(BuildContext context) {
    final pane = _detail?.paneContent;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Text(
                  'Terminal snapshot',
                  style: Theme.of(context).textTheme.titleSmall,
                ),
                const Spacer(),
                TextButton.icon(
                  onPressed: () => _openTerminal(_detail?.info ?? widget.session),
                  icon: const Icon(Icons.terminal, size: 16),
                  label: const Text('Live'),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Container(
              width: double.infinity,
              constraints: const BoxConstraints(maxHeight: 320),
              padding: const EdgeInsets.all(8),
              decoration: BoxDecoration(
                color: Colors.black,
                borderRadius: BorderRadius.circular(6),
              ),
              child: SingleChildScrollView(
                child: SelectableText(
                  (pane == null || pane.isEmpty) ? '(no output)' : pane,
                  style: const TextStyle(
                    fontFamily: 'monospace',
                    fontSize: 12,
                    color: Colors.greenAccent,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _actions(BuildContext context, SessionInfo info) {
    final running = info.status == SessionStatus.running;
    return Wrap(
      spacing: 12,
      runSpacing: 8,
      children: [
        FilledButton.tonalIcon(
          onPressed: _busy || !running ? null : _kill,
          icon: const Icon(Icons.stop),
          label: const Text('Kill'),
        ),
        FilledButton.tonalIcon(
          onPressed: _busy ? null : _restart,
          icon: const Icon(Icons.restart_alt),
          label: const Text('Restart'),
        ),
        FilledButton.tonalIcon(
          onPressed: _busy ? null : _delete,
          style: FilledButton.styleFrom(
            foregroundColor: Theme.of(context).colorScheme.error,
          ),
          icon: const Icon(Icons.delete_outline),
          label: const Text('Delete'),
        ),
      ],
    );
  }

  Widget _errorBanner(BuildContext context, String error) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 12),
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          color: Theme.of(context).colorScheme.errorContainer,
          borderRadius: BorderRadius.circular(6),
        ),
        child: Row(
          children: [
            Icon(
              Icons.warning_amber,
              size: 18,
              color: Theme.of(context).colorScheme.onErrorContainer,
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                error,
                style: TextStyle(
                  color: Theme.of(context).colorScheme.onErrorContainer,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
