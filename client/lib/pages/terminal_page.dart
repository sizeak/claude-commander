import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:uuid/uuid.dart';
import 'package:xterm/xterm.dart';

import '../server_config.dart';
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/terminal.dart' as rust;

/// Live attached terminal (Phase 3 spike). Streams raw PTY bytes from the
/// cdylib WS bridge into an `xterm.dart` [Terminal], forwards keystrokes/resize
/// back, offers an on-screen modifier bar for touch, and shows a live byte
/// throughput meter so we can judge whether Flutter + xterm.dart keep up.
class TerminalPage extends StatefulWidget {
  final ServerConfig config;
  final SessionInfo session;
  const TerminalPage({super.key, required this.config, required this.session});

  @override
  State<TerminalPage> createState() => _TerminalPageState();
}

class _TerminalPageState extends State<TerminalPage> {
  // A unique id ties this attach's control calls back to its socket in Rust.
  final String _handle = const Uuid().v4();
  late final Terminal _terminal;
  StreamSubscription<rust.TerminalEvent>? _sub;

  // Stateful UTF-8 decoder: PTY chunks can split a multibyte codepoint across
  // WS frames, so a chunked decoder buffers the partial tail until it completes.
  late final ByteConversionSink _decoder;

  String _status = 'connecting…';

  /// True once the attach has ended (detach/transport/error), so the UI offers
  /// a reconnect instead of pretending it's still live.
  bool _ended = false;

  // Throughput meter: bytes this second, refreshed on a 1s tick.
  int _totalBytes = 0;
  int _windowBytes = 0;
  int _bytesPerSec = 0;
  Timer? _meter;

  @override
  void initState() {
    super.initState();
    _terminal = Terminal(maxLines: 10000);
    _decoder = utf8.decoder.startChunkedConversion(
      StringConversionSink.withCallback((str) => _terminal.write(str)),
    );

    _terminal.onOutput = (data) {
      unawaited(
        rust.terminalSendInput(
          handle: _handle,
          bytes: utf8.encode(data),
        ),
      );
    };
    _terminal.onResize = (cols, rows, pixelWidth, pixelHeight) {
      unawaited(rust.terminalResize(handle: _handle, cols: cols, rows: rows));
    };

    _connect();

    _meter = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      setState(() {
        _bytesPerSec = _windowBytes;
        _windowBytes = 0;
      });
    });
  }

  /// Open (or re-open) the WS attach. The same handle is reused — the cdylib
  /// registry entry is dropped when an attach ends, so re-attaching re-inserts
  /// it. A re-attach replays tmux's pane, so output simply continues appending.
  void _connect() {
    _sub?.cancel();
    setState(() {
      _status = 'connecting…';
      _ended = false;
    });
    _sub =
        rust
            .attachTerminal(
              handle: _handle,
              baseUrl: widget.config.baseUrl,
              token: widget.config.token,
              sessionId: widget.session.id,
            )
            .listen(
              _onEvent,
              onError: (Object e) => setState(() {
                _status = 'stream error: $e';
                _ended = true;
              }),
            );
  }

  void _reconnect() => _connect();

  void _onEvent(rust.TerminalEvent e) {
    switch (e.kind) {
      case rust.TerminalEventKind.output:
        _totalBytes += e.bytes.length;
        _windowBytes += e.bytes.length;
        _decoder.add(e.bytes);
      case rust.TerminalEventKind.ready:
        setState(() => _status = 'attached: ${e.text}');
      case rust.TerminalEventKind.detached:
        setState(() {
          _status = 'detached: ${e.text}';
          _ended = true;
        });
      case rust.TerminalEventKind.error:
        setState(() {
          _status = 'error: ${e.text}';
          _ended = true;
        });
    }
  }

  void _send(List<int> bytes) =>
      unawaited(rust.terminalSendInput(handle: _handle, bytes: bytes));

  @override
  void dispose() {
    _meter?.cancel();
    unawaited(rust.terminalDetach(handle: _handle));
    _sub?.cancel();
    _decoder.close();
    super.dispose();
  }

  String _fmtRate(int bytesPerSec) {
    if (bytesPerSec >= 1024 * 1024) {
      return '${(bytesPerSec / (1024 * 1024)).toStringAsFixed(1)} MB/s';
    }
    if (bytesPerSec >= 1024) {
      return '${(bytesPerSec / 1024).toStringAsFixed(1)} KB/s';
    }
    return '$bytesPerSec B/s';
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: Text(widget.session.title, overflow: TextOverflow.ellipsis),
        actions: [
          IconButton(
            onPressed: _ended ? _reconnect : null,
            icon: const Icon(Icons.refresh),
            tooltip: 'Reconnect',
          ),
        ],
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(20),
          child: Padding(
            padding: const EdgeInsets.only(left: 16, right: 16, bottom: 4),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    _status,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.labelSmall,
                  ),
                ),
                Text(
                  '${_fmtRate(_bytesPerSec)} · ${_totalBytes ~/ 1024} KB',
                  style: Theme.of(context).textTheme.labelSmall,
                ),
              ],
            ),
          ),
        ),
      ),
      body: Column(
        children: [
          Expanded(
            child: TerminalView(
              _terminal,
              autofocus: true,
              backgroundOpacity: 1,
            ),
          ),
          _ModifierBar(onSend: _send),
        ],
      ),
    );
  }
}

