import 'package:flutter/widgets.dart';

import 'commander_store.dart';
import 'workspace_store.dart';

/// Exposes the app's single [CommanderStore] to the widget tree. Placed above
/// the `MaterialApp` so pushed routes (detail/terminal/review/settings) can
/// reach it too. The store instance only changes on first connect (null →
/// store); per-field reactivity is via `ListenableBuilder`, not this widget.
class CommanderStoreScope extends InheritedWidget {
  final CommanderStore? store;

  const CommanderStoreScope({
    super.key,
    required this.store,
    required super.child,
  });

  static CommanderStore? of(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<CommanderStoreScope>()
      ?.store;

  @override
  bool updateShouldNotify(CommanderStoreScope oldWidget) =>
      store != oldWidget.store;
}

/// Exposes the app's [WorkspaceStore] (the multi-server aggregator) to the widget
/// tree, placed above the `MaterialApp` so pushed routes can reach it. The list
/// page reads this to enumerate servers; each server group then re-provides its
/// own [CommanderStoreScope] so per-server consumers keep their single-store
/// contract. Per-field reactivity is via `ListenableBuilder`, not this widget.
class WorkspaceScope extends InheritedWidget {
  final WorkspaceStore? workspace;

  const WorkspaceScope({
    super.key,
    required this.workspace,
    required super.child,
  });

  static WorkspaceStore? of(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<WorkspaceScope>()
      ?.workspace;

  @override
  bool updateShouldNotify(WorkspaceScope oldWidget) =>
      workspace != oldWidget.workspace;
}
