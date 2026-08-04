import 'package:claude_commander_client/src/rust/api/diff.dart';
import 'package:claude_commander_client/src/rust/api/review.dart'
    show ReviewLineOrigin;
import 'package:claude_commander_client/theme/app_colors.dart';
import 'package:claude_commander_client/theme/diff_theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  final colors = DiffColors.dark;

  test('emphasis is composited on the line fill, not picked separately', () {
    // The rule the whole palette hangs on: an emphasised run is the SAME accent
    // again over the line's own fill, so it always reads as "more of this line".
    // Picking it independently is what makes a word diff look like a third
    // colour rather than a stronger second one.
    expect(
      colors.emphasisFill(DiffRole.addition),
      Color.alphaBlend(
        AppColors.green.withValues(alpha: 0.20),
        colors.additionFill,
      ),
    );
    expect(
      colors.emphasisFill(DiffRole.deletion),
      Color.alphaBlend(
        AppColors.red.withValues(alpha: 0.20),
        colors.deletionFill,
      ),
    );
    // And it is genuinely stronger than the line it sits on, or the word diff
    // would be invisible.
    expect(colors.emphasisFill(DiffRole.addition), isNot(colors.additionFill));
  });

  test('the line fill is the accent over the page background', () {
    expect(
      colors.additionFill,
      Color.alphaBlend(AppColors.green.withValues(alpha: 0.20), AppColors.bg),
    );
  });

  test('only changed lines carry a fill or an emphasis colour', () {
    expect(colors.lineFill(ReviewLineOrigin.context), isNull);
    expect(colors.lineFill(null), isNull);
    expect(colors.emphasisFill(DiffRole.context), isNull);
    expect(colors.emphasisFill(DiffRole.hunkHeader), isNull);
  });

  test('an unrecognised role still renders as readable code', () {
    // `DiffRole` mirrors a `#[non_exhaustive]` Rust enum, so a role this build
    // has never heard of must not come out invisible.
    expect(colors.foreground(DiffRole.other), colors.contextFg);
  });

  testWidgets('the palette in scope wins over the default', (tester) async {
    final custom = DiffColors.derive(
      background: const Color(0xFF000000),
      gutterBackground: const Color(0xFF000000),
      addition: const Color(0xFF00FF00),
      deletion: const Color(0xFFFF0000),
      contextFg: const Color(0xFFFFFFFF),
      gutterFg: const Color(0xFF888888),
      hunkHeaderFg: const Color(0xFF00FFFF),
      selection: const Color(0xFFFFFF00),
    );
    late DiffColors seen;
    await tester.pumpWidget(
      DiffTheme(
        colors: custom,
        child: Builder(
          builder: (context) {
            seen = DiffTheme.of(context);
            return const SizedBox();
          },
        ),
      ),
    );
    expect(seen, custom);
    expect(seen, isNot(DiffColors.dark));
  });
}
