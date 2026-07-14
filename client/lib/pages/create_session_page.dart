import 'package:flutter/material.dart';

import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';

/// Form for creating a session. `projectPath` is a path on the *server's*
/// filesystem (the repo to branch from) — the API offers no project listing to
/// pick from, so it's typed here.
///
/// The program and section come from the server's `create-options` (a program
/// dropdown replaces the old free-text field; section is optional). If
/// create-options can't be loaded, the program falls back to a free-text field so
/// creation still works. `section` isn't a create-session parameter, so a chosen
/// section is applied with `setSection` right after the session is created. Pops
/// with the new session id on success.
class CreateSessionPage extends StatefulWidget {
  final CommanderApi api;
  final String handle;
  const CreateSessionPage({super.key, required this.api, required this.handle});

  @override
  State<CreateSessionPage> createState() => _CreateSessionPageState();
}

class _CreateSessionPageState extends State<CreateSessionPage> {
  final _formKey = GlobalKey<FormState>();
  final _projectPath = TextEditingController();
  final _title = TextEditingController();
  final _program = TextEditingController(); // fallback when options don't load
  final _baseBranch = TextEditingController();
  final _initialPrompt = TextEditingController();

  CreateOptions? _options;
  bool _loadingOptions = true;
  String? _selectedProgram; // a program command, or null
  String? _selectedSection; // a section name, or null (= no section)
  bool _submitting = false;

  @override
  void initState() {
    super.initState();
    _loadOptions();
  }

  @override
  void dispose() {
    _projectPath.dispose();
    _title.dispose();
    _program.dispose();
    _baseBranch.dispose();
    _initialPrompt.dispose();
    super.dispose();
  }

  Future<void> _loadOptions() async {
    try {
      final opts = await widget.api.createOptions(handle: widget.handle);
      if (!mounted) return;
      setState(() {
        _options = opts;
        _loadingOptions = false;
        // Preselect the server's default program if it's in the list.
        final hasDefault = opts.programs.any(
          (p) => p.command == opts.defaultProgram,
        );
        _selectedProgram = hasDefault
            ? opts.defaultProgram
            : (opts.programs.isNotEmpty ? opts.programs.first.command : null);
      });
    } catch (_) {
      // Options are a convenience; on failure fall back to the free-text field.
      if (!mounted) return;
      setState(() => _loadingOptions = false);
    }
  }

  String? _emptyToNull(String s) => s.trim().isEmpty ? null : s.trim();

  /// The program to send: the dropdown selection when options loaded, else the
  /// free-text field.
  String? _programValue() {
    if (_options != null && _options!.programs.isNotEmpty) {
      return _selectedProgram;
    }
    return _emptyToNull(_program.text);
  }

  Future<void> _submit() async {
    if (!_formKey.currentState!.validate()) return;
    setState(() => _submitting = true);

    // Step 1: create. A failure here means no session exists — report it and
    // re-enable the form so the user can retry.
    final String id;
    try {
      id = await widget.api.createSession(
        handle: widget.handle,
        projectPath: _projectPath.text.trim(),
        title: _title.text.trim(),
        program: _programValue(),
        initialPrompt: _emptyToNull(_initialPrompt.text),
        effort: null,
        mode: null,
        baseBranch: _emptyToNull(_baseBranch.text),
      );
    } catch (e) {
      if (!mounted) return;
      setState(() => _submitting = false);
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('Failed to create: $e')));
      return;
    }

    // Step 2: the session now EXISTS — it's committed. `section` isn't a create
    // parameter, so apply it best-effort; a failure here must NOT be reported as
    // a create failure or re-arm the button (that would risk a duplicate
    // session). Surface it as a non-fatal warning and still return the new id.
    final section = _selectedSection;
    if (section != null && section.isNotEmpty) {
      try {
        await widget.api.setSection(
          handle: widget.handle,
          id: id,
          section: section,
        );
      } catch (e) {
        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('Session created; setting section failed: $e')),
          );
        }
      }
    }
    if (!mounted) return;
    Navigator.of(context).pop(id);
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
            _programField(),
            const SizedBox(height: 16),
            if (_sectionOptions().isNotEmpty) ...[
              _sectionField(),
              const SizedBox(height: 16),
            ],
            TextFormField(
              controller: _baseBranch,
              decoration: const InputDecoration(
                labelText: 'Base branch (optional)',
                hintText: 'defaults to the repo default',
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

  Widget _programField() {
    if (_loadingOptions) {
      return const InputDecorator(
        decoration: InputDecoration(
          labelText: 'Program',
          border: OutlineInputBorder(),
        ),
        child: Row(
          children: [
            SizedBox(
              width: 16,
              height: 16,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            SizedBox(width: 12),
            Text('Loading…'),
          ],
        ),
      );
    }
    final programs = _options?.programs ?? const <ProgramInfo>[];
    if (programs.isEmpty) {
      // Fallback: no options → free-text (keeps creation working offline).
      return TextFormField(
        controller: _program,
        decoration: const InputDecoration(
          labelText: 'Program (optional)',
          hintText: 'claude',
          border: OutlineInputBorder(),
        ),
      );
    }
    return DropdownButtonFormField<String>(
      initialValue: _selectedProgram,
      decoration: const InputDecoration(
        labelText: 'Program',
        border: OutlineInputBorder(),
      ),
      items: [
        for (final p in programs)
          DropdownMenuItem(value: p.command, child: Text(p.label)),
      ],
      onChanged: (v) => setState(() => _selectedProgram = v),
    );
  }

  List<String> _sectionOptions() => _options?.sections ?? const [];

  Widget _sectionField() {
    return DropdownButtonFormField<String?>(
      initialValue: _selectedSection,
      decoration: const InputDecoration(
        labelText: 'Section (optional)',
        border: OutlineInputBorder(),
      ),
      items: [
        const DropdownMenuItem(value: null, child: Text('None')),
        for (final s in _sectionOptions())
          DropdownMenuItem(value: s, child: Text(s)),
      ],
      onChanged: (v) => setState(() => _selectedSection = v),
    );
  }
}
