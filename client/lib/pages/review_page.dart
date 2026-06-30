import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../server_config.dart';
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/review.dart' as rust;

/// Review/diff + comments view for a session. Fetches the review snapshot
/// (parsed unified diff, comments, reviewed marks), lets the user browse files
/// and hunks, select a line range and attach a comment, then apply the staged
/// comments back to the agent. Mirrors the TUI review view, scoped to a first
/// cut: binary files render as a placeholder, reviewed marks are read-only.
class ReviewPage extends StatefulWidget {
  final ServerConfig config;
  final SessionInfo session;

  const ReviewPage({super.key, required this.config, required this.session});

  @override
  State<ReviewPage> createState() => _ReviewPageState();
}

class _ReviewPageState extends State<ReviewPage> {
  rust.ReviewSnapshotDto? _snapshot;
  String? _error;
  bool _loading = true;
  bool _busy = false;

  /// Display paths currently marked reviewed; mutated optimistically by
  /// [_toggleReviewed] and re-synced from each snapshot.
  final Set<String> _reviewed = {};

  String get _id => widget.session.id;

  @override
  void initState() {
    super.initState();
    _open();
  }

  Future<void> _open() async {
    setState(() => _loading = true);
    try {
      final snap = await rust.openReview(
        baseUrl: widget.config.baseUrl,
        token: widget.config.token,
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
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _loading = false;
      });
    }
  }

  /// Re-fetch only if the diff changed (204 → keep current snapshot).
  Future<void> _refresh() async {
    final prev = _snapshot;
    if (prev == null) return _open();
    setState(() => _busy = true);
    try {
      final snap = await rust.refreshReview(
        baseUrl: widget.config.baseUrl,
        token: widget.config.token,
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
      await rust.deleteComment(
        baseUrl: widget.config.baseUrl,
        token: widget.config.token,
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
      final result = await rust.applyComments(
        baseUrl: widget.config.baseUrl,
        token: widget.config.token,
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
    try {
      final nowReviewed = await rust.toggleFileReviewed(
        baseUrl: widget.config.baseUrl,
        token: widget.config.token,
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
    }
  }

  Future<Uint8List> _loadBlob(String side, String path) => rust.fetchBlob(
    baseUrl: widget.config.baseUrl,
    token: widget.config.token,
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
      builder: (ctx) => _CommentDialog(
        file: file,
        lineStart: lineStart,
        lineEnd: lineEnd,
      ),
    );
    if (text == null || text.trim().isEmpty) return;
    setState(() => _busy = true);
    try {
      await rust.createComment(
        baseUrl: widget.config.baseUrl,
        token: widget.config.token,
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
    final stagedCount = snap?.comments
            .where((c) => c.status == rust.ReviewCommentStatus.staged)
            .length ??
        0;
    return Scaffold(
      appBar: AppBar(
        title: Text(
          'Review · ${widget.session.title}',
          overflow: TextOverflow.ellipsis,
        ),
        actions: [
          IconButton(
            onPressed: _busy ? null : _refresh,
            icon: const Icon(Icons.refresh),
            tooltip: 'Refresh diff',
          ),
        ],
      ),
      floatingActionButton: (snap != null && stagedCount > 0)
          ? FloatingActionButton.extended(
              onPressed: _busy ? null : _apply,
              icon: const Icon(Icons.send),
              label: Text('Apply ($stagedCount)'),
            )
          : null,
      body: _body(context, snap),
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
                style: Theme.of(context).textTheme.bodyMedium,
              ),
            ),
          ],
        ),
      );
    }
    return RefreshIndicator(
      onRefresh: _open,
      child: ListView(
        padding: const EdgeInsets.all(12),
        children: [
          Text(
            'Base: ${snap.base}',
            style: Theme.of(context).textTheme.labelMedium,
          ),
          const SizedBox(height: 8),
          if (snap.comments.isNotEmpty) ...[
            Text('Comments', style: Theme.of(context).textTheme.titleSmall),
            const SizedBox(height: 6),
            ...snap.comments.map((c) => _commentCard(context, c)),
            const SizedBox(height: 16),
          ],
          Text('Files', style: Theme.of(context).textTheme.titleSmall),
          const SizedBox(height: 6),
          ...snap.files.map(
            (f) => _FileCard(
              file: f,
              reviewed: _reviewed.contains(f.displayPath),
              onToggleReviewed: _busy ? null : () => _toggleReviewed(f.displayPath),
              onLoadImage: _loadBlob,
              onAddComment: _busy ? null : _addComment,
            ),
          ),
        ],
      ),
    );
  }

  Widget _commentCard(BuildContext context, rust.CommentDto c) {
    return Card(
      margin: const EdgeInsets.only(bottom: 6),
      child: ListTile(
        title: Text(c.comment),
        subtitle: Text(
          '${c.file} · ${c.side == rust.ReviewCommentSide.old ? "old" : "new"} '
          'L${c.lineStart}'
          '${c.lineEnd != c.lineStart ? "-${c.lineEnd}" : ""}',
          style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
        ),
        leading: _commentStatusChip(context, c.status),
        trailing: IconButton(
          onPressed: _busy ? null : () => _deleteComment(c.id),
          icon: const Icon(Icons.delete_outline),
          tooltip: 'Delete comment',
        ),
        isThreeLine: false,
      ),
    );
  }

  Widget _commentStatusChip(BuildContext context, rust.ReviewCommentStatus s) {
    final (label, color) = switch (s) {
      rust.ReviewCommentStatus.staged => ('staged', Colors.lightBlue),
      rust.ReviewCommentStatus.drifted => ('drifted', Colors.orange),
      rust.ReviewCommentStatus.applied => ('applied', Colors.green),
    };
    return _pill(label, color);
  }

  Widget _errorView(BuildContext context, String error) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.warning_amber,
              color: Theme.of(context).colorScheme.error,
            ),
            const SizedBox(height: 12),
            Text(error, textAlign: TextAlign.center),
            const SizedBox(height: 16),
            FilledButton.tonal(onPressed: _open, child: const Text('Retry')),
          ],
        ),
      ),
    );
  }
}

