import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../chrome/chrome.dart';
import '../chrome/chrome_forms.dart';
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/workspace.dart';
import '../state/commander_store.dart';
import '../state/fleet_store.dart';
import '../theme/tokens.dart';
import '../util/workspace_color.dart';

/// The key of a workspace's row on [WorkspacesPage] ([name] null = Main), so a
/// test can find one row among several that share text.
Key workspaceRowKey(String? name) => ValueKey('workspace-row:${name ?? ''}');

/// Settings → PROJECTS → Workspaces: create, rename, delete, reorder and colour
/// the workspaces, pick the one the app opens on, and move projects between
/// them.
///
/// Every edit to the list goes to **every** connected server at once
/// ([FleetStore]'s fan-out), because each server stores the definitions for its
/// own projects and the app shows them merged by name. It is best effort: a
/// server that refuses (or is unreachable) is named in a snackbar and the rest
/// keep the edit. Moving a project is the exception — it writes only to the
/// server that owns the project, which defines the workspace for itself if it
/// had to.
///
/// The page owns no workspace state: it renders [FleetStore.workspaces] and
/// re-renders when the fan-out's refreshes land.
class WorkspacesPage extends StatefulWidget {
  final FleetStore fleet;

  const WorkspacesPage({super.key, required this.fleet});

  @override
  State<WorkspacesPage> createState() => _WorkspacesPageState();
}

/// What a workspace row's action sheet offers.
enum _RowAction { rename, colour, moveUp, moveDown, delete }

class _WorkspacesPageState extends State<WorkspacesPage> {
  bool _busy = false;

  FleetStore get _fleet => widget.fleet;

  /// The user workspaces' names in display order (Main excluded — it has no
  /// name and always leads).
  List<String> get _userNames => [for (final w in _fleet.workspaces) ?w.name];

