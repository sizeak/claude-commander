// Flutter-free folding of a typed character into the byte a terminal sends for
// Ctrl+<character>.
//
// The on-screen key row above a soft keyboard can only carry a fixed handful of
// chords (^C, ^D, ^Z, …), and every pane that wants one it does not list —
// Ctrl+W to kill a word, Ctrl+O in nano, Ctrl+X anywhere — is unreachable from
// a phone. So the row's Ctrl key *arms* instead of sending: the next character
// the keyboard produces is folded here.

/// What an armed Ctrl does to one chunk of terminal input.
///
/// [output] replaces the input; [consumed] says the arm was used up. A chunk
/// that is not a single typed character has no [CtrlChord] at all (see
/// [ctrlChord]).
class CtrlChord {
  final String output;
  final bool consumed;
  const CtrlChord(this.output, {required this.consumed});
}

/// The chord for [text] when Ctrl is armed, or `null` when [text] is not a
/// single typed character and so must pass through with the arm left standing.
///
/// The mapping is the ASCII one every terminal implements: the control code is
/// the character's code point with bits 6 and 7 cleared (`& 0x1f`) across the
/// `@ A–Z [ \ ] ^ _` block, lowercase folded to upper first. Two chords sit
/// outside that block by convention and are special-cased — Ctrl+Space is NUL,
/// Ctrl+? is DEL. A character with no control form (a digit, `£`, an emoji) is
/// returned unchanged but still consumes the arm, because the user *did* aim it
/// at that keystroke and leaving it armed would silently mangle the next one.
///
/// `null` is deliberately distinct from "unchanged": a paste, an arrow key's
/// escape sequence and the emulator's own replies to device queries all reach
/// the same funnel, and none of them is the keystroke the arm was set for.
CtrlChord? ctrlChord(String text) {
  // Runes, not `length`: an astral codepoint is a single character in two
  // UTF-16 code units, which a length check would read as a paste.
  final runes = text.runes.toList();
  if (runes.length != 1) return null;
  final code = runes.first;

  // The two chords outside the block below, by convention.
  if (code == 0x20) return const CtrlChord('\x00', consumed: true); // Space→NUL
  if (code == 0x3f) return const CtrlChord('\x7f', consumed: true); // ?→DEL

  // Lowercase a–z onto their uppercase code points, so both cases land in the
  // `@`–`_` block below.
  final upper = (code >= 0x61 && code <= 0x7a) ? code - 0x20 : code;
  if (upper >= 0x40 && upper <= 0x5f) {
    return CtrlChord(String.fromCharCode(upper & 0x1f), consumed: true);
  }
  return CtrlChord(text, consumed: true);
}
