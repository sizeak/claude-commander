import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../chrome/chrome.dart';
import '../chrome/chrome_forms.dart';
import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/review.dart' as rust;
import '../theme/tokens.dart';
import '../widgets/session_chips.dart';
import 'adaptive_shell.dart' show kWideBreakpoint;

/// Signature for staging a comment on a selected line range.
typedef _AddCommentFn =
    Future<void> Function({
      required String file,
      required String side,
      required int lineStart,
      required int lineEnd,
      required String snippet,
    });

/// Review/diff + comments view for a session, layout-agnostic (no Scaffold, no
/// route). Fetches the review snapshot (parsed unified diff, comments, reviewed
/// marks), lets the user browse files and hunks, select a line range and attach a
/// comment, then apply the staged comments back to the agent. Mirrors the TUI
/// review view, scoped to a first cut: binary files render as a placeholder,
/// reviewed marks are read-only.
///
/// Its own top action bar carries the diff summary + refresh + apply (rather than
/// a Scaffold app bar / FAB) so it drops cleanly into either the narrow
/// [ReviewPage] route or the wide shell's detail pane. Wide layouts (≥
/// [kWideBreakpoint]) render the deck's FILES CHANGED sidebar + diff pane split;
/// narrow layouts keep the expandable file-card flow.
class ReviewBody extends StatefulWidget {
  final CommanderApi api;
  final String handle;
  final SessionInfo session;

  const ReviewBody({
    super.key,
    required this.api,
    required this.handle,
    required this.session,
  });

  @override
  State<ReviewBody> createState() => _ReviewBodyState();
}

class _ReviewBodyState extends State<ReviewBody> {
  rust.ReviewSnapshotDto? _snapshot;
  String? _error;
  bool _loading = true;
  bool _busy = false;

  /// Index of the file shown in the wide-layout diff pane. Clamped against the
  /// current snapshot at render time so a shrinking file list can't dangle it.
  int _selectedFile = 0;

  /// Display paths currently marked reviewed; mutated optimistically by
  /// [_toggleReviewed] and re-synced from each snapshot.
  final Set<String> _reviewed = {};

  /// Display paths with an in-flight reviewed toggle, so rapid re-taps don't
  /// fire overlapping flips that desync the optimistic [_reviewed] set.
  final Set<String> _toggling = {};

  String get _id => widget.session.id;

  @override
  void initState() {
    super.initState();
    _open();
  }

  Future<void> _open() async {
    setState(() => _loading = true);
    try {
      final snap = await widget.api.openReview(
        handle: widget.handle,
        sessionId: _id,
      );
      if (!mounted) return;
      setState(() {
        _snapshot = snap;
        _reviewed
          ..clear()
          ..addAll(snap.reviewed);
        _error = null;
        _loading = false;
        // _open() is the "now idle" point for every action that ends by
        // re-opening (apply/delete/add comment), so clear the busy gate here —
        // otherwise the page's controls stay disabled after a successful action.
        _busy = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _loading = false;
        _busy = false;
      });
    }
  }

  /// Re-fetch only if the diff changed (204 → keep current snapshot).
  Future<void> _refresh() async {
    final prev = _snapshot;
    if (prev == null) return _open();
    setState(() => _busy = true);
    try {
      final snap = await widget.api.refreshReview(
        handle: widget.handle,
        sessionId: _id,
        prevHash: prev.contentHash,
      );
      if (!mounted) return;
      setState(() {
        if (snap != null) {
          _snapshot = snap;
          _reviewed
            ..clear()
            ..addAll(snap.reviewed);
        }
        _busy = false;
      });
      if (snap == null) {
        _snack('No changes since last refresh');
      }
    } catch (e) {
      if (!mounted) return;
      setState(() => _busy = false);
      _snack('Refresh failed: $e');
    }
  }

  Future<void> _deleteComment(String commentId) async {
    setState(() => _busy = true);
    try {
      await widget.api.deleteComment(
        handle: widget.handle,
        sessionId: _id,
        commentId: commentId,
      );
      await _open();
    } catch (e) {
      if (!mounted) return;
      setState(() => _busy = false);
      _snack('Delete failed: $e');
    }
  }

  Future<void> _apply() async {
    setState(() => _busy = true);
    try {
      final result = await widget.api.applyComments(
        handle: widget.handle,
        sessionId: _id,
      );
      if (!mounted) return;
      _snack(_applyMessage(result));
      // Re-open so statuses (staged → applied) refresh.
      await _open();
    } catch (e) {
      if (!mounted) return;
      setState(() => _busy = false);
      _snack('Apply failed: $e');
    }
  }