/// One changed file as an expandable card: header (path + stats + status), and
/// either a unified-diff body or a binary placeholder.
class _FileCard extends StatelessWidget {
  final rust.ReviewFileDto file;
  final bool reviewed;

  /// Toggle this file's reviewed mark; null while busy.
  final VoidCallback? onToggleReviewed;

  /// Fetch raw bytes for one side of a (binary) file: `(side, path) → bytes`.
  final Future<Uint8List> Function(String side, String path) onLoadImage;

  /// Stage a comment for a selected line range; null while busy.
  final Future<void> Function({
    required String file,
    required String side,
    required int lineStart,
    required int lineEnd,
    required String snippet,
  })? onAddComment;

  const _FileCard({
    required this.file,
    required this.reviewed,
    required this.onToggleReviewed,
    required this.onLoadImage,
    required this.onAddComment,
  });

  /// Which side's blob to render: deletions only have an old side; everything
  /// else shows the new (working-tree) side.
  String get _imageSide =>
      file.status == rust.ReviewFileStatus.deleted ? 'old' : 'new';

  bool get _isImage => file.isBinary && (file.binaryMime?.startsWith('image/') ?? false);

  @override
  Widget build(BuildContext context) {
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      child: ExpansionTile(
        leading: Checkbox(
          value: reviewed,
          onChanged:
              onToggleReviewed == null ? null : (_) => onToggleReviewed!(),
        ),
        title: Text(
          file.displayPath,
          style: const TextStyle(fontFamily: 'monospace', fontSize: 13),
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: Row(
          children: [
            _statusChip(context, file.status),
            const SizedBox(width: 8),
            Text(
              '+${file.added}',
              style: const TextStyle(color: Colors.green, fontSize: 12),
            ),
            const SizedBox(width: 6),
            Text(
              '-${file.removed}',
              style: const TextStyle(color: Colors.red, fontSize: 12),
            ),
          ],
        ),
        children: [
          if (file.isBinary)
            Padding(
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
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
            )
          else
            ...file.hunks.map(
              (h) => _HunkView(
                file: file.displayPath,
                hunk: h,
                onAddComment: onAddComment,
              ),
            ),
        ],
      ),
    );
  }

