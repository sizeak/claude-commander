import 'package:flutter/widgets.dart';

import 'commander_store.dart';
import 'fleet_store.dart';

/// Exposes one server's [CommanderStore] to the subtree beneath it. In the
/// aggregated multi-server UI it is re-provided per server group (and per pushed
/// detail/terminal/review route) so per-server consumers resolve the store for
/// the server they belong to; the top-level aggregator is [FleetScope].
/// Per-field reactivity is via `ListenableBuilder`, not this widget.
class CommanderStoreScope extends InheritedWidget {
  final CommanderStore? store;

  const CommanderStoreScope({
    super.key,
    required this.store,
    required super.child,
  });

  static CommanderStore? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<CommanderStoreScope>()?.store;

  @override
  bool updateShouldNotify(CommanderStoreScope oldWidget) =>
      store != oldWidget.store;
}

/// Exposes the app's [FleetStore] (the multi-server aggregator) to the widget
/// tree, placed above the `MaterialApp` so pushed routes can reach it. The list
/// page reads this to enumerate servers; each server group then re-provides its
/// own [CommanderStoreScope] so per-server consumers keep their single-store
/// contract. Per-field reactivity is via `ListenableBuilder`, not this widget.
class FleetScope extends InheritedWidget {
  final FleetStore? fleet;

  const FleetScope({super.key, required this.fleet, required super.child});

  static FleetStore? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<FleetScope>()?.fleet;

  @override
  bool updateShouldNotify(FleetScope oldWidget) => fleet != oldWidget.fleet;
}