  Future<void> _toggleReviewed(String displayPath) async {
    if (_toggling.contains(displayPath)) return;
    setState(() => _toggling.add(displayPath));
    try {
      final nowReviewed = await widget.api.toggleFileReviewed(
        handle: widget.handle,
        sessionId: _id,
        displayPath: displayPath,
      );
      if (!mounted) return;
      setState(() {
        if (nowReviewed) {
          _reviewed.add(displayPath);
        } else {
          _reviewed.remove(displayPath);
        }
      });
    } catch (e) {
      _snack('Toggle reviewed failed: $e');
    } finally {
      if (mounted) setState(() => _toggling.remove(displayPath));
    }
  }

  Future<Uint8List> _loadBlob(String side, String path) => widget.api.fetchBlob(
    handle: widget.handle,
    sessionId: _id,
    side: side,
    path: path,
  );

  String _applyMessage(rust.ApplyResult r) => switch (r.kind) {
    rust.ApplyResultKind.nothing => 'Nothing to apply',
    rust.ApplyResultKind.blocked =>
      'Blocked: ${r.driftedIds.length} drifted comment(s) — review or delete them',
    rust.ApplyResultKind.applied => 'Applied ${r.count} comment(s)',
    rust.ApplyResultKind.deferred_ =>
      'Deferred ${r.count} comment(s) (agent busy — re-apply later)',
  };

