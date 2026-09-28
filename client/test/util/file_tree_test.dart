import 'package:claude_commander_client/util/file_tree.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  /// A readable rendering of the visible rows: indent, then a `dir/` or a leaf.
  List<String> render(List<String> paths, {Set<String> collapsed = const {}}) =>
      [
        for (final row in flattenFileTree(buildFileTree(paths), collapsed))
          switch (row) {
            FileTreeDirRow() => '${'  ' * row.depth}${row.name}/',
            FileTreeFileRow() => '${'  ' * row.depth}${row.name}',
          },
      ];

  test('a single-child directory chain compresses to one row', () {
    // The point of the tree: `crates` → `core` → `src` → `git` costs four rows
    // and says nothing that one row does not.
    expect(render(['crates/core/src/git/diff.rs']), [
      'crates/core/src/git/',
      '  diff.rs',
    ]);
  });

  test('a chain stops compressing where it branches', () {
    expect(render(['lib/a/one.dart', 'lib/b/two.dart']), [
      'lib/',
      '  a/',
      '    one.dart',
      '  b/',
      '    two.dart',
    ]);
  });

  test('files at the root sit alongside directories, in input order', () {
    expect(render(['README.md', 'lib/main.dart', 'LICENSE']), [
      'README.md',
      'lib/',
      '  main.dart',
      'LICENSE',
    ]);
  });

  test('collapsing a directory hides its whole subtree', () {
    final paths = ['lib/a/one.dart', 'lib/b/two.dart', 'top.txt'];
    // The key is the compressed node's *full* path, so a collapse survives a
    // refresh that reorders the files.
    expect(render(paths, collapsed: {'lib'}), ['lib/', 'top.txt']);
    expect(render(paths, collapsed: {'lib/a'}), [
      'lib/',
      '  a/',
      '  b/',
      '    two.dart',
      'top.txt',
    ]);
  });

  test('file rows keep the index of the file they came from', () {
    final rows = flattenFileTree(
      buildFileTree(['z/last.rs', 'a/first.rs']),
      const {},
    ).whereType<FileTreeFileRow>().toList();
    // Order follows the tree, but each row still points back at its own file.
    expect(rows.map((r) => r.name), ['last.rs', 'first.rs']);
    expect(rows.map((r) => r.index), [0, 1]);
  });

  test('a file and a directory of the same name stay separate', () {
    // A rename can leave `build` (a file) next to `build/` (a directory);
    // folding them together would lose one of them entirely.
    expect(render(['build', 'build/out.o']), ['build', 'build/', '  out.o']);
  });
}
