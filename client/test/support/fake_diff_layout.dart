import 'package:claude_commander_client/src/rust/api/diff.dart';
import 'package:claude_commander_client/src/rust/api/review.dart';

/// A stand-in for the cdylib's `diff_rows`, for widget tests.
///
/// `flutter test` runs without the native library, so the real layout engine is
/// out of reach; this reproduces the *shape* it returns — a header row per hunk,
/// a line row per diff line, one span of content each, selectable indices
/// counted across the file — which is all a widget test needs to drive the view.
///
/// Deliberately does **not** reproduce what makes the real thing worth having:
/// word-diff emphasis, side-by-side pairing, gap expansion and tab handling are
/// covered by `diff_rows`' own Rust tests, and a Dart test that wanted them
/// would be testing this file. A widget test that needs those builds the layout
/// it wants by hand instead.
DiffLayoutDto fakeDiffLayout(ReviewFileDto file) {
  final rows = <DiffRowDto>[];
  var sel = 0;
  for (final hunk in file.hunks) {
    rows.add(
      DiffRowDto(
        kind: DiffRowKind.hunkHeader,
        fullWidth: true,
        left: cellOf(
          '@@ -${hunk.oldStart},${hunk.oldLines} '
          '+${hunk.newStart},${hunk.newLines} @@'
          '${hunk.header.isNotEmpty ? " ${hunk.header}" : ""}',
          DiffRole.hunkHeader,
        ),
        right: absentCell,
        hidden: 0,
        canExpandUp: false,
        canExpandDown: false,
      ),
    );
    for (final line in hunk.lines) {
      rows.add(
        DiffRowDto(
          kind: DiffRowKind.line,
          fullWidth: false,
          left: cellOf(
            line.content,
            switch (line.origin) {
              ReviewLineOrigin.addition => DiffRole.addition,
              ReviewLineOrigin.deletion => DiffRole.deletion,
              ReviewLineOrigin.context => DiffRole.context,
            },
            origin: line.origin,
            oldLineno: line.oldLineno,
            newLineno: line.newLineno,
            sel: sel,
          ),
          right: absentCell,
          hidden: 0,
          canExpandUp: false,
          canExpandDown: false,
        ),
      );
      sel++;
    }
  }
  return DiffLayoutDto(rows: rows, selectable: sel, hasHiddenContext: false);
}

/// A cell holding one run of text.
DiffCellDto cellOf(
  String text,
  DiffRole role, {
  ReviewLineOrigin? origin,
  int? oldLineno,
  int? newLineno,
  int? sel,
}) => DiffCellDto(
  present: true,
  origin: origin,
  oldLineno: oldLineno,
  newLineno: newLineno,
  sel: sel,
  spans: [DiffSpanDto(text: text, role: role, emphasis: false)],
  text: text,
);

/// The blank half of a side-by-side pair, and the unused right half inline.
const absentCell = DiffCellDto(present: false, spans: [], text: '');