  void _snack(String msg) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(msg)));
  }

  /// Open the line-comment dialog and stage the comment on success.
  Future<void> _addComment({
    required String file,
    required String side,
    required int lineStart,
    required int lineEnd,
    required String snippet,
  }) async {
    final text = await showDialog<String>(
      context: context,
      builder: (ctx) =>
          _CommentDialog(file: file, lineStart: lineStart, lineEnd: lineEnd),
    );
    if (text == null || text.trim().isEmpty) return;
    setState(() => _busy = true);
    try {
      await widget.api.createComment(
        handle: widget.handle,
        sessionId: _id,
        file: file,
        side: side,
        lineStart: lineStart,
        lineEnd: lineEnd,
        snippet: snippet,
        comment: text.trim(),
      );
      await _open();
    } catch (e) {
      if (!mounted) return;
      setState(() => _busy = false);
      _snack('Comment failed: $e');
    }
  }

  @override
  Widget build(BuildContext context) {
    final snap = _snapshot;
    final stagedCount =
        snap?.comments
            .where((c) => c.status == rust.ReviewCommentStatus.staged)
            .length ??
        0;
    return Column(
      children: [
        _actionBar(context, snap, stagedCount),
        Expanded(child: _body(context, snap)),
      ],
    );
  }

  /// Top action bar: the deck's `+adds −dels · N files` summary on the left, with
  /// refresh + apply on the right (in place of a Scaffold app bar / FAB), so the
  /// body composes into either layout.
  Widget _actionBar(
    BuildContext context,
    rust.ReviewSnapshotDto? snap,
    int stagedCount,
  ) {
    final t = CommanderTokens.of(context);
    return Container(
      decoration: BoxDecoration(
        color: t.canvas,
        border: Border(bottom: BorderSide(color: t.divider)),
      ),
      padding: const EdgeInsets.only(left: 16, right: 6, top: 8, bottom: 8),
      child: Row(
        children: [
          Expanded(child: _diffSummary(snap)),
          if (snap != null && stagedCount > 0)
            Padding(
              padding: const EdgeInsets.only(left: 8),
              child: FilledButton.icon(
                onPressed: _busy ? null : _apply,
                style: FilledButton.styleFrom(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 14,
                    vertical: 10,
                  ),
                ),
                icon: const Icon(Icons.send, size: 16),
                label: Text('Apply ($stagedCount)'),
              ),
            ),
          IconButton(
            visualDensity: VisualDensity.compact,
            color: t.textMuted,
            onPressed: (_busy || _loading) ? null : _refresh,
            icon: const Icon(Icons.refresh),
            tooltip: 'Refresh diff',
          ),
        ],
      ),
    );
  }

  /// The coloured `+adds −dels · N files` strip, summed across the snapshot.
  Widget _diffSummary(rust.ReviewSnapshotDto? snap) {
    final t = CommanderTokens.of(context);
    final files = snap?.files ?? const <rust.ReviewFileDto>[];
    if (files.isEmpty) {
      return Text('No changes', style: t.meta(color: t.textFaint));
    }
    final added = files.fold<int>(0, (n, f) => n + f.added);
    final removed = files.fold<int>(0, (n, f) => n + f.removed);
    final count = files.length;
    return Row(
      children: [
        Text('+$added', style: t.meta(color: t.success)),
        const SizedBox(width: 8),
        Text('−$removed', style: t.meta(color: t.danger)),
        Flexible(
          child: Text(
            ' · $count file${count == 1 ? '' : 's'}',
            overflow: TextOverflow.ellipsis,
            style: t.meta(color: t.textMuted),
          ),
        ),
      ],
    );
  }

  Widget _body(BuildContext context, rust.ReviewSnapshotDto? snap) {
    if (_loading) {
      return const Center(child: CircularProgressIndicator());
    }
    if (_error != null) {
      return _errorView(context, _error!);
    }
    if (snap == null) {
      return const Center(child: Text('No review data'));
    }
    if (snap.files.isEmpty && snap.comments.isEmpty) {
      final t = CommanderTokens.of(context);
      return RefreshIndicator(
        onRefresh: _open,
        child: ListView(
          children: [
            Padding(
              padding: const EdgeInsets.all(32),
              child: Text(
                'No changes against ${snap.base}.',
                textAlign: TextAlign.center,
                style: t.meta(color: t.textFaint, height: 1.5),
              ),
            ),
          ],
        ),
      );
    }
    return LayoutBuilder(
      builder: (context, constraints) {
        if (constraints.maxWidth >= kWideBreakpoint && snap.files.isNotEmpty) {
          return _wideBody(context, snap);
        }
        return _narrowBody(context, snap);
      },
    );
  }

  /// The phone flow: comments, then the expandable per-file rows.
  Widget _narrowBody(BuildContext context, rust.ReviewSnapshotDto snap) {
    final t = CommanderTokens.of(context);
    return RefreshIndicator(
      onRefresh: _open,
      child: ListView(
        padding: const EdgeInsets.all(12),
        children: [
          Text('Base: ${snap.base}', style: t.meta(color: t.textFaint)),
          const SizedBox(height: 10),
          if (snap.comments.isNotEmpty) ...[
            const ChromeEyebrow('Comments'),
            ...snap.comments.map((c) => _commentCard(context, c)),
            const SizedBox(height: 18),
          ],
          const ChromeEyebrow('Files changed'),
          // One flat run, so a row's position is its index in the whole list.
          for (final (i, f) in snap.files.indexed)
            _FileCard(
              file: f,
              index: i,
              count: snap.files.length,
              reviewed: _reviewed.contains(f.displayPath),
              onToggleReviewed: (_busy || _toggling.contains(f.displayPath))
                  ? null
                  : () => _toggleReviewed(f.displayPath),
              onLoadImage: _loadBlob,
              onAddComment: _busy ? null : _addComment,
            ),
        ],
      ),
    );
  }

  /// The deck's wide layout: FILES CHANGED sidebar + a diff pane for the selected
  /// file. Comments on the selected file sit above its diff; comments whose file
  /// isn't among the changed files (e.g. its diff was fully reverted) surface in
  /// an "Other comments" section so every comment — and its delete affordance —
  /// stays reachable and a drifted one can never silently block apply.
  Widget _wideBody(BuildContext context, rust.ReviewSnapshotDto snap) {
    final sel = _selectedFile.clamp(0, snap.files.length - 1);
    final file = snap.files[sel];
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _filesSidebar(context, snap, sel),
        VerticalDivider(width: 1, color: CommanderTokens.of(context).divider),
        Expanded(child: _diffPane(context, snap, file)),
      ],
    );
  }

  Widget _filesSidebar(
    BuildContext context,
    rust.ReviewSnapshotDto snap,
    int sel,
  ) {
    final t = CommanderTokens.of(context);
    return Container(
      width: 250,
      color: t.canvas,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const ChromeEyebrow('FILES CHANGED'),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.symmetric(horizontal: 10),
              children: [
                for (var i = 0; i < snap.files.length; i++)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 4),
                    child: _fileRow(
                      context,
                      file: snap.files[i],
                      index: i,
                      count: snap.files.length,
                      selected: i == sel,
                      reviewed: _reviewed.contains(snap.files[i].displayPath),
                      onSelect: () => setState(() => _selectedFile = i),
                      onToggleReviewed:
                          (_busy ||
                              _toggling.contains(snap.files[i].displayPath))
                          ? null
                          : () => _toggleReviewed(snap.files[i].displayPath),
                    ),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _diffPane(
    BuildContext context,
    rust.ReviewSnapshotDto snap,
    rust.ReviewFileDto file,
  ) {
    final t = CommanderTokens.of(context);
    // Only this file's comments belong above its diff; any comment whose file
    // isn't among the changed files would otherwise be unreachable on desktop,
    // so collect those into an "Other comments" section.
    final changedPaths = snap.files.map((f) => f.displayPath).toSet();
    final fileComments = snap.comments
        .where((c) => c.file == file.displayPath)
        .toList();
    final otherComments = snap.comments
        .where((c) => !changedPaths.contains(c.file))
        .toList();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // File header: path + per-file stats + the (single-mode) view label.
        Container(
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: t.divider)),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 11),
          child: Row(
            children: [
              Flexible(
                child: Text(
                  file.displayPath,
                  overflow: TextOverflow.ellipsis,
                  style: t.meta(
                    size: 12,
                    weight: FontWeight.w600,
                    color: t.text,
                  ),
                ),
              ),
              const SizedBox(width: 10),
              Text('+${file.added}', style: t.meta(color: t.success)),
              const SizedBox(width: 6),
              Text('−${file.removed}', style: t.meta(color: t.danger)),
              const Spacer(),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                decoration: BoxDecoration(
                  color: t.surface,
                  borderRadius: BorderRadius.circular(7),
                  border: Border.all(color: t.border),
                ),
                child: Text(
                  'unified',
                  style: t.meta(
                    size: 10,
                    weight: FontWeight.w600,
                    color: t.textMuted,
                  ),
                ),
              ),
            ],
          ),
        ),
        Expanded(
          child: ListView(
            children: [
              if (otherComments.isNotEmpty) ...[
                const ChromeEyebrow('OTHER COMMENTS'),
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 12),
                  child: Column(
                    children: otherComments
                        .map((c) => _commentCard(context, c))
                        .toList(),
                  ),
                ),
                const SizedBox(height: 8),
              ],
              if (fileComments.isNotEmpty) ...[
                const ChromeEyebrow('COMMENTS'),
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 12),
                  child: Column(
                    children: fileComments
                        .map((c) => _commentCard(context, c))
                        .toList(),
                  ),
                ),
                const SizedBox(height: 8),
              ],
              // TODO: _FileDiffBody eagerly builds every hunk of the selected
              // file up front; move to a lazy/sliver builder if large diffs
              // become a scroll-perf problem.
              _FileDiffBody(
                file: file,
                onLoadImage: _loadBlob,
                onAddComment: _busy ? null : _addComment,
              ),
            ],
          ),
        ),
      ],
    );
  }

  /// One staged/applied comment as a chrome panel: a rounded card in Mission
  /// Control (which is exactly what the `Card` this replaced rendered — the app's
  /// `cardTheme` is the same surface, radius and border the panel builds), a
  /// hard-cornered top-bordered block in LCARS.
  ///
  /// The `ListTile` stays *inside* the panel with the panel's own padding zeroed,
  /// so the content keeps its existing metrics rather than being re-laid-out by
  /// hand. Deliberately no [ChromePanelSpec.accent] from the comment's status:
  /// Mission Control renders an accent as a tinted border, which would change a
  /// card that is currently plain.
  Widget _commentCard(BuildContext context, rust.CommentDto c) {
    final t = CommanderTokens.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: ChromePanel(
        ChromePanelSpec(
          padding: EdgeInsets.zero,
          child: ListTile(
            title: Text(
              c.comment,
              style: Theme.of(context).textTheme.bodyMedium,
            ),
            subtitle: Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                '${c.file} · ${c.side == rust.ReviewCommentSide.old ? "old" : "new"} '
                'L${c.lineStart}'
                '${c.lineEnd != c.lineStart ? "-${c.lineEnd}" : ""}',
                style: t.meta(color: t.textFaint),
              ),
            ),
            leading: _commentStatusChip(context, c.status),
            trailing: IconButton(
              onPressed: _busy ? null : () => _deleteComment(c.id),
              icon: Icon(Icons.delete_outline, color: t.textMuted),
              tooltip: 'Delete comment',
            ),
            isThreeLine: false,
          ),
        ),
      ),
    );
  }

  Widget _commentStatusChip(BuildContext context, rust.ReviewCommentStatus s) {
    final t = CommanderTokens.of(context);
    final (label, color) = switch (s) {
      rust.ReviewCommentStatus.staged => ('staged', t.info),
      rust.ReviewCommentStatus.drifted => ('drifted', t.attention),
      rust.ReviewCommentStatus.applied => ('applied', t.success),
    };
    return AppChip(label: label, color: color);
  }

  Widget _errorView(BuildContext context, String error) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.warning_amber, color: CommanderTokens.of(context).danger),
            const SizedBox(height: 12),
            Text(
              error,
              textAlign: TextAlign.center,
              style: Theme.of(context).textTheme.bodyMedium,
            ),
            const SizedBox(height: 16),
            FilledButton.tonal(onPressed: _open, child: const Text('Retry')),
          ],
        ),
      ),
    );
  }
}

