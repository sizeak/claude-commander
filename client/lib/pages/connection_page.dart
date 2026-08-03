import 'package:flutter/material.dart';
import 'package:uuid/uuid.dart';

import '../server_config.dart';
import '../services/commander_api.dart';
import '../theme/app_colors.dart';
import '../theme/app_theme.dart';
import '../widgets/brand_mark.dart';

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

  /// The health-ok banner text after a successful [_test], or null when there's
  /// no fresh success to show (never tested, failed, or a field changed since).
  String? _healthOk;

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
    setState(() {
      _busy = true;
      _healthOk = null;
    });
    try {
      final problem = await _probe(_config);
      if (!mounted) return;
      setState(() => _healthOk = problem == null ? '/health ok · tmux healthy' : null);
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
    final canPop = Navigator.of(context).canPop();
    return Scaffold(
      body: SafeArea(
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 460),
            child: Form(
              key: _formKey,
              child: ListView(
                padding: const EdgeInsets.fromLTRB(26, 20, 26, 26),
                children: [
                  Row(
                    children: [
                      const BrandMark(size: 44),
                      const Spacer(),
                      if (canPop)
                        IconButton(
                          onPressed: () => Navigator.of(context).pop(),
                          icon: const Icon(Icons.close),
                          tooltip: 'Close',
                        ),
                    ],
                  ),
                  const SizedBox(height: 28),
                  Text(
                    'CLAUDE COMMANDER',
                    style: AppTheme.mono(
                      size: 11,
                      weight: FontWeight.w600,
                      color: AppColors.textMuted,
                      letterSpacing: 2,
                    ),
                  ),
                  const SizedBox(height: 6),
                  Text(
                    editing ? 'Edit server' : 'Connect to a server',
                    style: const TextStyle(
                      fontFamily: AppFonts.sans,
                      fontSize: 26,
                      fontWeight: FontWeight.w600,
                      letterSpacing: -0.3,
                      color: AppColors.text,
                    ),
                  ),
                  const SizedBox(height: 8),
                  _field(
                    key: const Key('nameField'),
                    controller: _nameController,
                    label: 'NAME',
                    hint: 'laptop (defaults to the host)',
                    autocorrect: false,
                  ),
                  _field(
                    key: const Key('urlField'),
                    controller: _urlController,
                    label: 'SERVER URL',
                    hint: 'http://100.x.y.z:7878',
                    helper: 'Reach a 127.0.0.1 server via SSH tunnel or Tailscale',
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
                  _field(
                    key: const Key('tokenField'),
                    controller: _tokenController,
                    label: 'ACCESS TOKEN',
                    hint: 'The server prints this on first run',
                    obscureText: true,
                    autocorrect: false,
                    enableSuggestions: false,
                    validator: (v) =>
                        (v?.trim().isEmpty ?? true) ? 'Required' : null,
                  ),
                  if (_healthOk != null) ...[
                    const SizedBox(height: 16),
                    _healthBanner(_healthOk!),
                  ],
                  const SizedBox(height: 20),
                  OutlinedButton.icon(
                    onPressed: _busy ? null : _test,
                    icon: const Icon(Icons.wifi_tethering, size: 18),
                    label: const Text('Test connection'),
                  ),
                  const SizedBox(height: 12),
                  FilledButton(
                    onPressed: _busy ? null : _save,
                    child: _busy
                        ? const SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : Text(editing ? 'Save' : 'Connect'),
                  ),
                  const SizedBox(height: 14),
                  Center(
                    child: Text(
                      'TOKEN STORED IN KEYCHAIN',
                      style: AppTheme.mono(
                        size: 10.5,
                        weight: FontWeight.w500,
                        color: AppColors.textFaint,
                        letterSpacing: 0.6,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  /// A mono-labelled input row: an uppercase label above a dark, mono-text field
  /// (plus an optional muted helper caption). Clearing [_healthOk] on any edit
  /// keeps a stale success banner from lingering past a changed value.
  Widget _field({
    required Key key,
    required TextEditingController controller,
    required String label,
    String? hint,
    String? helper,
    bool obscureText = false,
    bool autocorrect = true,
    bool enableSuggestions = true,
    TextInputType? keyboardType,
    String? Function(String?)? validator,
  }) {
    return Padding(
      padding: const EdgeInsets.only(top: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(bottom: 7),
            child: Text(
              label,
              style: AppTheme.mono(
                size: 11,
                weight: FontWeight.w500,
                color: AppColors.textMuted,
              ),
            ),
          ),
          TextFormField(
            key: key,
            controller: controller,
            style: AppTheme.mono(
              size: 13,
              weight: FontWeight.w500,
              color: AppColors.text,
              letterSpacing: obscureText ? 1.5 : null,
            ),
            decoration: InputDecoration(
              hintText: hint,
              hintStyle: AppTheme.mono(size: 13, color: AppColors.textFaint),
              helperText: helper,
              helperStyle: AppTheme.mono(size: 10.5, color: AppColors.textFaint),
              helperMaxLines: 2,
            ),
            obscureText: obscureText,
            autocorrect: autocorrect,
            enableSuggestions: enableSuggestions,
            keyboardType: keyboardType,
            validator: validator,
            onChanged: _healthOk == null
                ? null
                : (_) => setState(() => _healthOk = null),
          ),
        ],
      ),
    );
  }

  Widget _healthBanner(String text) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: AppColors.teal.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: AppColors.teal.withValues(alpha: 0.25)),
      ),
      child: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(
              color: AppColors.teal,
              shape: BoxShape.circle,
              boxShadow: [
                BoxShadow(color: AppColors.teal.withValues(alpha: 0.6), blurRadius: 8),
              ],
            ),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              text,
              style: AppTheme.mono(
                size: 12,
                weight: FontWeight.w500,
                color: AppColors.teal,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
