import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../theme/theme_prefs.dart';
import '../theme/tokens.dart';
import '../util/colour_hex.dart';

/// One colour the dialog offers: a theme role's [name] and its `#rrggbb` [hex].
typedef ColourSwatch = ({String name, String hex});

/// The swatches the dialog offers: [t]'s semantic roles in [ThemeRole] order,
/// with any colour a theme reuses for several roles listed once under its
/// first name (Mission Control's `nav` is its `primary`, LCARS's `working` is
/// its `primary`). Any `#rrggbb` is accepted; these are just the menu, drawn
/// from the palette the colour will sit in.
List<ColourSwatch> themeSwatches(CommanderTokens t) {
  final seen = <String>{};
  return [
    for (final role in ThemeRole.values)
      if (seen.add(hexOf(role.of(t))))
        (name: role.label, hex: hexOf(role.of(t))),
  ];
}

/// What the dialog returned: a [color], or null for "Reset" — go back to
/// whatever the scope inherits. A dismissed dialog returns no choice at all.
@immutable
class ColourChoice {
  final Color? color;
  const ColourChoice(this.color);
}

/// Opens the colour dialog titled [title], prefilled with [current], offering
/// [swatchesFrom]'s palette (the app's active tokens when null). Resolves to a
/// [ColourChoice], or null when cancelled or dismissed.
Future<ColourChoice?> showColourDialog(
  BuildContext context, {
  required String title,
  Color? current,
  CommanderTokens? swatchesFrom,
}) => showDialog<ColourChoice>(
  context: context,
  builder: (_) =>
      _ColourDialog(title: title, current: current, swatchesFrom: swatchesFrom),
);

/// Theme swatches, a hex field (typed or pasted), and Reset.
///
/// The field is the single source of truth: tapping a swatch writes its hex
/// into it, and the swatch whose hex the field holds is the selected one — so
/// a pasted hex that happens to be a theme colour selects that swatch too.
class _ColourDialog extends StatefulWidget {
  final String title;
  final Color? current;
  final CommanderTokens? swatchesFrom;

  const _ColourDialog({
    required this.title,
    required this.current,
    required this.swatchesFrom,
  });

  @override
  State<_ColourDialog> createState() => _ColourDialogState();
}

class _ColourDialogState extends State<_ColourDialog> {
  late final _controller = TextEditingController(
    text: widget.current == null ? '' : hexOf(widget.current!),
  );

  /// The swatch the pointer is over, shown in the caption ahead of the
  /// selection so a desktop user can read a colour's name before picking it.
  ColourSwatch? _hovered;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  String get _hex => normalizeHexInput(_controller.text);

  /// Null for an empty field: nothing typed is not a mistake, just nothing to
  /// save.
  String? get _error => _hex.isEmpty ? null : hexColorError(_hex);

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
    Navigator.of(context).pop(ColourChoice(parseHexColor(_hex)));
  }

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final swatches = themeSwatches(widget.swatchesFrom ?? t);
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
    final preview = valid ? parseHexColor(_hex) : null;

    return AlertDialog(
      title: Text(widget.title),
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
                              color: parseHexColor(s.hex),
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
          onPressed: () => Navigator.of(context).pop(const ColourChoice(null)),
          child: const Text('Reset'),
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