  /// Run a fleet edit with a busy guard, then name any server that refused it.
  Future<void> _edit(
    Future<List<WorkspaceEditFailure>> Function() action,
  ) async {
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final failures = await action();
      final message = describeWorkspaceFailures(failures);
      if (message != null) _snack(message);
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

  /// Why [raw] cannot name a workspace other than [except] (the one being
  /// renamed), or null when it can. Both halves are the shared rules, through
  /// the bridge: the server's own name check, and the one only the merged list
  /// can answer — not already taken by a workspace or Main's label, ignoring
  /// case (`viewmodel::workspace_name_taken`, which the TUI uses too).
  String? _nameError(String raw, {MergedWorkspace? except, bool main = false}) {
    final api = _fleet.api;
    final ruleError = main
        ? api.workspaceLabelError(raw)
        : api.workspaceNameError(raw);
    if (ruleError != null) return ruleError;
    if (api.workspaceNameTaken(_fleet.workspaces, raw, except: except)) {
      return 'A workspace named "${raw.trim()}" already exists';
    }
    return null;
  }

  Future<String?> _promptName({
    required String title,
    String initial = '',
    required String? Function(String) validate,
  }) => showDialog<String>(
    context: context,
    builder: (_) =>
        _NameDialog(title: title, initial: initial, validate: validate),
  );

  Future<void> _create() async {
    final name = await _promptName(
      title: 'New workspace',
      validate: (raw) => _nameError(raw),
    );
    if (name == null) return;
    await _edit(() => _fleet.createWorkspace(name));
  }

  Future<void> _rowActions(MergedWorkspace w) async {
    final names = _userNames;
    final index = w.name == null ? -1 : names.indexOf(w.name!);
    final t = CommanderTokens.of(context);
    final action = await showModalBottomSheet<_RowAction>(
      context: context,
      backgroundColor: t.canvasRaised,
      builder: (sheetContext) {
        Widget tile(IconData icon, String label, _RowAction value) => ListTile(
          leading: Icon(icon, color: t.primary),
          title: Text(label),
          onTap: () => Navigator.of(sheetContext).pop(value),
        );
        return SafeArea(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              tile(Icons.edit_outlined, 'Rename', _RowAction.rename),
              tile(Icons.palette_outlined, 'Colour', _RowAction.colour),
              // Main always leads and cannot be deleted: it is where untagged
              // projects live, on every server.
              if (index > 0)
                tile(Icons.arrow_upward, 'Move up', _RowAction.moveUp),
              if (index >= 0 && index < names.length - 1)
                tile(Icons.arrow_downward, 'Move down', _RowAction.moveDown),
              if (index >= 0)
                tile(Icons.delete_outline, 'Delete', _RowAction.delete),
            ],
          ),
        );
      },
    );
    if (action == null || !mounted) return;
    switch (action) {
      case _RowAction.rename:
        await _rename(w);
      case _RowAction.colour:
        await _recolour(w);
      case _RowAction.moveUp:
        await _move(names, index, index - 1);
      case _RowAction.moveDown:
        await _move(names, index, index + 1);
      case _RowAction.delete:
        await _delete(w.name!);
    }
  }

  Future<void> _rename(MergedWorkspace w) async {
    final from = w.name;
    final to = await _promptName(
      title: from == null ? 'Rename Main' : 'Rename workspace',
      initial: w.label,
      validate: (raw) => _nameError(raw, except: w, main: from == null),
    );
    if (to == null || to == w.label) return;
    await _edit(
      () => from == null
          ? _fleet.renameMainWorkspace(to)
          : _fleet.renameWorkspace(from, to),
    );
  }

  Future<void> _recolour(MergedWorkspace w) async {
    final choice = await showDialog<_ColourChoice>(
      context: context,
      builder: (_) => _ColourDialog(
        current: w.color,
        validate: _fleet.api.workspaceColorError,
      ),
    );
    if (choice == null) return;
    await _edit(() => _fleet.setWorkspaceColor(w.name, choice.hex));
  }

  Future<void> _move(List<String> names, int from, int to) async {
    final reordered = List.of(names);
    final moved = reordered.removeAt(from);
    reordered.insert(to, moved);
    await _edit(() => _fleet.reorderWorkspaces(reordered));
  }

  Future<void> _delete(String name) async {
    final t = CommanderTokens.of(context);
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Delete workspace?'),
        content: Text(
          'Deleting "$name" moves its projects to Main on every server. '
          'No project or session is removed.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(
              backgroundColor: t.danger,
              foregroundColor: t.canvas,
            ),
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('Delete'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    await _edit(() => _fleet.deleteWorkspace(name));
  }

  /// The startup choice's display form: `last` reads as "Last used", `main` as
  /// Main's label, anything else is a workspace name.
  String _startupLabel(String startup) => switch (startup.toLowerCase()) {
    'last' => 'Last used',
    'main' => _fleet.workspaces.first.label,
    _ => startup,
  };

  Future<void> _pickStartup() async {
    final current = _fleet.startupWorkspace;
    final options = ['last', 'main', for (final n in _userNames) n];
    final picked = await _pickFromSheet<String>(
      title: 'Open the app on',
      options: [
        for (final o in options)
          (value: o, label: _startupLabel(o), selected: o == current),
      ],
    );
    if (picked == null || picked == current) return;
    await _edit(() => _fleet.setStartupWorkspace(picked));
  }

  Future<void> _moveProject(CommanderStore store, ProjectInfoDto p) async {
    final picked = await _pickFromSheet<MergedWorkspace>(
      title: 'Move ${p.name} to',
      options: [
        for (final w in _fleet.workspaces)
          (value: w, label: w.label, selected: w.name == p.workspace),
      ],
    );
    if (picked == null || picked.name == p.workspace) return;
    await _edit(() => _fleet.moveProject(store, p.id.field0.uuid, picked.name));
  }

  /// A bottom sheet of choices with the current one ticked; pops the chosen
  /// value, or null when dismissed.
  Future<T?> _pickFromSheet<T>({
    required String title,
    required List<({T value, String label, bool selected})> options,
  }) {
    final t = CommanderTokens.of(context);
    return showModalBottomSheet<T>(
      context: context,
      backgroundColor: t.canvasRaised,
      isScrollControlled: true,
      builder: (sheetContext) => SafeArea(
        child: ListView(
          shrinkWrap: true,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 14, 16, 4),
              child: Text(title, style: t.meta(size: 11, color: t.textMuted)),
            ),
            for (final o in options)
              ListTile(
                title: Text(o.label),
                trailing: o.selected
                    ? Icon(Icons.check, size: 18, color: t.primary)
                    : null,
                onTap: () => Navigator.of(sheetContext).pop(o.value),
              ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: _fleet,
      builder: (context, _) {
        final workspaces = _fleet.workspaces;
        final labels = {for (final w in workspaces) w.name: w.label};
        final projects = [
          for (final store in _fleet.servers)
            for (final p in store.projects) (store: store, project: p),
        ];
        return ChromePage(
          title: 'Workspaces',
          code: '47-W',
          primaryAction: ChromeButtonAction(
            icon: Icons.add,
            label: 'New workspace',
            onPressed: _busy ? null : _create,
          ),
          body: ListView(
            padding: const EdgeInsets.fromLTRB(14, 4, 14, 88),
            children: [
              const ChromeEyebrow('WORKSPACES'),
              for (final w in workspaces)
                _Row(
                  key: workspaceRowKey(w.name),
                  label: w.label,
                  caption: _workspaceCaption(w),
                  color: parseWorkspaceColor(w.color),
                  trailingTooltip: 'Workspace actions',
                  onTap: _busy ? null : () => _rowActions(w),
                ),
              const SizedBox(height: 16),
              const ChromeEyebrow('STARTUP'),
              _Row(
                label: 'Startup workspace',
                caption: _startupLabel(_fleet.startupWorkspace),
                onTap: _busy ? null : _pickStartup,
              ),
              const SizedBox(height: 16),
              const ChromeEyebrow('PROJECTS'),
              if (projects.isEmpty)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 12),
                  child: Text('No projects on a connected server'),
                ),
              for (final (:store, :project) in projects)
                _Row(
                  key: ValueKey('workspace-project:${project.id.field0.uuid}'),
                  label: project.name,
                  caption:
                      '${store.config.name} · '
                      '${labels[project.workspace] ?? project.workspace ?? ''}',
                  color: parseWorkspaceColor(
                    workspaces
                        .where((w) => w.name == project.workspace)
                        .firstOrNull
                        ?.color,
                  ),
                  onTap: _busy || store.handle == null
                      ? null
                      : () => _moveProject(store, project),
                ),
            ],
          ),
        );
      },
    );
  }

  /// How many projects (across every server) a workspace holds.
  String _workspaceCaption(MergedWorkspace w) {
    var count = 0;
    for (final store in _fleet.servers) {
      count += store.projectsIn(w.name).length;
    }
    final noun = count == 1 ? 'project' : 'projects';
    return w.name == null ? '$count $noun · built in' : '$count $noun';
  }
}

