import '../chrome/title_menu.dart';
import '../state/fleet_store.dart';
import '../util/workspace_color.dart';

/// The workspace switcher the fleet title carries ("Fleet · Work ▾"), or null
/// while there is only Main — the switcher stays out of sight until there is
/// something to switch to. Each entry shows how many of its sessions are
/// waiting for input, which is how a question asked in another workspace gets
/// noticed without leaving this one.
ChromeTitleMenu? workspaceTitleMenu(FleetStore fleet) {
  if (!fleet.workspacesVisible) return null;
  final active = fleet.activeWorkspace;
  final current = fleet.activeWorkspaceEntry;
  return ChromeTitleMenu(
    current: current.label,
    color: parseWorkspaceColor(current.color),
    items: [
      for (final w in fleet.waitingCounts)
        ChromeTitleMenuItem(
          label: w.workspace.label,
          color: parseWorkspaceColor(w.workspace.color),
          badge: w.waiting,
          selected: w.workspace.name == active,
          onSelected: () => fleet.selectWorkspace(w.workspace.name),
        ),
    ],
  );
}
