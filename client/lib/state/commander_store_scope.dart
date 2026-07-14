import 'package:flutter/widgets.dart';

import 'commander_store.dart';

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
