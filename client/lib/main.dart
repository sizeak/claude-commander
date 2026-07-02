import 'package:flutter/material.dart';

import 'pages/connection_page.dart';
import 'pages/session_list_page.dart';
import 'server_config.dart';
import 'services/commander_api.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // Initialise the Rust bridge before any `api` call.
  await RustLib.init();
  const store = SecureServerConfigStore();
  final config = await store.load();
  runApp(
    CommanderApp(
      api: const RustCommanderApi(),
      store: store,
      initialConfig: config,
    ),
  );
}

class CommanderApp extends StatelessWidget {
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
  Widget build(BuildContext context) {
    final config = initialConfig;
    return MaterialApp(
      title: 'Claude Commander',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: Colors.deepPurple,
          brightness: Brightness.dark,
        ),
        useMaterial3: true,
      ),
      home: config == null
          ? ConnectionPage(api: api, store: store)
          : SessionListPage(api: api, store: store, config: config),
    );
  }
}