/// One row: an optional colour dot, a label, a mono caption, and a trailing
/// affordance. Built on [ChromePanel] so each theme frames it its own way, as
/// the settings rows are.
class _Row extends StatelessWidget {
  final String label;
  final String caption;
  final Color? color;

  /// When set, the trailing affordance is an overflow button with this tooltip
  /// (doing what a tap on the row does); otherwise a chevron.
  final String? trailingTooltip;
  final VoidCallback? onTap;

  const _Row({
    super.key,
    required this.label,
    required this.caption,
    this.color,
    this.trailingTooltip,
    this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final enabled = onTap != null;
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: ChromePanel(
        ChromePanelSpec(
          accent: color,
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 11),
          onTap: onTap,
          child: Row(
            children: [
              Container(
                width: 10,
                height: 10,
                decoration: BoxDecoration(
                  color: color,
                  shape: BoxShape.circle,
                  border: color == null
                      ? Border.all(color: t.textFaint, width: 1)
                      : null,
                ),
              ),
              const SizedBox(width: 11),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      t.caseLabel(label),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontFamily: t.sans,
                        fontSize: 13.5,
                        fontWeight: FontWeight.w600,
                        letterSpacing: t.uppercaseLabels ? 0.6 : -0.1,
                        color: enabled ? t.text : t.textDim,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      caption,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: t.meta(
                        size: 10,
                        color: enabled ? t.textMuted : t.textDim,
                      ),
                    ),
                  ],
                ),
              ),
              if (trailingTooltip case final tooltip?)
                IconButton(
                  onPressed: onTap,
                  tooltip: tooltip,
                  visualDensity: VisualDensity.compact,
                  icon: Icon(Icons.more_vert, size: 18, color: t.textFaint),
                )
              else if (enabled) ...[
                const SizedBox(width: 8),
                Icon(Icons.chevron_right, size: 18, color: t.textFaint),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// A name prompt that checks the name as it is typed and will not submit one
/// that [validate] refuses. Pops the trimmed name, or null on cancel.
class _NameDialog extends StatefulWidget {
  final String title;
  final String initial;
  final String? Function(String raw) validate;

  const _NameDialog({
    required this.title,
    required this.initial,
    required this.validate,
  });

  @override
  State<_NameDialog> createState() => _NameDialogState();
}

class _NameDialogState extends State<_NameDialog> {
  late final _controller = TextEditingController(text: widget.initial);

  /// Only shown once something has been typed: an empty field on open is not
  /// yet a mistake.
  bool _touched = false;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  String? get _error => widget.validate(_controller.text);

  void _submit() {
    setState(() => _touched = true);
    if (_error != null) return;
    Navigator.of(context).pop(_controller.text.trim());
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.title),
      content: TextField(
        controller: _controller,
        autofocus: true,
        decoration: InputDecoration(
          labelText: 'Name',
          errorText: _touched ? _error : null,
        ),
        onChanged: (_) => setState(() => _touched = true),
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

/// A colour pick: a `#rrggbb` hex, or null to clear the colour.
class _ColourChoice {
  final String? hex;
  const _ColourChoice(this.hex);
}

/// The active theme's colours as swatches, a hex field (typed or pasted), and
/// "No colour". Pops a [_ColourChoice], or null on dismiss.
///
/// The field is the single source of truth: tapping a swatch writes its hex
/// into it, and the swatch whose hex the field holds is the selected one — so
/// a pasted hex that happens to be a theme colour selects that swatch too.
class _ColourDialog extends StatefulWidget {
  final String? current;

  /// Why a normalised `#rrggbb` is refused, or null — the wire rule, through
  /// the bridge ([CommanderApi.workspaceColorError]), so a fake can stand in
  /// under `flutter test`.
  final String? Function(String hex) validate;

  const _ColourDialog({required this.current, required this.validate});

  @override
  State<_ColourDialog> createState() => _ColourDialogState();
}

class _ColourDialogState extends State<_ColourDialog> {
  late final _controller = TextEditingController(
    text: normalizeWorkspaceColorInput(widget.current ?? ''),
  );

  /// The swatch the pointer is over, shown in the caption ahead of the
  /// selection so a desktop user can read a colour's name before picking it.
  WorkspaceSwatch? _hovered;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  String get _hex => normalizeWorkspaceColorInput(_controller.text);

  /// Null for an empty field: nothing typed is not a mistake, just nothing to
  /// save.
  String? get _error => _hex.isEmpty ? null : widget.validate(_hex);

  bool get _valid => _hex.isNotEmpty && _error == null;

  void _set(String text) {
    _controller.value = TextEditingValue(
      text: text,
      selection: TextSelection.collapsed(offset: text.length),
    );
    setState(() {});
  }

  Future<void> _paste() async {
    final text = (await Clipboard.getData(Clipboard.kTextPlain))?.text;
    if (text == null || !mounted) return;
    _set(text.trim());
  }

  void _save() {
    if (!_valid) return;
    Navigator.of(context).pop(_ColourChoice(_hex));
  }

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final swatches = themeSwatches(t);
    final valid = _valid;
    final selected = valid
        ? swatches.where((s) => s.hex == _hex).firstOrNull
        : null;
    final shown = _hovered ?? selected;
    final caption = shown != null
        ? '${shown.name} · ${shown.hex}'
        : valid
        ? 'Custom · $_hex'
        : '';
    final preview = valid ? parseWorkspaceColor(_hex) : null;

    return AlertDialog(
      title: const Text('Colour'),
      content: SizedBox(
        width: 320,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Wrap(
                spacing: 10,
                runSpacing: 10,
                children: [
                  for (final s in swatches)
                    Tooltip(
                      message: '${s.name} · ${s.hex}',
                      child: MouseRegion(
                        onEnter: (_) => setState(() => _hovered = s),
                        onExit: (_) => setState(() => _hovered = null),
                        child: InkResponse(
                          key: ValueKey('swatch-${s.hex}'),
                          onTap: () => _set(s.hex),
                          child: Container(
                            width: 32,
                            height: 32,
                            decoration: BoxDecoration(
                              color: parseWorkspaceColor(s.hex),
                              shape: BoxShape.circle,
                              border: s == selected
                                  ? Border.all(color: t.textBright, width: 2)
                                  : null,
                            ),
                          ),
                        ),
                      ),
                    ),
                ],
              ),
              const SizedBox(height: 10),
              Text(
                caption,
                key: const ValueKey('colour-caption'),
                style: t.meta(size: 11, color: t.textMuted),
              ),
              const SizedBox(height: 12),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Padding(
                    padding: const EdgeInsets.only(top: 12),
                    child: Container(
                      key: const ValueKey('colour-preview'),
                      width: 28,
                      height: 28,
                      decoration: BoxDecoration(
                        color: preview,
                        shape: BoxShape.circle,
                        border: preview == null
                            ? Border.all(color: t.textFaint, width: 1)
                            : null,
                      ),
                    ),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: TextField(
                      key: const ValueKey('colour-hex-field'),
                      controller: _controller,
                      autocorrect: false,
                      enableSuggestions: false,
                      style: TextStyle(fontFamily: t.mono),
                      decoration: InputDecoration(
                        labelText: 'Hex',
                        hintText: '#rrggbb',
                        errorText: _error,
                        errorMaxLines: 2,
                        suffixIcon: IconButton(
                          tooltip: 'Paste',
                          icon: const Icon(Icons.content_paste),
                          onPressed: _paste,
                        ),
                      ),
                      onChanged: (_) => setState(() {}),
                      onSubmitted: (_) => _save(),
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(const _ColourChoice(null)),
          child: const Text('No colour'),
        ),
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: valid ? _save : null,
          child: const Text('Save'),
        ),
      ],
    );
  }
}
