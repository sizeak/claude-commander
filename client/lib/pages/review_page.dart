import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../services/commander_api.dart';
import '../src/rust/api/diff.dart';
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/review.dart' as rust;
import '../util/file_tree.dart';
import '../theme/app_colors.dart';
import '../theme/app_theme.dart';
import '../widgets/diff_view.dart';
import '../widgets/session_chips.dart';
import 'adaptive_shell.dart' show kWideBreakpoint;

/// Review/diff + comments view for a session, layout-agnostic (no Scaffold, no
/// route). Fetches the review snapshot, lets the user browse files, select a
/// line range and attach a comment, then apply the staged comments back to the
/// agent.
///
/// The diff itself is laid out by `diffgrid` in the cdylib — the same engine the
/// TUI renders through — so word-diff emphasis, side-by-side and expandable
/// context are the same decisions in both frontends; see [DiffView]. Binary
/// files still render as an image or a placeholder rather than a diff.
///
/// Its own top action bar carries the diff summary + refresh + apply (rather than
/// a Scaffold app bar / FAB) so it drops cleanly into either the narrow
/// [ReviewPage] route or the wide shell's detail pane. Wide layouts (≥
/// [kWideBreakpoint]) render the FILES CHANGED tree + diff pane split, and are
/// the only place side by side is offered — a phone has room for one code
/// column, so narrow layouts keep the unified, expandable file-card flow.
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

  /// Two-column diff, offered only in the wide layout: side by side needs room
  /// for two full code columns, and a phone has room for one.
  bool _sideBySide = false;

  /// Directory paths the user has collapsed in the files tree. Keyed by the
  /// node's full path, so a collapse survives a refresh that reorders files.
  final Set<String> _collapsedDirs = {};

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

  /// The working-tree text of a file, so the diff view can reveal the context
  /// its hunks elide. Malformed bytes are replaced rather than thrown on: a file
  /// that isn't quite UTF-8 should still expand, not break the view.
  Future<String> _loadText(String path) async =>
      utf8.decode(await _loadBlob('new', path), allowMalformed: true);

  /// Unified / split, offered only where there is room for two code columns.
  Widget _viewModeToggle() {
    Widget option(String label, bool split) {
      final on = _sideBySide == split;
      return InkWell(
        onTap: on ? null : () => setState(() => _sideBySide = split),
        borderRadius: BorderRadius.circular(6),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
          decoration: BoxDecoration(
            color: on ? AppColors.surfaceSel : Colors.transparent,
            borderRadius: BorderRadius.circular(6),
          ),
          child: Text(
            label,
            style: AppTheme.mono(
              size: 10,
              weight: FontWeight.w600,
              color: on ? AppColors.text : AppColors.textMuted,
            ),
          ),
        ),
      );
    }

    return Container(
      padding: const EdgeInsets.all(2),
      decoration: BoxDecoration(
        color: AppColors.surface,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: AppColors.border),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [option('unified', false), option('split', true)],
      ),
    );
  }

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
    return Container(
      decoration: const BoxDecoration(
        color: AppColors.bg,
        border: Border(bottom: BorderSide(color: AppColors.divider)),
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
            color: AppColors.textMuted,
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
    final files = snap?.files ?? const <rust.ReviewFileDto>[];
    if (files.isEmpty) {
      return Text(
        'No changes',
        style: AppTheme.mono(color: AppColors.textFaint),
      );
    }
    final added = files.fold<int>(0, (n, f) => n + f.added);
    final removed = files.fold<int>(0, (n, f) => n + f.removed);
    final count = files.length;
    return Row(
      children: [
        Text('+$added', style: AppTheme.mono(color: AppColors.green)),
        const SizedBox(width: 8),
        Text('−$removed', style: AppTheme.mono(color: AppColors.red)),
        Flexible(
          child: Text(
            ' · $count file${count == 1 ? '' : 's'}',
            overflow: TextOverflow.ellipsis,
            style: AppTheme.mono(color: AppColors.textMuted),
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
      return RefreshIndicator(
        onRefresh: _open,
        child: ListView(
          children: [
            Padding(
              padding: const EdgeInsets.all(32),
              child: Text(
                'No changes against ${snap.base}.',
                textAlign: TextAlign.center,
                style: AppTheme.mono(color: AppColors.textFaint, height: 1.5),
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

  /// The phone flow: comments, then the expandable per-file cards.
  Widget _narrowBody(BuildContext context, rust.ReviewSnapshotDto snap) {
    return RefreshIndicator(
      onRefresh: _open,
      child: ListView(
        padding: const EdgeInsets.all(12),
        children: [
          Text(
            'Base: ${snap.base}',
            style: AppTheme.mono(color: AppColors.textFaint),
          ),
          const SizedBox(height: 10),
          if (snap.comments.isNotEmpty) ...[
            Text('Comments', style: AppTheme.eyebrow()),
            const SizedBox(height: 8),
            ...snap.comments.map((c) => _commentCard(context, c)),
            const SizedBox(height: 18),
          ],
          Text('Files changed', style: AppTheme.eyebrow()),
          const SizedBox(height: 8),
          ...snap.files.map(
            (f) => _FileCard(
              api: widget.api,
              raw: snap.raw,
              file: f,
              reviewed: _reviewed.contains(f.displayPath),
              onToggleReviewed: (_busy || _toggling.contains(f.displayPath))
                  ? null
                  : () => _toggleReviewed(f.displayPath),
              onLoadImage: _loadBlob,
              onLoadText: _loadText,
              onAddComment: _busy ? null : _addComment,
            ),
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
        const VerticalDivider(width: 1, color: AppColors.divider),
        Expanded(child: _diffPane(context, snap, file)),
      ],
    );
  }

  Widget _filesSidebar(
    BuildContext context,
    rust.ReviewSnapshotDto snap,
    int sel,
  ) {
    return Container(
      width: 250,
      color: AppColors.bg,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 14, 16, 10),
            child: Text('FILES CHANGED', style: AppTheme.eyebrow()),
          ),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.symmetric(horizontal: 10),
              children: _fileTreeRows(snap, sel),
            ),
          ),
        ],
      ),
    );
  }

  /// The sidebar's rows: a compressed directory tree over the changed paths, so
  /// a change spanning several directories reads as structure rather than as a
  /// column of near-identical full paths.
  List<Widget> _fileTreeRows(rust.ReviewSnapshotDto snap, int sel) {
    final tree = buildFileTree([for (final f in snap.files) f.displayPath]);
    return [
      for (final row in flattenFileTree(tree, _collapsedDirs))
        switch (row) {
          FileTreeDirRow() => _DirRow(
            row: row,
            onToggle: () => setState(() {
              // `remove` reports whether it was there, so this is one lookup.
              if (!_collapsedDirs.remove(row.path)) {
                _collapsedDirs.add(row.path);
              }
            }),
          ),
          FileTreeFileRow() => _FileRow(
            file: snap.files[row.index],
            name: row.name,
            depth: row.depth,
            selected: row.index == sel,
            reviewed: _reviewed.contains(snap.files[row.index].displayPath),
            onSelect: () => setState(() => _selectedFile = row.index),
            onToggleReviewed:
                (_busy || _toggling.contains(snap.files[row.index].displayPath))
                ? null
                : () => _toggleReviewed(snap.files[row.index].displayPath),
          ),
        },
    ];
  }

  Widget _diffPane(
    BuildContext context,
    rust.ReviewSnapshotDto snap,
    rust.ReviewFileDto file,
  ) {
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
          decoration: const BoxDecoration(
            border: Border(bottom: BorderSide(color: AppColors.divider)),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 11),
          child: Row(
            children: [
              Flexible(
                child: Text(
                  file.displayPath,
                  overflow: TextOverflow.ellipsis,
                  style: AppTheme.mono(
                    size: 12,
                    weight: FontWeight.w600,
                    color: AppColors.text,
                  ),
                ),
              ),
              const SizedBox(width: 10),
              Text(
                '+${file.added}',
                style: AppTheme.mono(color: AppColors.green),
              ),
              const SizedBox(width: 6),
              Text(
                '−${file.removed}',
                style: AppTheme.mono(color: AppColors.red),
              ),
              const Spacer(),
              _viewModeToggle(),
            ],
          ),
        ),
        Expanded(
          child: ListView(
            children: [
              if (otherComments.isNotEmpty) ...[
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 14, 16, 8),
                  child: Text('OTHER COMMENTS', style: AppTheme.eyebrow()),
                ),
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
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 14, 16, 8),
                  child: Text('COMMENTS', style: AppTheme.eyebrow()),
                ),
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
                api: widget.api,
                raw: snap.raw,
                file: file,
                sideBySide: _sideBySide,
                dualGutter: true,
                onLoadImage: _loadBlob,
                onLoadText: _loadText,
                onAddComment: _busy ? null : _addComment,
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _commentCard(BuildContext context, rust.CommentDto c) {
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      child: ListTile(
        title: Text(c.comment, style: Theme.of(context).textTheme.bodyMedium),
        subtitle: Padding(
          padding: const EdgeInsets.only(top: 2),
          child: Text(
            '${c.file} · ${c.side == rust.ReviewCommentSide.old ? "old" : "new"} '
            'L${c.lineStart}'
            '${c.lineEnd != c.lineStart ? "-${c.lineEnd}" : ""}',
            style: AppTheme.mono(color: AppColors.textFaint),
          ),
        ),
        leading: _commentStatusChip(context, c.status),
        trailing: IconButton(
          onPressed: _busy ? null : () => _deleteComment(c.id),
          icon: const Icon(Icons.delete_outline, color: AppColors.textMuted),
          tooltip: 'Delete comment',
        ),
        isThreeLine: false,
      ),
    );
  }

  Widget _commentStatusChip(BuildContext context, rust.ReviewCommentStatus s) {
    final (label, color) = switch (s) {
      rust.ReviewCommentStatus.staged => ('staged', AppColors.accentSoft),
      rust.ReviewCommentStatus.drifted => ('drifted', AppColors.amber),
      rust.ReviewCommentStatus.applied => ('applied', AppColors.green),
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
            const Icon(Icons.warning_amber, color: AppColors.red),
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

/// The phone (stacked-navigation) review screen: a Scaffold titled by the
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
    return Scaffold(
      appBar: AppBar(
        title: Text(
          'Review · ${session.title}',
          overflow: TextOverflow.ellipsis,
        ),
      ),
      body: ReviewBody(api: api, handle: handle, session: session),
    );
  }
}

/// The colour that codes a file's change status — a green add, red delete, teal
/// modify, violet rename — shared by the status dot and the status chip.
Color _statusColor(rust.ReviewFileStatus status) => switch (status) {
  rust.ReviewFileStatus.added => AppColors.green,
  rust.ReviewFileStatus.deleted => AppColors.red,
  rust.ReviewFileStatus.modified => AppColors.teal,
  rust.ReviewFileStatus.renamed => AppColors.accentSoft,
};

String _statusLabel(rust.ReviewFileStatus status) => switch (status) {
  rust.ReviewFileStatus.added => 'added',
  rust.ReviewFileStatus.deleted => 'deleted',
  rust.ReviewFileStatus.modified => 'modified',
  rust.ReviewFileStatus.renamed => 'renamed',
};

/// A small square status swatch (the deck's rounded-1px chip in a file row).
Widget _statusDot(rust.ReviewFileStatus status) => Container(
  width: 7,
  height: 7,
  decoration: BoxDecoration(
    color: _statusColor(status),
    borderRadius: BorderRadius.circular(2),
  ),
);

/// A directory row in the FILES CHANGED tree. Tapping it collapses or expands
/// its subtree; a compressed chain shows as one `a/b/c` label.
class _DirRow extends StatelessWidget {
  final FileTreeDirRow row;
  final VoidCallback onToggle;

  const _DirRow({required this.row, required this.onToggle});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: EdgeInsets.only(left: row.depth * 12.0, bottom: 2),
      child: InkWell(
        onTap: onToggle,
        borderRadius: BorderRadius.circular(8),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 5),
          child: Row(
            children: [
              Icon(
                row.collapsed
                    ? Icons.keyboard_arrow_right
                    : Icons.keyboard_arrow_down,
                size: 14,
                color: AppColors.textFaint,
              ),
              const SizedBox(width: 3),
              Expanded(
                child: Text(
                  row.name,
                  overflow: TextOverflow.ellipsis,
                  style: AppTheme.mono(color: AppColors.textMuted),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// One file row in the wide FILES CHANGED tree: a status swatch, the file's own
/// segment (the directories above it already say where it lives), per-file +/−
/// counts, and a reviewed toggle. Tapping the row selects the file for the diff
/// pane; the trailing check toggles its reviewed mark.
class _FileRow extends StatelessWidget {
  final rust.ReviewFileDto file;

  /// The leaf segment to label the row with.
  final String name;

  /// Tree depth, for the indent.
  final int depth;
  final bool selected;
  final bool reviewed;
  final VoidCallback onSelect;
  final VoidCallback? onToggleReviewed;

  const _FileRow({
    required this.file,
    required this.name,
    required this.depth,
    required this.selected,
    required this.reviewed,
    required this.onSelect,
    required this.onToggleReviewed,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: EdgeInsets.only(left: depth * 12.0, bottom: 4),
      child: Material(
        color: selected ? AppColors.surface : Colors.transparent,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(8),
          side: BorderSide(
            color: selected ? AppColors.border : Colors.transparent,
          ),
        ),
        child: InkWell(
          onTap: onSelect,
          borderRadius: BorderRadius.circular(8),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 8),
            child: Row(
              children: [
                _statusDot(file.status),
                const SizedBox(width: 8),
                Expanded(
                  // The full path is still one hover away, since the tree only
                  // shows the leaf.
                  child: Tooltip(
                    message: file.displayPath,
                    child: Text(
                      name,
                      overflow: TextOverflow.ellipsis,
                      style: AppTheme.mono(
                        color: selected ? AppColors.text : AppColors.textBright,
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                if (file.added > 0)
                  Text(
                    '+${file.added}',
                    style: AppTheme.mono(color: AppColors.green),
                  ),
                if (file.removed > 0) ...[
                  const SizedBox(width: 5),
                  Text(
                    '−${file.removed}',
                    style: AppTheme.mono(color: AppColors.red),
                  ),
                ],
                const SizedBox(width: 4),
                InkWell(
                  onTap: onToggleReviewed,
                  customBorder: const CircleBorder(),
                  child: Padding(
                    padding: const EdgeInsets.all(2),
                    child: Icon(
                      reviewed
                          ? Icons.check_circle
                          : Icons.radio_button_unchecked,
                      size: 16,
                      color: reviewed ? AppColors.green : AppColors.idle,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// One changed file as an expandable card (phone layout): header (path + stats +
/// status), and either a unified-diff body or a binary placeholder.
class _FileCard extends StatelessWidget {
  final CommanderApi api;

  /// The snapshot's raw unified diff, laid out in the cdylib. `null` against a
  /// server that predates the field.
  final String? raw;
  final rust.ReviewFileDto file;
  final bool reviewed;

  /// Toggle this file's reviewed mark; null while busy.
  final VoidCallback? onToggleReviewed;

  /// Fetch raw bytes for one side of a (binary) file: `(side, path) → bytes`.
  final Future<Uint8List> Function(String side, String path) onLoadImage;

  /// Fetch the working-tree text of a file, for context expansion.
  final Future<String> Function(String path) onLoadText;

  /// Stage a comment for a selected line range; null while busy.
  final AddCommentFn? onAddComment;

  const _FileCard({
    required this.api,
    required this.raw,
    required this.file,
    required this.reviewed,
    required this.onToggleReviewed,
    required this.onLoadImage,
    required this.onLoadText,
    required this.onAddComment,
  });

  @override
  Widget build(BuildContext context) {
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      clipBehavior: Clip.antiAlias,
      child: ExpansionTile(
        shape: const Border(),
        collapsedShape: const Border(),
        leading: Checkbox(
          value: reviewed,
          onChanged: onToggleReviewed == null
              ? null
              : (_) => onToggleReviewed!(),
        ),
        title: Text(
          file.displayPath,
          style: AppTheme.mono(size: 13, color: AppColors.text),
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: Padding(
          padding: const EdgeInsets.only(top: 4),
          child: Row(
            children: [
              AppChip(
                label: _statusLabel(file.status),
                color: _statusColor(file.status),
              ),
              const SizedBox(width: 8),
              Text(
                '+${file.added}',
                style: AppTheme.mono(color: AppColors.green),
              ),
              const SizedBox(width: 6),
              Text(
                '−${file.removed}',
                style: AppTheme.mono(color: AppColors.red),
              ),
            ],
          ),
        ),
        children: [
          _FileDiffBody(
            api: api,
            raw: raw,
            file: file,
            onLoadImage: onLoadImage,
            onLoadText: onLoadText,
            onAddComment: onAddComment,
          ),
        ],
      ),
    );
  }
}

/// The diff body of one file, shared by the phone card and the wide diff pane:
/// the laid-out hunks, or a binary/image placeholder.
class _FileDiffBody extends StatelessWidget {
  final CommanderApi api;
  final String? raw;
  final rust.ReviewFileDto file;
  final bool sideBySide;
  final bool dualGutter;
  final Future<Uint8List> Function(String side, String path) onLoadImage;
  final Future<String> Function(String path) onLoadText;
  final AddCommentFn? onAddComment;

  const _FileDiffBody({
    required this.api,
    required this.raw,
    required this.file,
    required this.onLoadImage,
    required this.onLoadText,
    required this.onAddComment,
    this.sideBySide = false,
    this.dualGutter = false,
  });

  /// Which side's blob to render: deletions only have an old side; everything
  /// else shows the new (working-tree) side.
  String get _imageSide =>
      file.status == rust.ReviewFileStatus.deleted ? 'old' : 'new';

  bool get _isImage =>
      file.isBinary && (file.binaryMime?.startsWith('image/') ?? false);

  @override
  Widget build(BuildContext context) {
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
                style: AppTheme.mono(color: AppColors.textFaint),
              ),
      );
    }
    return _LaidOutDiff(
      api: api,
      raw: raw,
      file: file,
      sideBySide: sideBySide,
      dualGutter: dualGutter,
      onLoadText: onLoadText,
      onAddComment: onAddComment,
    );
  }
}

/// One file's diff, laid out by the cdylib and rendered by [DiffView].
///
/// Owns the two pieces of state a layout is a function of: the gap expansions
/// the user has asked for, and the file text those expansions reveal *from*.
/// The text is fetched only once the layout says the diff actually elides
/// something — a file with no hidden context never costs a round trip.
class _LaidOutDiff extends StatefulWidget {
  final CommanderApi api;
  final String? raw;
  final rust.ReviewFileDto file;
  final bool sideBySide;
  final bool dualGutter;
  final Future<String> Function(String path) onLoadText;
  final AddCommentFn? onAddComment;

  const _LaidOutDiff({
    required this.api,
    required this.raw,
    required this.file,
    required this.sideBySide,
    required this.dualGutter,
    required this.onLoadText,
    required this.onAddComment,
  });

  @override
  State<_LaidOutDiff> createState() => _LaidOutDiffState();
}

class _LaidOutDiffState extends State<_LaidOutDiff> {
  DiffLayoutDto? _layout;
  String? _error;

  /// Expansions in the order the user asked for them, replayed onto every fresh
  /// layout. The bridge is stateless by design, so this is the whole of the
  /// view's expansion state.
  final List<DiffExpansion> _expansions = [];

  /// The file's working-tree text, once fetched.
  String? _text;
  bool _fetchingText = false;

  /// Bumped whenever the widget switches to a different file (or the same file
  /// after a refresh). Both async paths capture it before their first `await`
  /// and drop their result if it has moved on — otherwise a text fetch or a
  /// layout started for file A can land after the switch and be applied to
  /// file B, giving it A's [FileSource] (wrong trailing-gap size, wrong
  /// revealed lines) and permanently overwriting B's own result if B's landed
  /// first.
  int _generation = 0;

  @override
  void initState() {
    super.initState();
    _layOut();
  }

  @override
  void didUpdateWidget(covariant _LaidOutDiff old) {
    super.didUpdateWidget(old);
    // Identity, not equality: every snapshot rebuilds fresh DTOs, so a changed
    // identity is exactly "a different file, or the same file after a refresh".
    // Either invalidates the revealed text and the gap indices the expansions
    // name, and the DTOs' generated `==` compares hunk *lists* by identity
    // anyway, so equality would answer the same question less honestly.
    if (!identical(old.file, widget.file)) {
      _expansions.clear();
      _text = null;
      _fetchingText = false;
      _generation++;
      _layOut();
    } else if (old.sideBySide != widget.sideBySide) {
      // Presentation only: expansions are expressed in gap indices, which the
      // mode does not move.
      _layOut();
    }
  }

  Future<void> _layOut() async {
    final gen = _generation;
    try {
      final layout = await widget.api.diffRows(
        raw: widget.raw,
        file: widget.file,
        mode: widget.sideBySide
            ? DiffLayoutMode.sideBySide
            : DiffLayoutMode.inline,
        fileText: _text,
        expansions: List.of(_expansions),
      );
      if (!mounted || gen != _generation) return;
      setState(() {
        _layout = layout;
        _error = null;
      });
      if (layout.hasHiddenContext && _text == null) _fetchText();
    } catch (e) {
      if (!mounted || gen != _generation) return;
      setState(() => _error = e.toString());
    }
  }

  /// Fetch the file text the expand controls reveal from, then re-lay-out so
  /// they appear. A failure is silent: the diff still reads, it just cannot be
  /// expanded, and there is nothing the user could do about it here.
  Future<void> _fetchText() async {
    if (_fetchingText) return;
    _fetchingText = true;
    final gen = _generation;
    try {
      final text = await widget.onLoadText(widget.file.displayPath);
      // The file may have been switched while this was in flight. Applying
      // A's text to B would give B the wrong `FileSource` — and clobber B's
      // own text if it arrived first.
      if (!mounted || gen != _generation) return;
      _text = text;
      await _layOut();
    } catch (_) {
      // Leave the diff collapsed rather than surfacing an error for an
      // affordance the user has not asked for yet.
    }
  }

  void _expand(DiffExpansion expansion) {
    _expansions.add(expansion);
    _layOut();
  }

  @override
  Widget build(BuildContext context) {
    final error = _error;
    if (error != null) {
      return Padding(
        padding: const EdgeInsets.all(16),
        child: Text(
          'Could not lay out this diff: $error',
          style: AppTheme.mono(color: AppColors.red),
        ),
      );
    }
    final layout = _layout;
    if (layout == null) {
      return const Padding(
        padding: EdgeInsets.all(24),
        child: Center(child: CircularProgressIndicator()),
      );
    }
    return DiffView(
      file: widget.file.displayPath,
      layout: layout,
      sideBySide: widget.sideBySide,
      dualGutter: widget.dualGutter,
      onAddComment: widget.onAddComment,
      onExpand: _expand,
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
    final range = widget.lineEnd != widget.lineStart
        ? 'L${widget.lineStart}-${widget.lineEnd}'
        : 'L${widget.lineStart}';
    return AlertDialog(
      title: const Text('Add comment'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '${widget.file} · $range',
            style: AppTheme.mono(color: AppColors.textFaint),
          ),
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
    if (_bytes != null) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '${widget.mime} · ${widget.side} side',
            style: AppTheme.mono(color: AppColors.textFaint),
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
          Text('Failed: $_error', style: const TextStyle(color: AppColors.red)),
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
