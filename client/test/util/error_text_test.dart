import 'package:claude_commander_client/util/error_text.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('errorText', () {
    /// The regression this helper exists for: an unreachable server produced a
    /// ten-frame Rust backtrace on the session list. The exact shape below is
    /// what the bridge hands Dart (frb serializes `format!("{:?}", err)`).
    test('unwraps an AnyhowException and drops its backtrace', () {
      const raw =
          'AnyhowException(backend unavailable: could not connect to server\n'
          '\n'
          'Stack backtrace:\n'
          '   0: <unknown>\n'
          '   1: <unknown>\n'
          '   2: __start_thread)';

      expect(errorText(raw), 'Could not connect to server');
    });

    test('keeps only the outermost context of a Caused by chain', () {
      const raw =
          'AnyhowException(server error: boom\n\nCaused by:\n'
          '    0: inner detail\n    1: deeper detail)';

      expect(errorText(raw), 'Server error: boom');
    });

    test('strips the redundant category prefixes', () {
      expect(
        errorText('backend unavailable: tmux is not running'),
        'Tmux is not running',
      );
      expect(errorText(Exception('boom')), 'Boom');
    });

    test('leaves an already-tidy message alone', () {
      expect(
        errorText('authentication failed (check your token)'),
        'Authentication failed (check your token)',
      );
    });

    test('capitalize: false leaves the text fit for appending to a label', () {
      expect(
        errorText(
          'backend unavailable: could not connect to server',
          capitalize: false,
        ),
        'could not connect to server',
      );
    });

    test('clamps a long single-line message', () {
      final long = 'x' * 400;
      final out = errorText(long, maxLength: 40);

      expect(out.length, 41); // 40 chars plus the ellipsis
      expect(out.endsWith('…'), isTrue);
    });

    test('falls back when nothing usable survives', () {
      expect(
        errorText('AnyhowException(\n\nStack backtrace:\n 0: x)'),
        'Something went wrong',
      );
      expect(errorText('   '), 'Something went wrong');
    });
  });
}
