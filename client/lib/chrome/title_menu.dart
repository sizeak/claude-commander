import 'package:flutter/material.dart';

import '../theme/tokens.dart';

/// A view title that doubles as a switcher: "Fleet · Work ▾", which opens a
/// list of the alternatives. The workspace picker is its one user.
///
/// Carried as data on the view specs ([ChromeViewRailSpec.titleMenu],
/// [ChromeWideSpec.titleMenu]) rather than as a widget, like every other chrome
/// input, because the two themes present the list differently: Mission Control
/// drops a menu under the title, LCARS raises a sheet — the same split
/// [ChromeMenuAction] makes, for the same reason (LCARS has nothing to hang a
/// dropdown from, and a sheet is the better touch target).
@immutable
class ChromeTitleMenu {
  /// The current choice's label, shown after the title.
  final String current;

  /// The current choice's accent, tinting its label. Null uses the title's.
  final Color? color;

  final List<ChromeTitleMenuItem> items;

  const ChromeTitleMenu({
    required this.current,
    this.color,
    required this.items,
  });
}

/// One entry of a [ChromeTitleMenu].
@immutable
class ChromeTitleMenuItem {
  final String label;

  /// The entry's accent: a dot beside the label. Null draws no dot.
  final Color? color;

  /// A count that wants attention (sessions waiting for input), shown as
  /// `●n` in the attention colour. Zero shows nothing.
  final int badge;

  final bool selected;
  final VoidCallback onSelected;

  const ChromeTitleMenuItem({
    required this.label,
    this.color,
    this.badge = 0,
    this.selected = false,
    required this.onSelected,
  });
}

/// The tappable title, as both themes key it — the locator tests use.
const chromeTitleMenuKey = ValueKey('chrome-title-menu');

/// [title] alone, or — given a [menu] — "title · current ▾" as one tap target
/// that opens it. [upper] uppercases the whole run, for LCARS.
Widget chromeMenuTitle(
  BuildContext context, {
  required String title,
  required TextStyle? style,
  ChromeTitleMenu? menu,
  bool upper = false,
}) {
  String cased(String s) => upper ? s.toUpperCase() : s;
  if (menu == null) {
    return Text(
      cased(title),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: style,
    );
  }
  // A Builder so the menu anchors to the title's own box rather than to
  // whatever [context] belongs to (a whole pane, typically).
  return Builder(
    builder: (anchor) => InkWell(
      key: chromeTitleMenuKey,
      onTap: () => showChromeTitleMenu(anchor, menu),
      borderRadius: BorderRadius.circular(6),
      child: Text.rich(
        TextSpan(
          children: [
            TextSpan(text: cased(title)),
            TextSpan(
              text: ' · ',
              style: TextStyle(color: style?.color?.withValues(alpha: 0.5)),
            ),
            TextSpan(
              text: cased(menu.current),
              style: menu.color == null ? null : TextStyle(color: menu.color),
            ),
            // An icon, not a "▾" glyph: none of the bundled faces carries
            // U+25BE, so as text it rendered as a missing-glyph box wherever
            // no system font happened to fill it in (every golden, for one).
            WidgetSpan(
              alignment: PlaceholderAlignment.middle,
              child: Icon(
                Icons.arrow_drop_down,
                size: (style?.fontSize ?? 14) * 1.1,
                color: style?.color,
              ),
            ),
          ],
        ),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: style,
      ),
    ),
  );
}

/// Open [menu]: a dropdown under the title in Mission Control, a bottom sheet
/// in LCARS.
Future<void> showChromeTitleMenu(
  BuildContext context,
  ChromeTitleMenu menu,
) async {
  final t = CommanderTokens.of(context);
  if (t.chrome == ChromeKind.lcars) {
    await showModalBottomSheet<void>(
      context: context,
      builder: (sheetContext) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (final item in menu.items)
              _MenuRow(
                item: item,
                upper: true,
                onTap: () {
                  Navigator.of(sheetContext).pop();
                  item.onSelected();
                },
              ),
          ],
        ),
      ),
    );
    return;
  }
  final box = context.findRenderObject() as RenderBox?;
  final overlay =
      Navigator.of(context).overlay?.context.findRenderObject() as RenderBox?;
  if (box == null || overlay == null) return;
  final origin = box.localToGlobal(
    box.size.bottomLeft(Offset.zero),
    ancestor: overlay,
  );
  final chosen = await showMenu<int>(
    context: context,
    position: RelativeRect.fromLTRB(
      origin.dx,
      origin.dy,
      overlay.size.width - origin.dx,
      0,
    ),
    items: [
      for (var i = 0; i < menu.items.length; i++)
        PopupMenuItem(
          value: i,
          child: _MenuRow(item: menu.items[i], upper: false),
        ),
    ],
  );
  if (chosen != null) menu.items[chosen].onSelected();
}

/// A menu entry: accent dot, label, waiting badge and a check on the current.
class _MenuRow extends StatelessWidget {
  final ChromeTitleMenuItem item;
  final bool upper;
  final VoidCallback? onTap;

  const _MenuRow({required this.item, required this.upper, this.onTap});

  @override
  Widget build(BuildContext context) {
    final t = CommanderTokens.of(context);
    final row = Row(
      children: [
        SizedBox(
          width: 16,
          child: item.color == null
              ? null
              : Container(
                  width: 9,
                  height: 9,
                  decoration: BoxDecoration(
                    color: item.color,
                    shape: BoxShape.circle,
                  ),
                ),
        ),
        Expanded(
          child: Text(
            upper ? item.label.toUpperCase() : item.label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontFamily: t.sans,
              fontWeight: item.selected ? FontWeight.w700 : FontWeight.w500,
              color: t.text,
            ),
          ),
        ),
        if (item.badge > 0) ...[
          const SizedBox(width: 10),
          Text(
            '●${item.badge}',
            style: t.meta(
              size: 11,
              weight: FontWeight.w700,
              color: t.attention,
            ),
          ),
        ],
        const SizedBox(width: 8),
        SizedBox(
          width: 18,
          child: item.selected
              ? Icon(Icons.check, size: 16, color: t.primary)
              : null,
        ),
      ],
    );
    if (onTap == null) return row;
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 14),
        child: row,
      ),
    );
  }
}
