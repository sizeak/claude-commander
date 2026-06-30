import 'dart:async';
import 'dart:convert';

import 'package:claude_commander_client/src/rust/api/terminal.dart' as rust;
import 'package:claude_commander_client/src/rust/frb_generated.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:xterm/xterm.dart';

/// Phase 3 spike measurement: drive a synthetic high-volume PTY flood through
/// the real client pipeline (cdylib frb stream → chunked UTF-8 decode →
/// xterm.dart write → rendered frames) and report sustained throughput. No
/// server/socket — isolates the frb + terminal-widget cost, which is the part
/// the framework choice hinges on.
void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());

  testWidgets('xterm.dart render throughput under a synthetic PTY flood', (
    tester,
  ) async {
    const chunks = 2000;
    const chunkBytes = 4096; // ~8 MB total
    final terminal = Terminal(maxLines: 10000);
    final decoder = utf8.decoder.startChunkedConversion(
      StringConversionSink.withCallback((s) => terminal.write(s)),
    );

    await tester.pumpWidget(
      MaterialApp(home: Scaffold(body: TerminalView(terminal))),
    );

    var bytes = 0;
    final done = Completer<void>();
    final sw = Stopwatch()..start();
    final sub = rust
        .benchTerminalStream(chunks: chunks, chunkBytes: chunkBytes)
        .listen((e) {
          switch (e.kind) {
            case rust.TerminalEventKind.output:
              bytes += e.bytes.length;
              decoder.add(e.bytes);
            case rust.TerminalEventKind.detached:
              if (!done.isCompleted) done.complete();
            case rust.TerminalEventKind.ready:
            case rust.TerminalEventKind.error:
              break;
          }
        });

    // Pump frames while data flows so xterm.dart actually lays out + paints.
    var guard = 0;
    while (!done.isCompleted && guard < 6000) {
      await tester.pump(const Duration(milliseconds: 16));
      guard++;
    }
    sw.stop();
    await sub.cancel();
    decoder.close();

    final secs = sw.elapsedMilliseconds / 1000.0;
    final mb = bytes / (1024 * 1024);
    final rate = secs > 0 ? mb / secs : 0;
    // ignore: avoid_print
    print(
      'TERMINAL_THROUGHPUT bytes=$bytes elapsed_ms=${sw.elapsedMilliseconds} '
      'rate=${rate.toStringAsFixed(1)}MB/s frames=$guard',
    );
    expect(bytes, chunks * chunkBytes);
  });
}
