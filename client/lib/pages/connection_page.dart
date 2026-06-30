import 'package:flutter/material.dart';

import '../server_config.dart';
import '../src/rust/api/simple.dart' as rust;
import 'session_list_page.dart';

/// First-run / settings screen: enter the server URL + bearer token, test the
/// connection, and save. On save we navigate to the session list.
class ConnectionPage extends StatefulWidget {
  final ServerConfig? existing;
  const ConnectionPage({super.key, this.existing});

  @override
  State<ConnectionPage> createState() => _ConnectionPageState();
}

class _ConnectionPageState extends State<ConnectionPage> {
  final _formKey = GlobalKey<FormState>();
  late final TextEditingController _urlController;
  late final TextEditingController _tokenController;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _urlController = TextEditingController(
      text: widget.existing?.baseUrl ?? 'http://127.0.0.1:7878',
    );
    _tokenController = TextEditingController(text: widget.existing?.token ?? '');
  }

  @override
  void dispose() {
    _urlController.dispose();
    _tokenController.dispose();
    super.dispose();
  }

  ServerConfig get _config => ServerConfig(
    baseUrl: _urlController.text.trim(),
    token: _tokenController.text.trim(),
  );

  void _snack(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        backgroundColor: error ? Theme.of(context).colorScheme.error : null,
      ),
    );
  }

  Future<void> _test() async {
    if (!_formKey.currentState!.validate()) return;
    setState(() => _busy = true);
    try {
      final cfg = _config;
      final alive = await rust.health(baseUrl: cfg.baseUrl);
      if (!alive) {
        _snack('Server reachable but /health did not return OK', error: true);
        return;
      }
      // Authenticated probe — surfaces a 401 as an error.
      final tmuxOk = await rust.healthTmux(
        baseUrl: cfg.baseUrl,
        token: cfg.token,
      );
      _snack(tmuxOk ? 'Connected — auth OK, tmux healthy' : 'Auth OK, but tmux is unavailable');
    } catch (e) {
      _snack('Connection failed: $e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _saveAndConnect() async {
    if (!_formKey.currentState!.validate()) return;
    setState(() => _busy = true);
    try {
      final cfg = _config;
      await ServerConfigStore.save(cfg);
      if (!mounted) return;
      Navigator.of(context).pushReplacement(
        MaterialPageRoute(builder: (_) => SessionListPage(config: cfg)),
      );
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Connect to server')),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: Form(
          key: _formKey,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              TextFormField(
                controller: _urlController,
                decoration: const InputDecoration(
                  labelText: 'Server URL',
                  hintText: 'http://100.x.y.z:7878',
                  helperText: 'Reach a 127.0.0.1 server via SSH tunnel or Tailscale',
                ),
                keyboardType: TextInputType.url,
                autocorrect: false,
                validator: (v) {
                  final t = v?.trim() ?? '';
                  if (t.isEmpty) return 'Required';
                  final uri = Uri.tryParse(t);
                  if (uri == null || !uri.hasScheme || uri.host.isEmpty) {
                    return 'Enter a full URL (scheme://host:port)';
                  }
                  return null;
                },
              ),
              const SizedBox(height: 16),
              TextFormField(
                controller: _tokenController,
                decoration: const InputDecoration(
                  labelText: 'Bearer token',
                  helperText: 'The server prints this on first run',
                ),
                obscureText: true,
                autocorrect: false,
                enableSuggestions: false,
                validator: (v) =>
                    (v?.trim().isEmpty ?? true) ? 'Required' : null,
              ),
              const SizedBox(height: 24),
              OutlinedButton.icon(
                onPressed: _busy ? null : _test,
                icon: const Icon(Icons.wifi_tethering),
                label: const Text('Test connection'),
              ),
              const SizedBox(height: 12),
              FilledButton.icon(
                onPressed: _busy ? null : _saveAndConnect,
                icon: _busy
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.login),
                label: const Text('Save & connect'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
