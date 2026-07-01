import 'package:flutter/material.dart';

import '../server_config.dart';
import '../services/commander_api.dart';

/// Form for creating a session. `projectPath` is a path on the *server's*
/// filesystem (the repo to branch from) — the API offers no project listing to
/// pick from, so it's typed here. Pops with the new session id on success.
class CreateSessionPage extends StatefulWidget {
  final CommanderApi api;
  final ServerConfig config;
  const CreateSessionPage({super.key, required this.api, required this.config});

  @override
  State<CreateSessionPage> createState() => _CreateSessionPageState();
}

class _CreateSessionPageState extends State<CreateSessionPage> {
  final _formKey = GlobalKey<FormState>();
  final _projectPath = TextEditingController();
  final _title = TextEditingController();
  final _program = TextEditingController();
  final _initialPrompt = TextEditingController();

  bool _submitting = false;

  @override
  void dispose() {
    _projectPath.dispose();
    _title.dispose();
    _program.dispose();
    _initialPrompt.dispose();
    super.dispose();
  }

  String? _emptyToNull(String s) => s.trim().isEmpty ? null : s.trim();

  Future<void> _submit() async {
    if (!_formKey.currentState!.validate()) return;
    setState(() => _submitting = true);
    try {
      final id = await widget.api.createSession(
        baseUrl: widget.config.baseUrl,
        token: widget.config.token,
        projectPath: _projectPath.text.trim(),
        title: _title.text.trim(),
        program: _emptyToNull(_program.text),
        initialPrompt: _emptyToNull(_initialPrompt.text),
        effort: null,
        mode: null,
        baseBranch: null,
      );
      if (!mounted) return;
      Navigator.of(context).pop(id);
    } catch (e) {
      if (!mounted) return;
      setState(() => _submitting = false);
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('Failed to create: $e')));
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('New session')),
      body: Form(
        key: _formKey,
        child: ListView(
          padding: const EdgeInsets.all(16),
          children: [
            TextFormField(
              controller: _projectPath,
              decoration: const InputDecoration(
                labelText: 'Project path (on the server)',
                hintText: '/home/you/Projects/my-repo',
                border: OutlineInputBorder(),
              ),
              validator: (v) =>
                  (v == null || v.trim().isEmpty) ? 'Required' : null,
            ),
            const SizedBox(height: 16),
            TextFormField(
              controller: _title,
              decoration: const InputDecoration(
                labelText: 'Title',
                border: OutlineInputBorder(),
              ),
              validator: (v) =>
                  (v == null || v.trim().isEmpty) ? 'Required' : null,
            ),
            const SizedBox(height: 16),
            TextFormField(
              controller: _program,
              decoration: const InputDecoration(
                labelText: 'Program (optional)',
                hintText: 'claude',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 16),
            TextFormField(
              controller: _initialPrompt,
              minLines: 2,
              maxLines: 5,
              decoration: const InputDecoration(
                labelText: 'Initial prompt (optional)',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 24),
            FilledButton.icon(
              onPressed: _submitting ? null : _submit,
              icon: _submitting
                  ? const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.add),
              label: const Text('Create session'),
            ),
          ],
        ),
      ),
    );
  }
}
