import 'package:flutter/material.dart';

import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';

/// Manages the server's registered projects (git repos). Lists each project's
/// name + repo path; adds one by its server-side path (`addProject`), removes one
/// (`removeProject`), scans a server-side directory for repos (`scanDirectory`),
/// and browses a project's branches on demand (`listBranches`).
///
/// Project paths are typed, not picked — the paths live on the server, not the
/// device, mirroring the TUI. The list is rendered reactively from the
/// [CommanderStore] (the change feed refreshes it after a mutation).
class ProjectsPage extends StatefulWidget {
  final CommanderStore store;

  const ProjectsPage({super.key, required this.store});

  @override
  State<ProjectsPage> createState() => _ProjectsPageState();
}

class _ProjectsPageState extends State<ProjectsPage> {
  bool _busy = false;

  CommanderStore get _store => widget.store;

  /// Prompt for a server-side path; returns the trimmed value or null on cancel.
  Future<String?> _promptPath({
    required String title,
    required String label,
  }) => showDialog<String>(
    context: context,
    builder: (_) => _PathPromptDialog(title: title, label: label),
  );

  Future<void> _addProject() async {
    final path = await _promptPath(
      title: 'Add project',
      label: 'Server-side repo path',
    );
    if (path == null || path.isEmpty) return;
    await _run(() async {
      await _store.addProject(path);
      await _store.refresh();
      _snack('Project added');
    });
  }

  Future<void> _scan() async {
    final path = await _promptPath(
      title: 'Scan directory',
      label: 'Server-side directory path',
    );
    if (path == null || path.isEmpty) return;
    await _run(() async {
      final result = await _store.scanDirectory(path);
      await _store.refresh();
      _snack('Added ${result.added}, skipped ${result.skipped}');
    });
  }

  Future<void> _remove(ProjectInfoDto project) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Remove project?'),
        content: Text(
          'Deregisters "${project.name}" from the server. '
          'The repo on disk is not touched.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: Colors.red),
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    await _run(() async {
      await _store.removeProject(project.id.field0.uuid);
      await _store.refresh();
      _snack('Project removed');
    });
  }

  /// Run a mutation with a busy guard and a failure snackbar.
  Future<void> _run(Future<void> Function() action) async {
    if (_busy) return;
    setState(() => _busy = true);
    try {
      await action();
    } catch (e) {
      _snack('Failed: $e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _snack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: _store,
      builder: (context, _) {
        final projects = _store.projects;
        return Scaffold(
          appBar: AppBar(
            title: const Text('Projects'),
            actions: [
              IconButton(
                onPressed: _busy ? null : _scan,
                icon: const Icon(Icons.travel_explore),
                tooltip: 'Scan directory',
              ),
            ],
          ),
          floatingActionButton: FloatingActionButton(
            onPressed: _busy ? null : _addProject,
            tooltip: 'Add project',
            child: const Icon(Icons.add),
          ),
          body: projects.isEmpty
              ? _emptyState()
              : ListView(
                  padding: const EdgeInsets.only(bottom: 88),
                  children: [
                    for (final p in projects)
                      _ProjectTile(
                        key: ValueKey(p.id.field0.uuid),
                        project: p,
                        store: _store,
                        onRemove: _busy ? null : () => _remove(p),
                      ),
                  ],
                ),
        );
      },
    );
  }

  Widget _emptyState() {
    return ListView(
      children: const [
        SizedBox(height: 120),
        Center(child: Icon(Icons.folder_off_outlined, size: 48)),
        SizedBox(height: 12),
        Center(child: Text('No projects — tap + to add one')),
      ],
    );
  }
}

/// One project row: name + repo path, a remove action, and a lazily-loaded
/// branch list revealed on expand (`listBranches`, local branches only).
class _ProjectTile extends StatefulWidget {
  final ProjectInfoDto project;
  final CommanderStore store;
  final VoidCallback? onRemove;

  const _ProjectTile({
    super.key,
    required this.project,
    required this.store,
    required this.onRemove,
  });

  @override
  State<_ProjectTile> createState() => _ProjectTileState();
}

class _ProjectTileState extends State<_ProjectTile> {
  List<BranchInfo>? _branches;
  Object? _error;
  bool _loading = false;

  Future<void> _loadBranches() async {
    if (_loading || _branches != null) return;
    setState(() => _loading = true);
    try {
      final branches = await widget.store.listBranches(
        widget.project.id.field0.uuid,
      );
      if (!mounted) return;
      setState(() {
        _branches = branches;
        _error = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = e);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
      child: ExpansionTile(
        title: Text(
          widget.project.name,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: Text(
          widget.project.repoPath,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: Theme.of(context).textTheme.bodySmall,
        ),
        trailing: IconButton(
          onPressed: widget.onRemove,
          icon: const Icon(Icons.delete_outline),
          tooltip: 'Remove',
        ),
        onExpansionChanged: (expanded) {
          if (expanded) _loadBranches();
        },
        children: [_branchesView(context)],
      ),
    );
  }

  Widget _branchesView(BuildContext context) {
    if (_loading) {
      return const Padding(
        padding: EdgeInsets.all(16),
        child: Center(child: CircularProgressIndicator()),
      );
    }
    if (_error != null) {
      return Padding(
        padding: const EdgeInsets.all(16),
        child: Text('Failed to load branches: $_error'),
      );
    }
    final branches = _branches;
    if (branches == null) return const SizedBox.shrink();
    if (branches.isEmpty) {
      return const Padding(
        padding: EdgeInsets.all(16),
        child: Text('No branches'),
      );
    }
    return Column(
      children: [
        for (final b in branches)
          ListTile(
            dense: true,
            leading: Icon(
              b.isRemote ? Icons.cloud_outlined : Icons.call_split,
              size: 18,
            ),
            title: Text(b.name),
          ),
      ],
    );
  }
}

/// A path-prompt dialog that owns its controller and disposes it with its route.
/// Pops with the trimmed text on confirm, or null on cancel.
class _PathPromptDialog extends StatefulWidget {
  final String title;
  final String label;

  const _PathPromptDialog({required this.title, required this.label});

  @override
  State<_PathPromptDialog> createState() => _PathPromptDialogState();
}

class _PathPromptDialogState extends State<_PathPromptDialog> {
  final _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _submit() => Navigator.of(context).pop(_controller.text.trim());

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.title),
      content: TextField(
        controller: _controller,
        autofocus: true,
        decoration: InputDecoration(labelText: widget.label),
        onSubmitted: (_) => _submit(),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(onPressed: _submit, child: const Text('OK')),
      ],
    );
  }
}
