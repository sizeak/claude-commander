import 'package:flutter/material.dart';

import 'pages/adaptive_shell.dart';
import 'pages/connection_page.dart';
import 'server_config.dart';
import 'services/commander_api.dart';
import 'src/rust/frb_generated.dart';
import 'services/pref_store.dart';
import 'services/window_service.dart';
import 'state/commander_store_scope.dart';
import 'state/workspace_store.dart';
import 'theme/theme_controller.dart';
import 'theme/theme_data.dart';
import 'theme/tokens.dart';
import 'window/window_controller.dart';
import 'window/window_frame.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // A missing token extension falls back to Mission Control so widget tests can
  // pump bare widgets; in the real app that silence would hide a mis-themed
  // subtree, so opt into the assert here.
  debugAssertTokensPresent = true;
  // Initialise the Rust bridge before any `api` call.
  await RustLib.init();
  final api = RustCommanderApi();
  final workspace = WorkspaceStore(
    api: api,
    listStore: SecureServerListStore(),
  );
  // Restore the saved theme *before* the first frame. The connect screen is the
  // first thing a cold start with no servers shows, and it has to arrive already
  // themed rather than flashing the default for a frame.
  final theme = ThemeController(store: const SharedPrefStore());
  await theme.load();
  // Same rule, for the same reason: the runner shows the window on its first
  // Flutter frame, so a frame or geometry restored late is a visible jump from
  // the default 1280x720 to wherever the user left the window. Null on Android,
  // where there is no window to manage.
  final windowService = createWindowService();
  final window = windowService == null
      ? null
      : WindowController(
          store: const SharedPrefStore(),
          service: windowService,
        );
  await window?.load();
  // Fire-and-forget per server: each surfaces its own connect progress as state.
  await workspace.loadAndConnectAll();
  runApp(
    CommanderApp(api: api, workspace: workspace, theme: theme, window: window),
  );
}

/// Owns the app's [WorkspaceStore] — the multi-server aggregator. Every saved
/// server is connected at once; the session list groups their sessions by
/// server. With no servers configured (first run) the home is the add-server
/// screen; adding the first server flips the home to the [AdaptiveShell].
class CommanderApp extends StatefulWidget {
  final CommanderApi api;
  final WorkspaceStore workspace;

  /// The selected theme. Already loaded by `main()`, so the first frame is
  /// painted in the user's chosen theme rather than the default.
  final ThemeController theme;

  /// The desktop window, or **null** where there is no window to manage. Null is
  /// what leaves Android with no window bar and no F11 handler — the absence is
  /// structural rather than a platform check inside the UI.
  final WindowController? window;

  const CommanderApp({
    super.key,
    required this.api,
    required this.workspace,
    required this.theme,
    this.window,
  });

  @override
  State<CommanderApp> createState() => _CommanderAppState();
}

class _CommanderAppState extends State<CommanderApp> {
  @override
  void dispose() {
    widget.workspace.dispose();
    widget.window?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return WorkspaceScope(
      workspace: widget.workspace,
      child: WindowScope(
        controller: widget.window,
        child: ThemeScope(
          controller: widget.theme,
          // Rebuilds the whole app on a theme change, which is the whole
          // switching mechanism: MaterialApp wraps an AnimatedTheme, so colours
          // crossfade rather than snap.
          //
          // Structure does *not* crossfade. `tokens.chrome` swaps at the lerp
          // midpoint, and the two chromes build different widget types, so the
          // shells re-inflate and page State beneath them is rebuilt: a live
          // terminal attach in the wide shell reopens, and fleet search text
          // resets. That is the same cost as switching workspace tabs, which
          // already re-creates attaches — but it is not "nothing is disposed",
          // and the server-side session is untouched either way.
          child: ListenableBuilder(
            listenable: widget.theme,
            builder: (context, _) => MaterialApp(
              title: 'Claude Commander',
              debugShowCheckedModeBanner: false,
              theme: themeDataFor(widget.theme.tokens),
              // Inside the app's Theme (so the bar resolves tokens) and above the
              // Navigator (so no pushed route can cover it).
              builder: (context, child) => WindowFrame(child: child!),
              home: ListenableBuilder(
                listenable: widget.workspace,
                builder: (context, _) => widget.workspace.isEmpty
                    // First run: no servers yet. Adding one flips the home below.
                    ? ConnectionPage(
                        api: widget.api,
                        onSubmit: widget.workspace.addServer,
                      )
                    : const AdaptiveShell(),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
