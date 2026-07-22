import 'package:flutter/material.dart';
import 'package:uuid/uuid.dart';

import '../server_config.dart';
import '../services/commander_api.dart';

/// Add / edit a server: enter a display name, URL, and bearer token, optionally
/// test the connection, and save. On save we probe the server first; a failed
/// probe offers "Save anyway?" so an offline server can still be added (it shows
/// degraded in the list until it comes up). [onSubmit] owns persisting +
/// connecting the server (via `WorkspaceStore`); this page never touches storage.
class ConnectionPage extends StatefulWidget {
  final CommanderApi api;

  /// The server being edited, or null when adding a new one. When editing, the
  /// stable [ServerConfig.id] is preserved so the live connection reconciles in
  /// place rather than spawning a duplicate.
  final ServerConfig? existing;

  /// Persist + connect (or reconnect) the server. Invoked with the assembled
  /// config after a successful (or "save anyway") save.
  final Future<void> Function(ServerConfig config) onSubmit;

  const ConnectionPage({
    super.key,
    required this.api,
    this.existing,
    required this.onSubmit,
  });

  @override
  State<ConnectionPage> createState() => _ConnectionPageState();
}

class _ConnectionPageState extends State<ConnectionPage> {
  final _formKey = GlobalKey<FormState>();
  late final TextEditingController _nameController;
  late final TextEditingController _urlController;
  late final TextEditingController _tokenController;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _nameController = TextEditingController(text: widget.existing?.name ?? '');
    _urlController = TextEditingController(
      text: widget.existing?.baseUrl ?? 'http://127.0.0.1:7878',
    );
    _tokenController = TextEditingController(
      text: widget.existing?.token ?? '',
    );
  }

  @override
  void dispose() {
    _nameController.dispose();
    _urlController.dispose();
    _tokenController.dispose();
    super.dispose();
  }

  ServerConfig get _config {
    final url = _urlController.text.trim();
    final name = _nameController.text.trim();
    return ServerConfig(
      id: widget.existing?.id ?? const Uuid().v4(),
      name: name.isEmpty ? ServerConfig.nameFromUrl(url) : name,
      baseUrl: url,
      token: _tokenController.text.trim(),
    );
  }

  void _snack(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        backgroundColor: error ? Theme.of(context).colorScheme.error : null,
      ),
    );
  }

  /// Probe the server: reachability (`/health`) then an authenticated tmux probe
  /// (surfaces a 401). Returns null on success, or a human message on failure.
  Future<String?> _probe(ServerConfig cfg) async {
    final alive = await widget.api.health(baseUrl: cfg.baseUrl);
    if (!alive) return 'Server reachable but /health did not return OK';
    final tmuxOk = await widget.api.healthTmux(
      baseUrl: cfg.baseUrl,
      token: cfg.token,
    );
    return tmuxOk ? null : 'Auth OK, but tmux is unavailable';
  }

  Future<void> _test() async {
    if (!_formKey.currentState!.validate()) return;
    setState(() => _busy = true);
    try {
      final problem = await _probe(_config);
      _snack(
        problem ?? 'Connected — auth OK, tmux healthy',
        error: problem != null,
      );
    } catch (e) {
      _snack('Connection failed: $e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _save() async {
    if (!_formKey.currentState!.validate()) return;
    setState(() => _busy = true);
    try {
      final cfg = _config;
      // Probe first; on failure ask whether to save anyway (mirrors the TUI's
      // "Connection Test Failed — Save anyway?" confirm).
      String? failure;
      try {
        failure = await _probe(cfg);
      } catch (e) {
        failure = '$e';
      }
      if (failure != null && !await _confirmSaveAnyway(failure)) return;
      await widget.onSubmit(cfg);
      if (!mounted) return;
      // Add-as-home (first run) can't pop; the settings/servers route can.
      if (Navigator.of(context).canPop()) Navigator.of(context).pop();
    } catch (e) {
      _snack('Save failed: $e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<bool> _confirmSaveAnyway(String failure) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Connection test failed'),
        content: Text('$failure\n\nSave this server anyway?'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Save anyway'),
          ),
        ],
      ),
    );
    return ok ?? false;
  }

  @override
  Widget build(BuildContext context) {
    final editing = widget.existing != null;
    return Scaffold(
      appBar: AppBar(title: Text(editing ? 'Edit server' : 'Add server')),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: Form(
          key: _formKey,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              TextFormField(
                controller: _nameController,
                decoration: const InputDecoration(
                  labelText: 'Name',
                  helperText: 'Shown as the group header (defaults to the host)',
                ),
                autocorrect: false,
              ),
              const SizedBox(height: 16),
              TextFormField(
                controller: _urlController,
                decoration: const InputDecoration(
                  labelText: 'Server URL',
                  hintText: 'http://100.x.y.z:7878',
                  helperText:
                      'Reach a 127.0.0.1 server via SSH tunnel or Tailscale',
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
                onPressed: _busy ? null : _save,
                icon: _busy
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.save),
                label: Text(editing ? 'Save' : 'Add server'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
