import 'package:claude_commander_client/util/viewport.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Future<bool> shortAt(WidgetTester tester, Size size) async {
    late bool short;
    await tester.pumpWidget(
      MediaQuery(
        data: MediaQueryData(size: size),
        child: Builder(
          builder: (context) {
            short = isShortViewport(context);
            return const SizedBox();
          },
        ),
      ),
    );
    return short;
  }

  testWidgets('a phone in landscape is short', (tester) async {
    expect(await shortAt(tester, const Size(800, 360)), isTrue);
  });

  testWidgets('the same phone upright is not', (tester) async {
    expect(await shortAt(tester, const Size(360, 800)), isFalse);
  });

  testWidgets('a tablet in landscape is not', (tester) async {
    expect(await shortAt(tester, const Size(1024, 768)), isFalse);
  });

  testWidgets('the keyboard cannot change the answer', (tester) async {
    // `viewInsets`, not `size`, is what a soft keyboard moves — so a viewport
    // that is tall stays tall while the keyboard covers most of it. The
    // terminal's pane geometry depends on this being true.
    late bool short;
    await tester.pumpWidget(
      MediaQuery(
        data: const MediaQueryData(
          size: Size(360, 800),
          viewInsets: EdgeInsets.only(bottom: 500),
        ),
        child: Builder(
          builder: (context) {
            short = isShortViewport(context);
            return const SizedBox();
          },
        ),
      ),
    );
    expect(short, isFalse);
  });
}