/// The phone (stacked-navigation) review screen: a page frame titled by the
/// session, wrapping a [ReviewBody] whose own action bar carries refresh + apply.
class ReviewPage extends StatelessWidget {
  final CommanderApi api;
  final String handle;
  final SessionInfo session;

  const ReviewPage({
    super.key,
    required this.api,
    required this.handle,
    required this.session,
  });

  @override
  Widget build(BuildContext context) {
    // No chrome actions: refresh + apply live in ReviewBody's own action bar so
    // they follow it into the wide shell's detail pane, which has no frame.
    return ChromePage(
      code: '47-R',
      title: 'Review · ${session.title}',
      body: ReviewBody(api: api, handle: handle, session: session),
    );
  }
}

/// The colour that codes a file's change status — success for an add, danger for
/// a delete, working for a modify, info for a rename — carried into a row by
/// [_statusDot], and approximated for LCARS by [_statusTone].
Color _statusColor(BuildContext context, rust.ReviewFileStatus status) {
  final t = CommanderTokens.of(context);
  return switch (status) {
    rust.ReviewFileStatus.added => t.success,
    rust.ReviewFileStatus.deleted => t.danger,
    rust.ReviewFileStatus.modified => t.working,
    rust.ReviewFileStatus.renamed => t.info,
  };
}

