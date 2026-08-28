import 'package:claude_commander_client/util/ctrl_chord.dart';
import 'package:flutter_test/flutter_test.dart';

/// One control byte as a string, so expectations read as the code point they
/// pin rather than as an unprintable literal.
String ctl(int code) => String.fromCharCode(code);

void main() {
  group('ctrlChord', () {
    test('folds a lowercase letter onto its control code', () {
      expect(ctrlChord('a')?.output, ctl(0x01));
      expect(ctrlChord('c')?.output, ctl(0x03));
      expect(ctrlChord('w')?.output, ctl(0x17));
      expect(ctrlChord('z')?.output, ctl(0x1a));
    });

    // A phone with shift latched, or a hardware keyboard, produces the
    // uppercase form — which xterm's own `charInput` does not map at all.
    test('folds an uppercase letter the same way', () {
      expect(ctrlChord('A')?.output, ctl(0x01));
      expect(ctrlChord('W')?.output, ctl(0x17));
    });

    test('maps the punctuation block that has control codes', () {
      expect(ctrlChord('@')?.output, ctl(0x00));
      expect(ctrlChord('[')?.output, ctl(0x1b));
      expect(ctrlChord(r'\')?.output, ctl(0x1c));
      expect(ctrlChord(']')?.output, ctl(0x1d));
      expect(ctrlChord('^')?.output, ctl(0x1e));
      expect(ctrlChord('_')?.output, ctl(0x1f));
    });

    // The two chords that sit outside the `& 0x1f` block by convention.
    test('maps space to NUL and question mark to DEL', () {
      expect(ctrlChord(' ')?.output, ctl(0x00));
      expect(ctrlChord('?')?.output, ctl(0x7f));
    });

    test(
      'a character with no control form is sent unchanged, using the arm',
      () {
        final digit = ctrlChord('4');
        expect(digit?.output, '4');
        expect(digit?.consumed, isTrue);
        expect(ctrlChord('£')?.output, '£');
        expect(ctrlChord('✓')?.consumed, isTrue);
      },
    );

    test('a mapped character uses the arm', () {
      expect(ctrlChord('c')?.consumed, isTrue);
    });

    // Anything that is not a single typed character — a paste, an arrow key's
    // escape sequence, the emulator's own reply to a device query — must not
    // silently eat the arm the user set for the character they are about to
    // type.
    test('leaves the arm alone for anything but a single character', () {
      expect(ctrlChord('hello'), isNull);
      expect(ctrlChord('\x1b[A'), isNull);
      expect(ctrlChord(''), isNull);
    });

    // A codepoint outside the BMP is one rune but two UTF-16 code units, so a
    // `length`-based check would misread it as a paste.
    test('treats an astral character as a single character', () {
      final emoji = ctrlChord('😀');
      expect(emoji?.output, '😀');
      expect(emoji?.consumed, isTrue);
    });
  });
}
