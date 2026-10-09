import 'package:flutter/material.dart';
import '../src/rust/api/mirrors.dart';

class SessionEdit {
  final String title;
  final String program;
  final String? section;
  final bool keepAlive;
  final bool changeBase;
  final String? parentId;
  final bool restart;
  const SessionEdit({
    required this.title,
    required this.program,
    this.section,
    required this.keepAlive,
    required this.changeBase,
    this.parentId,
    required this.restart,
  });
}

/// Edits launch settings without changing the current process until confirmed.
class EditSessionDialog extends StatefulWidget {
  final SessionInfo session;
  final List<SessionInfo> peers;
  final Future<CreateOptions> options;
  const EditSessionDialog({
    super.key,
    required this.session,
    required this.peers,
    required this.options,
  });
  @override
  State<EditSessionDialog> createState() => _EditSessionDialogState();
}

class _EditSessionDialogState extends State<EditSessionDialog> {
  final _form = GlobalKey<FormState>();
  late final _name = TextEditingController(text: widget.session.title);
  late final _program = TextEditingController(
    text: widget.session.pendingProgram ?? widget.session.program,
  );
  late String? _section = widget.session.sectionOverride;
  late final String? _originalParent = _currentParent();
  late String? _parent = _originalParent;
  String? _currentParent() {
    final branch = widget.session.prBaseBranch;
    if (branch != null) {
      for (final peer in widget.peers) {
        if (peer.projectId == widget.session.projectId &&
            peer.id != widget.session.id &&
            peer.branch == branch) {
          return peer.id;
        }
      }
      return '__current__';
    }
    return widget.session.stackParentSessionId?.field0.toString();
  }

  late bool _keepAlive = widget.session.keepAlive;
  CreateOptions? _options;
  bool _confirming = false;
  String? _optionsError;

  @override
  void initState() {
    super.initState();
    widget.options
        .then((options) {
          if (mounted) {
            setState(() => _options = options);
          }
        })
        .catchError((Object error) {
          if (mounted) {
            setState(
              () => _optionsError =
                  'Could not load program and section choices. You can still edit the program.',
            );
          }
        });
  }

  @override
  void dispose() {
    _name.dispose();
    _program.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    if (_confirming || !_form.currentState!.validate()) return;
    var restart = false;
    if (_program.text.trim() !=
        (widget.session.pendingProgram ?? widget.session.program)) {
      setState(() => _confirming = true);
      final choice = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('Restart session?'),
          content: const Text(
            'Changing the program requires a fresh restart. Restarting stops the current agent and clears its conversation.\n\nSave for later to keep the current agent running. The new program starts fresh on the next restart.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Back'),
            ),
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('No, save for next restart'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Yes, restart and clear'),
            ),
          ],
        ),
      );
      if (!mounted) return;
      setState(() => _confirming = false);
      if (choice == null) return;
      restart = choice;
    }
    if (!mounted) return;
    Navigator.pop(
      context,
      SessionEdit(
        title: _name.text.trim(),
        program: _program.text.trim(),
        section: _section,
        keepAlive: _keepAlive,
        changeBase: _parent != _originalParent,
        parentId: _parent == "__current__" ? null : _parent,
        restart: restart,
      ),
    );
  }

  String? _required(String? value) =>
      value == null || value.trim().isEmpty ? 'Required' : null;
  @override
  Widget build(BuildContext context) {
    final sections = <String>{?_section, ...?_options?.sections};
    final parents = widget.peers
        .where(
          (s) =>
              s.projectId == widget.session.projectId &&
              s.id != widget.session.id,
        )
        .toList();
    return AlertDialog(
      title: const Text('Edit session'),
      content: SizedBox(
        width: 460,
        child: SingleChildScrollView(
          child: Form(
            key: _form,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextFormField(
                  controller: _name,
                  autofocus: true,
                  decoration: const InputDecoration(labelText: 'Session name'),
                  validator: _required,
                ),
                TextFormField(
                  controller: _program,
                  decoration: const InputDecoration(labelText: 'Program'),
                  validator: _required,
                ),
                if (_options?.programs.isNotEmpty == true)
                  DropdownButtonFormField<String>(
                    isExpanded: true,
                    initialValue: null,
                    decoration: const InputDecoration(
                      labelText: 'Configured programs',
                    ),
                    items: [
                      for (final p in _options!.programs)
                        DropdownMenuItem(
                          value: p.command,
                          child: Text(p.label, overflow: TextOverflow.ellipsis),
                        ),
                    ],
                    onChanged: (value) {
                      if (value != null) setState(() => _program.text = value);
                    },
                  ),
                DropdownButtonFormField<String>(
                  isExpanded: true,
                  initialValue: _section ?? '',
                  decoration: const InputDecoration(labelText: 'Section'),
                  items: [
                    const DropdownMenuItem(value: '', child: Text('Automatic')),
                    for (final s in sections)
                      DropdownMenuItem(
                        value: s,
                        child: Text(s, overflow: TextOverflow.ellipsis),
                      ),
                  ],
                  onChanged: (value) =>
                      setState(() => _section = value == '' ? null : value),
                ),
                DropdownButtonFormField<String>(
                  isExpanded: true,
                  initialValue: _parent ?? '',
                  decoration: const InputDecoration(labelText: 'Stack base'),
                  items: [
                    const DropdownMenuItem(
                      value: '',
                      child: Text('Project base'),
                    ),
                    if (_originalParent == '__current__' ||
                        (_originalParent != null &&
                            !parents.any((p) => p.id == _originalParent)))
                      DropdownMenuItem(
                        value: _originalParent,
                        child: Text(
                          'Current base (${widget.session.prBaseBranch ?? 'session unavailable'})',
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                    for (final p in parents)
                      DropdownMenuItem(
                        value: p.id,
                        child: Text(
                          '${p.title} (${p.branch})',
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                  ],
                  onChanged: (value) =>
                      setState(() => _parent = value == '' ? null : value),
                ),
                const Padding(
                  padding: EdgeInsets.only(top: 8),
                  child: Text(
                    'Stack base changes metadata and the PR target. Git history is unchanged.',
                  ),
                ),
                SwitchListTile(
                  contentPadding: EdgeInsets.zero,
                  title: const Text('Keep alive'),
                  subtitle: const Text('Prevent automatic hibernation'),
                  value: _keepAlive,
                  onChanged: (value) => setState(() => _keepAlive = value),
                ),
                if (_optionsError != null) Text(_optionsError!),
              ],
            ),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: _confirming ? null : () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: _confirming ? null : _save,
          child: const Text('Save'),
        ),
      ],
    );
  }
}