String _statusLabel(rust.ReviewFileStatus status) => switch (status) {
  rust.ReviewFileStatus.added => 'added',
  rust.ReviewFileStatus.deleted => 'deleted',
  rust.ReviewFileStatus.modified => 'modified',
  rust.ReviewFileStatus.renamed => 'renamed',
};

/// The tone a changed file's row paints with.
///
/// [ChromeListRowSpec] carries a [SessionTone] rather than a colour, so a file's
/// own [_statusColor] cannot reach LCARS' leading number block and 2px top
/// border: no tone's accent is `success` or `danger` in either theme. These are
/// the nearest available readings of each change kind. Mission Control is
/// unaffected by the choice — its divider-ruled row only consults the tone for an
/// attention state (none of these is one) and takes its change colour from
/// [_statusDot] in the glyph slot instead.
SessionTone _statusTone(rust.ReviewFileStatus status) => switch (status) {
  // Something new and live.
  rust.ReviewFileStatus.added => SessionTone.working,
  // Inert — the file is gone.
  rust.ReviewFileStatus.deleted => SessionTone.stopped,
  // Changed, and not yet seen.
  rust.ReviewFileStatus.modified => SessionTone.unread,
  // `creating`'s accent is `info` in both themes, which is exactly the colour
  // [_statusColor] gives a rename.
  rust.ReviewFileStatus.renamed => SessionTone.creating,
};

/// Where a row sits in a run of [count] rows. LCARS rounds a run's outer corners
/// so it reads as one bracketed cluster; Mission Control ignores it. Mirrors
/// `session_list_page.dart`'s helper of the same name.
ChromeRowPosition _rowPosition(int index, int count) {
  if (count == 1) return ChromeRowPosition.only;
  if (index == 0) return ChromeRowPosition.first;
  if (index == count - 1) return ChromeRowPosition.last;
  return ChromeRowPosition.middle;
}

/// The two-digit number LCARS prints in a file row's leading block.
///
/// Sequential rather than [lcarsRowNumber]: these rows are keyed on a file path,
/// not a session id, and the design's FILES CHANGED list numbers files in reading
/// order (`01`, `02`, …). The hash exists because a session list reorders on
/// activity and would renumber itself constantly; a file list holds its order for
/// as long as the snapshot does, so an index is stable here.
String _fileNumber(int index) => (index + 1).toString().padLeft(2, '0');

/// The change kind, for the row's subtitle. The line counts are *not* here: they
/// carry their own colours (see [_fileDelta]), and a subtitle is plain text the
/// chrome colours itself.
String _fileSubtitle(rust.ReviewFileDto file) => _statusLabel(file.status);

/// The line delta, added in success green and removed in danger red.
///
/// A widget rather than part of the subtitle string because that split is load
/// bearing — it is how the sidebar reads at a glance, and it is what the pre-chrome
/// `_FileRow` rendered. Composed into the row's trailing slot the same way
/// `_recentRow` pairs a PR badge with an age.
/// A zero count is omitted, so a pure deletion reads `−12` rather than
/// `+0 −12`. That matches the pre-chrome wide sidebar, which guarded each count
/// on `> 0`; the phone card always printed both, so the two disagreed and this
/// picks the quieter of the two behaviours for both.
Widget _fileDelta(BuildContext context, rust.ReviewFileDto file) {
  final t = CommanderTokens.of(context);
  return Row(
    mainAxisSize: MainAxisSize.min,
    children: [
      if (file.added > 0)
        Text('+${file.added}', style: t.meta(size: 10, color: t.success)),
      if (file.added > 0 && file.removed > 0) const SizedBox(width: 5),
      if (file.removed > 0)
        Text('−${file.removed}', style: t.meta(size: 10, color: t.danger)),
    ],
  );
}

/// A small square status swatch (the deck's rounded-1px chip in a file row).
Widget _statusDot(BuildContext context, rust.ReviewFileStatus status) =>
    Container(
      width: 7,
      height: 7,
      decoration: BoxDecoration(
        color: _statusColor(context, status),
        borderRadius: BorderRadius.circular(2),
      ),
    );