  Widget _statusChip(BuildContext context, rust.ReviewFileStatus status) {
    final (label, color) = switch (status) {
      rust.ReviewFileStatus.added => ('added', Colors.green),
      rust.ReviewFileStatus.deleted => ('deleted', Colors.red),
      rust.ReviewFileStatus.modified => ('modified', Colors.blue),
      rust.ReviewFileStatus.renamed => ('renamed', Colors.purple),
    };
    return _pill(label, color);
  }
}

/// A single hunk rendered as a unified diff: a header line followed by
/// color-coded lines with old/new line-number gutters. Tapping a line selects
/// it (tap-and-hold extends to a range) and offers an "add comment" action.
class _HunkView extends StatefulWidget {
  final String file;
  final rust.ReviewHunkDto hunk;
  final Future<void> Function({
    required String file,
    required String side,
    required int lineStart,
    required int lineEnd,
    required String snippet,
  })? onAddComment;

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

    final numbers = selected
        .map((l) => allDeletions ? l.oldLineno : l.newLineno)
        .whereType<int>()
        .toList();
    if (numbers.isEmpty) {
      _clear();
      return;
    }
    numbers.sort();
    final snippet = selected.map((l) => l.content).join('\n');

    await cb(
      file: widget.file,
      side: side,
      lineStart: numbers.first,
      lineEnd: numbers.last,
      snippet: snippet,
    );
    _clear();
  }

  @override
  Widget build(BuildContext context) {
    final hunk = widget.hunk;
    final hasSelection = _anchor != null && _focus != null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: double.infinity,
          color: Colors.indigo.withValues(alpha: 0.18),
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          child: Text(
            '@@ -${hunk.oldStart},${hunk.oldLines} '
            '+${hunk.newStart},${hunk.newLines} @@'
            '${hunk.header.isNotEmpty ? " ${hunk.header}" : ""}',
            style: const TextStyle(
              fontFamily: 'monospace',
              fontSize: 11,
              color: Colors.indigoAccent,
            ),
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

/// One diff line: old/new gutters + a color-coded, monospace content row.
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
    final (bg, marker, fg) = switch (line.origin) {
      rust.ReviewLineOrigin.addition => (
        Colors.green.withValues(alpha: 0.12),
        '+',
        Colors.greenAccent,
      ),
      rust.ReviewLineOrigin.deletion => (
        Colors.red.withValues(alpha: 0.12),
        '-',
        Colors.redAccent,
      ),
      rust.ReviewLineOrigin.context => (
        Colors.transparent,
        ' ',
        Colors.white70,
      ),
    };
    return InkWell(
      onTap: onTap,
      onLongPress: onLongPress,
      child: Container(
        color: selected ? Colors.amber.withValues(alpha: 0.30) : bg,
        padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 1),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _gutter(line.oldLineno),
            _gutter(line.newLineno),
            const SizedBox(width: 4),
            Text(
              marker,
              style: TextStyle(
                fontFamily: 'monospace',
                fontSize: 12,
                color: fg,
              ),
            ),
            const SizedBox(width: 4),
            Expanded(
              child: Text(
                line.content,
                style: TextStyle(
                  fontFamily: 'monospace',
                  fontSize: 12,
                  color: fg,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _gutter(int? n) {
    return SizedBox(
      width: 36,
      child: Text(
        n?.toString() ?? '',
        textAlign: TextAlign.right,
        style: const TextStyle(
          fontFamily: 'monospace',
          fontSize: 11,
          color: Colors.white38,
        ),
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
            style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
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
            style: Theme.of(context).textTheme.labelSmall,
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
          Text(
            'Failed: $_error',
            style: TextStyle(color: Theme.of(context).colorScheme.error),
          ),
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

/// Small coloured pill used for file/comment status labels.
Widget _pill(String label, Color color) {
  return Container(
    padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
    decoration: BoxDecoration(
      color: color.withValues(alpha: 0.18),
      borderRadius: BorderRadius.circular(6),
      border: Border.all(color: color.withValues(alpha: 0.5)),
    ),
    child: Text(label, style: TextStyle(color: color, fontSize: 11)),
  );
}
