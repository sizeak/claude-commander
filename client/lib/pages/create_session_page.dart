import 'package:flutter/material.dart';

import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import 'projects_page.dart';

/// Form for creating a session. The project is picked from the server's
/// registered projects (a dropdown of [CommanderStore.projects]); its repo path
/// is what the session branches from. The body rebuilds with the store, so if
/// the workspace snapshot is still loading when the page opens, the picker fills
/// in as soon as the projects arrive rather than stranding on an empty state. If
/// the server genuinely has no projects, the page offers a jump to the projects
/// manager to add one.
///
/// The program and section come from the server's `create-options` (a program
/// dropdown replaces the old free-text field; section is optional). If
/// create-options can't be loaded, the program falls back to a free-text field so
/// creation still works. `section` isn't a create-session parameter, so a chosen
/// section is applied with `setSection` right after the session is created. Pops
/// with the new session id on success.
class CreateSessionPage extends StatefulWidget {
  final CommanderStore store;
  const CreateSessionPage({super.key, required this.store});

  @override
  State<CreateSessionPage> createState() => _CreateSessionPageState();
}

class _CreateSessionPageState extends State<CreateSessionPage> {
  final _formKey = GlobalKey<FormState>();
  final _title = TextEditingController();
  final _program = TextEditingController(); // fallback when options don't load

  CreateOptions? _options;
  bool _loadingOptions = true;
  String? _selectedProject; // a project repo path, or null
  String? _selectedProgram; // a program command, or null
  String? _selectedSection; // a section name, or null (= no section)
  bool _submitting = false;

  CommanderApi get _api => widget.store.api;
  String get _handle => widget.store.handle!;

  @override
  void initState() {
    super.initState();
    // The project selection is reconciled against the live list in build (which
    // also preselects the first project), so nothing to seed here.
    _loadOptions();
  }

  @override
  void dispose() {
    _title.dispose();
    _program.dispose();
    super.dispose();
  }

  Future<void> _loadOptions() async {
    try {
      final opts = await _api.createOptions(handle: _handle);
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
    final projectPath = _selectedProject;
    if (projectPath == null) return;
    setState(() => _submitting = true);

    // Step 1: create. A failure here means no session exists — report it and
    // re-enable the form so the user can retry.
    final String id;
    try {
      id = await _api.createSession(
        handle: _handle,
        projectPath: projectPath,
        title: _title.text.trim(),
        program: _programValue(),
        initialPrompt: null,
        effort: null,
        mode: null,
        baseBranch: null,
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
        await _api.setSection(handle: _handle, id: id, section: section);
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
      // Rebuild with the store so a still-loading workspace fills the picker in
      // as soon as its snapshot lands, rather than stranding on the empty state.
      body: ListenableBuilder(
        listenable: widget.store,
        builder: (context, _) {
          final projects = widget.store.projects;
          if (projects.isEmpty) return _noProjects();
          // Keep the selection valid: preselect the first project, and recover
          // if the chosen one vanished from the live list (a dropdown whose
          // value isn't among its items throws). Safe to assign during build —
          // it triggers no further build on its own.
          if (!projects.any((p) => p.repoPath == _selectedProject)) {
            _selectedProject = projects.first.repoPath;
          }
          return Form(
            key: _formKey,
            child: ListView(
              padding: const EdgeInsets.all(16),
              children: [
                _projectField(projects),
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
                if (_sectionOptions().isNotEmpty) ...[
                  const SizedBox(height: 16),
                  _sectionField(),
                ],
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
          );
        },
      ),
    );
  }

  Widget _noProjects() {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Icon(Icons.folder_off_outlined, size: 48),
          const SizedBox(height: 12),
          const Text('No projects registered on the server'),
          const SizedBox(height: 16),
          // The store is right here, so let the user jump straight to the
          // projects manager and add one; the picker fills in when they return.
          FilledButton.icon(
            onPressed: () => Navigator.of(context).push(
              MaterialPageRoute(
                builder: (_) => ProjectsPage(store: widget.store),
              ),
            ),
            icon: const Icon(Icons.folder_open),
            label: const Text('Manage projects'),
          ),
        ],
      ),
    );
  }

  Widget _projectField(List<ProjectInfoDto> projects) {
    return DropdownButtonFormField<String>(
      // Re-key on the selection so the field adopts the reconciled value when a
      // project is picked or the selected one disappears from the live list.
      key: ValueKey(_selectedProject),
      initialValue: _selectedProject,
      isExpanded: true,
      itemHeight: null, // allow two-line items (name + path)
      decoration: const InputDecoration(
        labelText: 'Project',
        border: OutlineInputBorder(),
      ),
      // The closed field shows just the project name (one line); the open menu
      // adds the repo path so same-named repos are distinguishable.
      selectedItemBuilder: (context) => [
        for (final p in projects)
          Align(
            alignment: Alignment.centerLeft,
            child: Text(p.name, maxLines: 1, overflow: TextOverflow.ellipsis),
          ),
      ],
      items: [
        for (final p in projects)
          DropdownMenuItem(
            value: p.repoPath,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(p.name, maxLines: 1, overflow: TextOverflow.ellipsis),
                Text(
                  p.repoPath,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ],
            ),
          ),
      ],
      validator: (v) => v == null ? 'Required' : null,
      onChanged: (v) => setState(() => _selectedProject = v),
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