/// One row in the wide FILES CHANGED sidebar: the change-kind swatch, the path,
/// its line delta, and a reviewed toggle. Tapping the row selects the file for the
/// diff pane; the trailing check toggles its reviewed mark.
///
/// A chrome row, so LCARS renders it as a numbered leading block against a
/// top-bordered panel and Mission Control as its divider-ruled two-line row.
Widget _fileRow(
  BuildContext context, {
  required rust.ReviewFileDto file,
  required int index,
  required int count,
  required bool selected,
  required bool reviewed,
  required VoidCallback onSelect,
  required VoidCallback? onToggleReviewed,
}) {
  final t = CommanderTokens.of(context);
  return ChromeListRow(
    ChromeListRowSpec(
      title: file.displayPath,
      subtitle: _fileSubtitle(file),
      tone: _statusTone(file.status),
      glyph: _statusDot(context, file.status),
      number: _fileNumber(index),
      selected: selected,
      position: _rowPosition(index, count),
      onTap: onSelect,
      trailingWidget: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          _fileDelta(context, file),
          const SizedBox(width: 8),
          InkWell(
            onTap: onToggleReviewed,
            customBorder: const CircleBorder(),
            child: Padding(
              padding: const EdgeInsets.all(2),
              child: Icon(
                reviewed ? Icons.check_circle : Icons.radio_button_unchecked,
                size: 16,
                color: reviewed ? t.success : t.idle,
              ),
            ),
          ),
        ],
      ),
    ),
  );
}

/// One changed file in the phone flow: a chrome row (path, line delta + change
/// kind, a reviewed checkbox and an expand caret) over the file's unified diff or
/// binary placeholder once expanded.
///
/// Was a `Card` wrapping an `ExpansionTile`. A [ChromeListRow] has no children
/// slot — the two themes disagree about a row's shape, not about what hangs below
/// one — so the expanded/collapsed state is held here and the body rendered
/// beneath the row rather than inside it.
class _FileCard extends StatefulWidget {
  final rust.ReviewFileDto file;

  /// Position in the changed-files run: drives the row's LCARS number and which
  /// of its corners round.
  final int index;
  final int count;

  final bool reviewed;

  /// Toggle this file's reviewed mark; null while busy.
  final VoidCallback? onToggleReviewed;

  /// Fetch raw bytes for one side of a (binary) file: `(side, path) → bytes`.
  final Future<Uint8List> Function(String side, String path) onLoadImage;

  /// Stage a comment for a selected line range; null while busy.
  final _AddCommentFn? onAddComment;

  const _FileCard({
    required this.file,
    required this.index,
    required this.count,
    required this.reviewed,
    required this.onToggleReviewed,
    required this.onLoadImage,
    required this.onAddComment,
  });

  @override
  State<_FileCard> createState() => _FileCardState();
}

class _FileCardState extends State<_FileCard> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final file = widget.file;
    final onToggleReviewed = widget.onToggleReviewed;
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ChromeListRow(
            ChromeListRowSpec(
              title: file.displayPath,
              subtitle: _fileSubtitle(file),
              tone: _statusTone(file.status),
              glyph: _statusDot(context, file.status),
              number: _fileNumber(widget.index),
              position: _rowPosition(widget.index, widget.count),
              onTap: () => setState(() => _expanded = !_expanded),
              trailingWidget: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  _fileDelta(context, file),
                  const SizedBox(width: 4),
                  Checkbox(
                    visualDensity: VisualDensity.compact,
                    value: widget.reviewed,
                    onChanged: onToggleReviewed == null
                        ? null
                        : (_) => onToggleReviewed(),
                  ),
                  Icon(
                    _expanded ? Icons.expand_less : Icons.expand_more,
                    size: 18,
                    color: t.textMuted,
                  ),
                ],
              ),
            ),
          ),
          if (_expanded)
            _FileDiffBody(
              file: file,
              onLoadImage: widget.onLoadImage,
              onAddComment: widget.onAddComment,
            ),
        ],
      ),
    );
  }
}

/// The diff body of one file, shared by the phone card and the wide diff pane:
/// the unified hunks, or a binary/image placeholder.
class _FileDiffBody extends StatelessWidget {
  final rust.ReviewFileDto file;
  final Future<Uint8List> Function(String side, String path) onLoadImage;
  final _AddCommentFn? onAddComment;

  const _FileDiffBody({
    required this.file,
    required this.onLoadImage,
    required this.onAddComment,
  });

  /// Which side's blob to render: deletions only have an old side; everything
  /// else shows the new (working-tree) side.
  String get _imageSide =>
      file.status == rust.ReviewFileStatus.deleted ? 'old' : 'new';

  bool get _isImage =>
      file.isBinary && (file.binaryMime?.startsWith('image/') ?? false);

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    if (file.isBinary) {
      return Padding(
        padding: const EdgeInsets.all(16),
        child: _isImage
            ? _BinaryImageView(
                side: _imageSide,
                path: file.displayPath,
                mime: file.binaryMime!,
                load: onLoadImage,
              )
            : Text(
                file.binaryMime != null
                    ? 'Binary file (${file.binaryMime})'
                    : 'Binary file',
                style: t.meta(color: t.textFaint),
              ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final h in file.hunks)
          _HunkView(
            file: file.displayPath,
            hunk: h,
            onAddComment: onAddComment,
          ),
      ],
    );
  }
}

