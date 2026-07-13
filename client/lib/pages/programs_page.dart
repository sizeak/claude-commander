import 'package:flutter/material.dart';

import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';

/// Edits the server's launch-program list (`PUT /api/config/programs`). Each row
/// is a `{label, command}` pair the create-session form offers as a choice.
///
/// Loads the current list from `create-options` (the same source the create form
/// uses), lets the user add / edit / remove / reorder rows, and saves the whole
/// list back with `setPrograms`. Blank rows (empty label AND command) are dropped
/// on save so an accidental empty row is harmless.
class ProgramsPage extends StatefulWidget {
  final CommanderApi api;
  final String handle;

  const ProgramsPage({super.key, required this.api, required this.handle});

  @override
  State<ProgramsPage> createState() => _ProgramsPageState();
}

/// A single editable program row: a pair of controllers owned by the page so
/// edits survive rebuilds/reorders. Disposed with the page.
class _Row {
  final TextEditingController label;
  final TextEditingController command;
  _Row(ProgramInfo p)
    : label = TextEditingController(text: p.label),
      command = TextEditingController(text: p.command);

  void dispose() {
    label.dispose();
    command.dispose();
  }

  ProgramInfo toInfo() =>
      ProgramInfo(label: label.text.trim(), command: command.text.trim());

  bool get isBlank => label.text.trim().isEmpty && command.text.trim().isEmpty;
}

class _ProgramsPageState extends State<ProgramsPage> {
  List<_Row>? _rows;
  Object? _loadError;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    for (final r in _rows ?? const <_Row>[]) {
      r.dispose();
    }
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final opts = await widget.api.createOptions(handle: widget.handle);
      if (!mounted) return;
      setState(() {
        _rows = opts.programs.map(_Row.new).toList();
        _loadError = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _loadError = e);
    }
  }

  void _add() {
    setState(
      () => (_rows ??= []).add(_Row(const ProgramInfo(label: '', command: ''))),
    );
  }

  void _removeAt(int i) {
    setState(() => _rows!.removeAt(i).dispose());
  }

  Future<void> _save() async {
    final rows = _rows ?? const <_Row>[];
    final programs = [
      for (final r in rows)
        if (!r.isBlank) r.toInfo(),
    ];
    setState(() => _saving = true);
    try {
      await widget.api.setPrograms(handle: widget.handle, programs: programs);
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Programs saved')),
      );
      Navigator.of(context).pop();
    } catch (e) {
      if (!mounted) return;
      setState(() => _saving = false);
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('Save failed: $e')));
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Programs'),
        actions: [
          if (_rows != null)
            _saving
                ? const Padding(
                    padding: EdgeInsets.all(16),
                    child: SizedBox(
                      width: 20,
                      height: 20,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    ),
                  )
                : IconButton(
                    onPressed: _save,
                    icon: const Icon(Icons.save),
                    tooltip: 'Save',
                  ),
        ],
      ),
      floatingActionButton: _rows == null
          ? null
          : FloatingActionButton(
              onPressed: _add,
              tooltip: 'Add program',
              child: const Icon(Icons.add),
            ),
      body: _body(context),
    );
  }

  Widget _body(BuildContext context) {
    if (_loadError != null) {
      return _ErrorView(error: _loadError.toString(), onRetry: _load);
    }
    final rows = _rows;
    if (rows == null) {
      return const Center(child: CircularProgressIndicator());
    }
    if (rows.isEmpty) {
      return ListView(
        children: const [
          SizedBox(height: 120),
          Center(child: Icon(Icons.list_alt_outlined, size: 48)),
          SizedBox(height: 12),
          Center(child: Text('No programs — tap + to add one')),
        ],
      );
    }
    // ReorderableListView so the order (which the create form presents) is
    // editable; the first entry is effectively the default suggestion.
    return ReorderableListView.builder(
      padding: const EdgeInsets.only(bottom: 88),
      itemCount: rows.length,
      onReorder: (from, to) {
        setState(() {
          if (to > from) to -= 1;
          final r = rows.removeAt(from);
          rows.insert(to, r);
        });
      },
      itemBuilder: (context, i) => _ProgramRowTile(
        key: ObjectKey(rows[i]),
        row: rows[i],
        index: i,
        onRemove: () => _removeAt(i),
      ),
    );
  }
}

class _ProgramRowTile extends StatelessWidget {
  final _Row row;
  final int index;
  final VoidCallback onRemove;

  const _ProgramRowTile({
    super.key,
    required this.row,
    required this.index,
    required this.onRemove,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
      child: Card(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(12, 8, 8, 8),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              Expanded(
                child: Column(
                  children: [
                    TextField(
                      controller: row.label,
                      decoration: const InputDecoration(
                        labelText: 'Label',
                        isDense: true,
                      ),
                    ),
                    const SizedBox(height: 8),
                    TextField(
                      controller: row.command,
                      decoration: const InputDecoration(
                        labelText: 'Command',
                        isDense: true,
                      ),
                    ),
                  ],
                ),
              ),
              IconButton(
                onPressed: onRemove,
                icon: const Icon(Icons.delete_outline),
                tooltip: 'Remove',
              ),
              ReorderableDragStartListener(
                index: index,
                child: const Padding(
                  padding: EdgeInsets.symmetric(horizontal: 4),
                  child: Icon(Icons.drag_handle),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _ErrorView extends StatelessWidget {
  final String error;
  final Future<void> Function() onRetry;
  const _ErrorView({required this.error, required this.onRetry});

  @override
  Widget build(BuildContext context) {
    return ListView(
      children: [
        const SizedBox(height: 80),
        Icon(
          Icons.cloud_off,
          size: 48,
          color: Theme.of(context).colorScheme.error,
        ),
        const SizedBox(height: 12),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24),
          child: Text(error, textAlign: TextAlign.center),
        ),
        const SizedBox(height: 16),
        Center(
          child: FilledButton.icon(
            onPressed: onRetry,
            icon: const Icon(Icons.refresh),
            label: const Text('Retry'),
          ),
        ),
      ],
    );
  }
}
