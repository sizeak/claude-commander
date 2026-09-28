import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/src/rust/api/workspace.dart';

/// Stand-ins for the cdylib's workspace rules (`rust/src/api/workspace.rs`), for
/// widget tests.
///
/// `flutter test` loads no native library, so the real merge, startup
/// resolution, name checks and per-server narrowing are out of reach — the
/// same split `fake_diff_layout.dart` documents for the diff engine. These
/// reproduce the behaviour a widget test leans on (Main first, first-source
/// order, orphan tags appended; a missing startup target falls back to Main;
/// the reserved and empty names refused; a server keeps its own spellings) and
/// nothing more. The rules themselves are pinned by the viewmodel's and the
/// cdylib's Rust tests; a Dart test that wanted to probe an edge of them would
/// be testing this file.

List<MergedWorkspace> fakeMergeWorkspaces(List<WorkspaceSourceDto> sources) {
  WorkspaceDef? main;
  for (final s in sources) {
    if (s.main != null) {
      main = s.main;
      break;
    }
  }
  final merged = <MergedWorkspace>[
    MergedWorkspace(label: main?.name ?? 'Main'),
  ];
  int indexOf(String name) => merged.indexWhere((m) => m.name == name);
  for (final def in sources.expand((s) => s.defs)) {
    if (indexOf(def.name) < 0) {
      merged.add(MergedWorkspace(name: def.name, label: def.name));
    }
  }
  for (final tag in sources.expand((s) => s.projectTags)) {
    if (indexOf(tag) < 0) {
      merged.add(MergedWorkspace(name: tag, label: tag));
    }
  }
  return merged;
}

String? fakeResolveStartupWorkspace({
  required String startup,
  String? last,
  required List<MergedWorkspace> workspaces,
}) {
  final wanted = switch (startup.toLowerCase()) {
    'main' => null,
    'last' => last,
    _ => startup,
  };
  if (wanted == null) return null;
  return workspaces.any((w) => w.name == wanted) ? wanted : null;
}

String? fakeWorkspaceLabelError(String raw) {
  final name = raw.trim();
  if (name.isEmpty) return 'workspace name must not be empty';
  if (name.length > 40) return 'workspace name must be at most 40 characters';
  return null;
}

String? fakeWorkspaceNameError(String raw) {
  final label = fakeWorkspaceLabelError(raw);
  if (label != null) return label;
  final name = raw.trim();
  if (const ['last', 'main'].contains(name.toLowerCase())) {
    return '"$name" is a reserved workspace name';
  }
  return null;
}

bool fakeWorkspaceNameTaken(
  List<MergedWorkspace> workspaces,
  String name, {
  MergedWorkspace? except,
}) {
  final wanted = name.trim().toLowerCase();
  return workspaces.any((w) => w != except && w.label.toLowerCase() == wanted);
}

List<WorkspaceDef> fakeDefinitionsForServer({
  required List<WorkspaceDef> wanted,
  required List<WorkspaceDef> own,
  String? mainLabel,
}) {
  final claimed = <String>[
    if (mainLabel != null) mainLabel.toLowerCase(),
    for (final o in own) o.name.toLowerCase(),
  ];
  final out = <WorkspaceDef>[];
  for (final d in wanted) {
    if (own.any((o) => o.name == d.name)) {
      out.add(d);
    } else if (!claimed.contains(d.name.toLowerCase())) {
      claimed.add(d.name.toLowerCase());
      out.add(d);
    }
  }
  return out;
}