/// A single hunk rendered as a unified diff: a header line followed by
/// color-coded lines with a line-number gutter. Tapping a line selects it
/// (tap-and-hold extends to a range) and offers an "add comment" action.
class _HunkView extends StatefulWidget {
  final String file;
  final rust.ReviewHunkDto hunk;
  final _AddCommentFn? onAddComment;

  const _HunkView({
    required this.file,
    required this.hunk,
    required this.onAddComment,
  });

  @override
  State<_HunkView> createState() => _HunkViewState();
}

class _HunkViewState extends State<_HunkView> {
  /// Selected line indices into `hunk.lines` (a contiguous range once
  /// extended). Empty when nothing is selected.
  int? _anchor;
  int? _focus;

  bool _inSelection(int i) {
    if (_anchor == null || _focus == null) return false;
    final lo = _anchor! < _focus! ? _anchor! : _focus!;
    final hi = _anchor! > _focus! ? _anchor! : _focus!;
    return i >= lo && i <= hi;
  }

  void _tap(int i) {
    setState(() {
      _anchor = i;
      _focus = i;
    });
  }

  void _extend(int i) {
    setState(() {
      _anchor ??= i;
      _focus = i;
    });
  }

  void _clear() => setState(() {
    _anchor = null;
    _focus = null;
  });

  /// Resolve the selected range into a (side, lineStart, lineEnd, snippet)
  /// suitable for a comment. The side is taken from the selection's lines:
  /// deletions anchor on the old side, everything else on the new side. Lines
  /// without a number on the chosen side are skipped for the range bounds.
  Future<void> _comment() async {
    final cb = widget.onAddComment;
    if (cb == null || _anchor == null || _focus == null) return;
    final lo = _anchor! < _focus! ? _anchor! : _focus!;
    final hi = _anchor! > _focus! ? _anchor! : _focus!;
    final selected = widget.hunk.lines.sublist(lo, hi + 1);

    // Side: if every selected line is a deletion, comment on the old side;
    // otherwise the new side (additions/context live there).
    final allDeletions = selected.every(
      (l) => l.origin == rust.ReviewLineOrigin.deletion,
    );
    final side = allDeletions ? 'old' : 'new';

    // Keep only the lines that exist on the chosen side. Both the line-number
    // bounds AND the snippet must come from these — a mixed selection's snippet
    // must not carry the other side's text, or the server's reanchor (which
    // searches the chosen side only) can't find it and the comment drifts on
    // creation.
    final sideLines = selected
        .where((l) => (allDeletions ? l.oldLineno : l.newLineno) != null)
        .toList();
    if (sideLines.isEmpty) {
      _clear();
      return;
    }
    final numbers =
        sideLines
            .map((l) => (allDeletions ? l.oldLineno : l.newLineno)!)
            .toList()
          ..sort();
    final snippet = sideLines.map((l) => l.content).join('\n');

    await cb(
      file: widget.file,
      side: side,
      lineStart: numbers.first,
      lineEnd: numbers.last,
      snippet: snippet,
    );
    // The re-open after staging disposes this hunk's State mid-await; don't
    // touch State once we're gone.
    if (!mounted) return;
    _clear();
  }

  @override
  void didUpdateWidget(covariant _HunkView old) {
    super.didUpdateWidget(old);
    // Every snapshot rebuilds fresh hunk DTOs, so an identity change means this
    // positional State is now bound to a different hunk. Drop stale selection
    // indices that would otherwise range-error on / mis-highlight the new lines.
    if (!identical(widget.hunk, old.hunk)) {
      _anchor = null;
      _focus = null;
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final hunk = widget.hunk;
    final hasSelection = _anchor != null && _focus != null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        // Muted `@@ ... @@` hunk header, gutter-aligned with the diff rows.
        Container(
          width: double.infinity,
          color: t.diffGutterBg,
          child: Row(
            children: [
              const SizedBox(width: 46),
              Expanded(
                child: Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 14,
                    vertical: 4,
                  ),
                  child: Text(
                    '@@ -${hunk.oldStart},${hunk.oldLines} '
                    '+${hunk.newStart},${hunk.newLines} @@'
                    '${hunk.header.isNotEmpty ? " ${hunk.header}" : ""}',
                    style: t.meta(color: t.textDim, height: 1.6),
                  ),
                ),
              ),
            ],
          ),
        ),
        for (var i = 0; i < hunk.lines.length; i++)
          _DiffLineRow(
            line: hunk.lines[i],
            selected: _inSelection(i),
            onTap: widget.onAddComment == null ? null : () => _tap(i),
            onLongPress: widget.onAddComment == null ? null : () => _extend(i),
          ),
        if (hasSelection && widget.onAddComment != null)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
            child: Row(
              children: [
                TextButton.icon(
                  onPressed: _comment,
                  icon: const Icon(Icons.add_comment, size: 16),
                  label: const Text('Comment on selection'),
                ),
                TextButton(onPressed: _clear, child: const Text('Clear')),
              ],
            ),
          ),
      ],
    );
  }
}

