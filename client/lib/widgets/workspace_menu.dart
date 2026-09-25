import '../chrome/title_menu.dart';
import '../state/fleet_store.dart';
import '../theme/theme_controller.dart';
import '../theme/theme_prefs.dart';

/// The workspace switcher the fleet title carries ("Fleet · Work ▾"), or null
/// while there is only Main — the switcher stays out of sight until there is
/// something to switch to. Each entry shows how many of its sessions are
/// waiting for input, which is how a question asked in another workspace gets
/// noticed without leaving this one.
///
/// Given the device's [theme], the current label and each entry's dot are that
/// workspace's resolved `primary`, so a themed workspace is recognisable in the
/// list before it is switched to. Without one (a test host with no
/// [ThemeScope]) nothing is tinted.
ChromeTitleMenu? workspaceTitleMenu(
  FleetStore fleet, {
  ThemeController? theme,
}) {
  if (!fleet.workspacesVisible) return null;
  final active = fleet.activeWorkspace;
  final current = fleet.activeWorkspaceEntry;
  final primaryOf = theme == null
      ? null
      : (String? name) => theme.tokensFor(workspaceThemeKey(name)).primary;
  return ChromeTitleMenu(
    current: current.label,
    color: primaryOf?.call(current.name),
    items: [
      for (final w in fleet.waitingCounts)
        ChromeTitleMenuItem(
          label: w.workspace.label,
          color: primaryOf?.call(w.workspace.name),
          badge: w.waiting,
          selected: w.workspace.name == active,
          onSelected: () => fleet.selectWorkspace(w.workspace.name),
        ),
    ],
  );
}