/// On-screen keys for touch — the modifiers and arrows a soft keyboard can't
/// easily produce. Each sends the raw byte sequence the PTY expects.
class _ModifierBar extends StatelessWidget {
  final void Function(List<int> bytes) onSend;
  const _ModifierBar({required this.onSend});

  static const _esc = [0x1b];
  static const _tab = [0x09];
  // Ctrl-<letter> is the letter's code & 0x1f.
  static const _ctrlC = [0x03];
  static const _ctrlD = [0x04];
  static const _ctrlZ = [0x1a];
  static const _ctrlL = [0x0c];
  static const _ctrlR = [0x12];
  static const _ctrlA = [0x01];
  static const _ctrlE = [0x05];
  static const _ctrlU = [0x15];
  static const _up = [0x1b, 0x5b, 0x41];
  static const _down = [0x1b, 0x5b, 0x42];
  static const _right = [0x1b, 0x5b, 0x43];
  static const _left = [0x1b, 0x5b, 0x44];
  static const _home = [0x1b, 0x5b, 0x48];
  static const _end = [0x1b, 0x5b, 0x46];
  static const _pgUp = [0x1b, 0x5b, 0x35, 0x7e];
  static const _pgDn = [0x1b, 0x5b, 0x36, 0x7e];

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      top: false,
      child: SizedBox(
        height: 44,
        child: ListView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          children: [
            _key(context, 'Esc', () => onSend(_esc)),
            _key(context, 'Tab', () => onSend(_tab)),
            _key(context, '^C', () => onSend(_ctrlC)),
            _key(context, '^D', () => onSend(_ctrlD)),
            _key(context, '^Z', () => onSend(_ctrlZ)),
            _key(context, '^L', () => onSend(_ctrlL)),
            _key(context, '^R', () => onSend(_ctrlR)),
            _key(context, '^A', () => onSend(_ctrlA)),
            _key(context, '^E', () => onSend(_ctrlE)),
            _key(context, '^U', () => onSend(_ctrlU)),
            _key(context, '↑', () => onSend(_up)),
            _key(context, '↓', () => onSend(_down)),
            _key(context, '←', () => onSend(_left)),
            _key(context, '→', () => onSend(_right)),
            _key(context, 'Home', () => onSend(_home)),
            _key(context, 'End', () => onSend(_end)),
            _key(context, 'PgUp', () => onSend(_pgUp)),
            _key(context, 'PgDn', () => onSend(_pgDn)),
          ],
        ),
      ),
    );
  }

  Widget _key(BuildContext context, String label, VoidCallback onTap) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 3),
      child: OutlinedButton(
        onPressed: onTap,
        style: OutlinedButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          minimumSize: const Size(0, 36),
        ),
        child: Text(label),
      ),
    );
  }
}