/// One diff line: a line-number gutter + a colour-coded, monospace content row.
class _DiffLineRow extends StatelessWidget {
  final rust.ReviewLineDto line;
  final bool selected;
  final VoidCallback? onTap;
  final VoidCallback? onLongPress;

  const _DiffLineRow({
    required this.line,
    required this.selected,
    required this.onTap,
    required this.onLongPress,
  });

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final (bg, marker, fg) = switch (line.origin) {
      rust.ReviewLineOrigin.addition => (
        t.success.withValues(alpha: 0.09),
        '+',
        t.success,
      ),
      rust.ReviewLineOrigin.deletion => (
        t.danger.withValues(alpha: 0.09),
        '-',
        t.danger,
      ),
      rust.ReviewLineOrigin.context => (Colors.transparent, ' ', t.textBright),
    };
    // Unified view: a single number per row (new side, or old for deletions).
    final lineNo = line.newLineno ?? line.oldLineno;
    return InkWell(
      onTap: onTap,
      onLongPress: onLongPress,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _gutter(context, lineNo),
          Expanded(
            child: Container(
              color: selected ? t.attention.withValues(alpha: 0.22) : bg,
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 1),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SizedBox(
                    width: 10,
                    child: Text(
                      marker,
                      style: t.meta(size: 12, color: fg, height: 1.6),
                    ),
                  ),
                  Expanded(
                    child: Text(
                      line.content,
                      style: t.meta(size: 12, color: fg, height: 1.6),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _gutter(BuildContext context, int? n) {
    final t = CommanderTokens.of(context);
    return Container(
      width: 46,
      color: t.diffGutterBg,
      padding: const EdgeInsets.only(right: 12, top: 1, bottom: 1),
      alignment: Alignment.topRight,
      child: Text(
        n?.toString() ?? '',
        style: t.meta(size: 11, color: t.diffGutter, height: 1.6),
      ),
    );
  }
}

/// Dialog to capture a comment's text for a selected line range.
class _CommentDialog extends StatefulWidget {
  final String file;
  final int lineStart;
  final int lineEnd;

  const _CommentDialog({
    required this.file,
    required this.lineStart,
    required this.lineEnd,
  });

  @override
  State<_CommentDialog> createState() => _CommentDialogState();
}

class _CommentDialogState extends State<_CommentDialog> {
  final _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final range = widget.lineEnd != widget.lineStart
        ? 'L${widget.lineStart}-${widget.lineEnd}'
        : 'L${widget.lineStart}';
    return AlertDialog(
      title: const Text('Add comment'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('${widget.file} · $range', style: t.meta(color: t.textFaint)),
          const SizedBox(height: 12),
          TextField(
            controller: _controller,
            autofocus: true,
            maxLines: 4,
            decoration: const InputDecoration(
              hintText: 'Your comment…',
              border: OutlineInputBorder(),
            ),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(_controller.text),
          child: const Text('Add'),
        ),
      ],
    );
  }
}

/// Lazily fetches and renders a binary image blob — a tap to load, so opening a
/// diff with many images doesn't eagerly download them all.
class _BinaryImageView extends StatefulWidget {
  final String side;
  final String path;
  final String mime;
  final Future<Uint8List> Function(String side, String path) load;

  const _BinaryImageView({
    required this.side,
    required this.path,
    required this.mime,
    required this.load,
  });

  @override
  State<_BinaryImageView> createState() => _BinaryImageViewState();
}

class _BinaryImageViewState extends State<_BinaryImageView> {
  Uint8List? _bytes;
  bool _loading = false;
  String? _error;

  Future<void> _fetch() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final bytes = await widget.load(widget.side, widget.path);
      if (!mounted) return;
      setState(() {
        _bytes = bytes;
        _loading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _loading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    if (_bytes != null) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '${widget.mime} · ${widget.side} side',
            style: t.meta(color: t.textFaint),
          ),
          const SizedBox(height: 8),
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 320),
            child: Image.memory(
              _bytes!,
              fit: BoxFit.contain,
              errorBuilder: (_, _, _) => const Text('Could not decode image'),
            ),
          ),
        ],
      );
    }
    if (_loading) {
      return const Center(
        child: Padding(
          padding: EdgeInsets.all(8),
          child: CircularProgressIndicator(),
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (_error != null) ...[
          Text('Failed: $_error', style: TextStyle(color: t.danger)),
          const SizedBox(height: 8),
        ],
        OutlinedButton.icon(
          onPressed: _fetch,
          icon: const Icon(Icons.image, size: 16),
          label: Text('Load image (${widget.mime})'),
        ),
      ],
    );
  }
}
