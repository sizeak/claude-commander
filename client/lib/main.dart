import 'package:flutter/material.dart';

import 'pages/connection_page.dart';
import 'pages/session_list_page.dart';
import 'server_config.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // Initialise the Rust bridge before any `api` call.
  await RustLib.init();
  final config = await ServerConfigStore.load();
  runApp(CommanderApp(initialConfig: config));
}

class CommanderApp extends StatelessWidget {
  final ServerConfig? initialConfig;
  const CommanderApp({super.key, this.initialConfig});

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
          ? const ConnectionPage()
          : SessionListPage(config: config),
    );
  }
}
