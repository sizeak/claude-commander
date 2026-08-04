/// The changed-files tree for the review sidebar: paths in, collapsible rows
/// out.
///
/// A flat list of paths stops being readable the moment a change spans more
/// than one directory, and a naive tree is worse — a chain of one-child folders
/// costs a row each and says nothing. So single-child directory chains are
/// compressed into one row (`lib` → `src` → files becomes `lib/src`), which is
/// the same lazygit-style shape the TUI's review pane shows.
///
/// This mirrors `build_file_tree`/`compress`/`flatten_tree` in the TUI's
/// `review.rs`. It is a *path* concern, not a diff-layout one, so `diffgrid`
/// deliberately has nothing to say about it — but the duplication is real, and
/// the fix if it ever drifts is to lift the algorithm into the shared protocol
/// crate and expose it through the bridge, not to let the two answers diverge.
library;

/// A node in the tree: a directory with children, or a file leaf.
class FileTreeNode {
  FileTreeNode({
    required this.name,
    required this.path,
    this.fileIndex,
    List<FileTreeNode>? children,
  }) : children = children ?? [];

  /// The segment label. A compressed directory joins its segments with `/`.
  String name;

  /// Full path of this node — the key a collapse is remembered under.
  String path;

  /// Index into the caller's file list, for a leaf. `null` for a directory.
  final int? fileIndex;

  List<FileTreeNode> children;

  bool get isDirectory => fileIndex == null;
}

/// One visible row of the flattened tree.
sealed class FileTreeRow {
  const FileTreeRow(this.depth);
  final int depth;
}

class FileTreeDirRow extends FileTreeRow {
  const FileTreeDirRow({
    required int depth,
    required this.path,
    required this.name,
    required this.collapsed,
  }) : super(depth);

  final String path;
  final String name;
  final bool collapsed;
}

class FileTreeFileRow extends FileTreeRow {
  const FileTreeFileRow({
    required int depth,
    required this.index,
    required this.name,
  }) : super(depth);

  /// Index into the file list the tree was built from.
  final int index;

  /// The file's own segment, not its whole path — the directories above it
  /// already say where it lives.
  final String name;
}

/// Build the tree for `displayPaths`, in the order given.
List<FileTreeNode> buildFileTree(List<String> displayPaths) {
  final roots = <FileTreeNode>[];
  for (var i = 0; i < displayPaths.length; i++) {
    _insert(roots, displayPaths[i].split('/'), i, '');
  }
  for (final node in roots) {
    _compress(node);
  }
  return roots;
}

void _insert(
  List<FileTreeNode> children,
  List<String> segments,
  int fileIndex,
  String prefix,
) {
  if (segments.isEmpty) return;
  final head = segments.first;
  final rest = segments.sublist(1);
  final path = prefix.isEmpty ? head : '$prefix/$head';
  if (rest.isEmpty) {
    children.add(FileTreeNode(name: head, path: path, fileIndex: fileIndex));
    return;
  }
  // Only merge into an existing *directory*: a file and a directory can share a
  // name across a rename, and folding them together would lose one.
  var idx = children.indexWhere((n) => n.isDirectory && n.name == head);
  if (idx < 0) {
    children.add(FileTreeNode(name: head, path: path));
    idx = children.length - 1;
  }
  _insert(children[idx].children, rest, fileIndex, path);
}

/// Merge a directory with its sole child while that child is also a directory,
/// then recurse.
void _compress(FileTreeNode node) {
  while (node.isDirectory &&
      node.children.length == 1 &&
      node.children.first.isDirectory) {
    final child = node.children.removeAt(0);
    node.name = '${node.name}/${child.name}';
    node.path = child.path;
    node.children = child.children;
  }
  for (final child in node.children) {
    _compress(child);
  }
}

/// Flatten to visible rows, skipping the subtree of every collapsed directory.
List<FileTreeRow> flattenFileTree(
  List<FileTreeNode> nodes,
  Set<String> collapsed, {
  int depth = 0,
}) {
  final out = <FileTreeRow>[];
  for (final node in nodes) {
    final index = node.fileIndex;
    if (index != null) {
      out.add(FileTreeFileRow(depth: depth, index: index, name: node.name));
      continue;
    }
    final isCollapsed = collapsed.contains(node.path);
    out.add(
      FileTreeDirRow(
        depth: depth,
        path: node.path,
        name: node.name,
        collapsed: isCollapsed,
      ),
    );
    if (!isCollapsed) {
      out.addAll(flattenFileTree(node.children, collapsed, depth: depth + 1));
    }
  }
  return out;
}
