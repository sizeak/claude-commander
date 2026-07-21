import 'package:flutter/material.dart';

import 'pages/adaptive_shell.dart';
import 'pages/connection_page.dart';
import 'server_config.dart';
import 'services/commander_api.dart';
import 'src/rust/frb_generated.dart';
import 'state/commander_store_scope.dart';
import 'state/workspace_store.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // Initialise the Rust bridge before any `api` call.
  await RustLib.init();
  const api = RustCommanderApi();
  final workspace = WorkspaceStore(
    api: api,
    listStore: SecureServerListStore(),
  );
  // Fire-and-forget per server: each surfaces its own connect progress as state.
  await workspace.loadAndConnectAll();
  runApp(CommanderApp(api: api, workspace: workspace));
}

/// Owns the app's [WorkspaceStore] — the multi-server aggregator. Every saved
/// server is connected at once; the session list groups their sessions by
/// server. With no servers configured (first run) the home is the add-server
/// screen; adding the first server flips the home to the [AdaptiveShell].
class CommanderApp extends StatefulWidget {
  final CommanderApi api;
  final WorkspaceStore workspace;

  const CommanderApp({super.key, required this.api, required this.workspace});

  @override
  State<CommanderApp> createState() => _CommanderAppState();
}

class _CommanderAppState extends State<CommanderApp> {
  @override
  void dispose() {
    widget.workspace.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return WorkspaceScope(
      workspace: widget.workspace,
      child: MaterialApp(
        title: 'Claude Commander',
        debugShowCheckedModeBanner: false,
        theme: ThemeData(
          colorScheme: ColorScheme.fromSeed(
            seedColor: Colors.deepPurple,
            brightness: Brightness.dark,
          ),
          useMaterial3: true,
        ),
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
    );
  }
}
