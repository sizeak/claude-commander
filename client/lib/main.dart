import 'package:flutter/material.dart';

import 'pages/adaptive_shell.dart';
import 'pages/connection_page.dart';
import 'server_config.dart';
import 'services/commander_api.dart';
import 'src/rust/frb_generated.dart';
import 'state/commander_store.dart';
import 'state/commander_store_scope.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // Initialise the Rust bridge before any `api` call.
  await RustLib.init();
  const store = SecureServerConfigStore();
  const api = RustCommanderApi();
  final config = await store.load();
  runApp(CommanderApp(api: api, store: store, initialConfig: config));
}

/// Owns the app's single [CommanderStore]. There is no store until a server is
/// configured: first run shows the [ConnectionPage] and only *then* builds the
/// store (via [_handleConnected]). A settings change reconnects the *same* store
/// rather than minting a fresh handle — the handle lifecycle lives in the store,
/// so it can never be abandoned.
class CommanderApp extends StatefulWidget {
  final CommanderApi api;
  final ServerConfigStore store;
  final ServerConfig? initialConfig;

  const CommanderApp({
    super.key,
    required this.api,
    required this.store,
    this.initialConfig,
  });

  @override
  State<CommanderApp> createState() => _CommanderAppState();
}

class _CommanderAppState extends State<CommanderApp> {
  CommanderStore? _store;

  @override
  void initState() {
    super.initState();
    final config = widget.initialConfig;
    if (config != null) {
      final store = CommanderStore(api: widget.api, config: config);
      _store = store;
      // Fire-and-forget: the store surfaces connect progress/errors as state,
      // so the session list can render its connecting/error view immediately.
      store.connect();
    }
  }

  @override
  void dispose() {
    _store?.dispose();
    super.dispose();
  }

  /// Called by the connection page on a successful save. First run creates and
  /// connects the store (flipping the home to the session list); a settings
  /// change reconnects the existing store (releasing the old handle first).
  Future<void> _handleConnected(ServerConfig config) async {
    final existing = _store;
    if (existing == null) {
      final store = CommanderStore(api: widget.api, config: config);
      setState(() => _store = store);
      await store.connect();
    } else {
      await existing.reconnect(config);
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = _store;
    return CommanderStoreScope(
      store: store,
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
        home: store == null
            ? ConnectionPage(
                api: widget.api,
                store: widget.store,
                onConnected: _handleConnected,
              )
            : AdaptiveShell(
                configStore: widget.store,
                onConnected: _handleConnected,
              ),
      ),
    );
  }
}
