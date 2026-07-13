import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:uuid/uuid.dart';
import 'package:xterm/xterm.dart';

import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';

/// Live attached terminal, layout-agnostic (no Scaffold, no route). Streams raw
/// PTY bytes from the cdylib WS bridge into an `xterm.dart` [Terminal], forwards
/// keystrokes/resize back, shows a compact status/throughput bar with a reconnect
/// action, and — only when [showModifierBar] is set (touch/narrow) — an on-screen
/// modifier bar.
///
/// Each attach uses a fresh per-attach id (a UUID) that keys its control channel
/// in the cdylib, so several attaches can be live against one server. The id is
/// registered with the [CommanderStore] (when one is in scope) so a
/// reconnect/dispose of the store tears the attach down before releasing the
/// handle. Resize is driven by `xterm`'s [Terminal.onResize], which fires from
/// the widget's actual laid-out size — so the pane's real cols/rows reach the
/// server, not a fixed 80x24.
class TerminalBody extends StatefulWidget {
  final CommanderApi api;

  /// The live server handle, used to resolve the transport client for the attach.
  final String handle;
  final SessionInfo session;

  /// Which pane to attach to: the agent pane (default) or the paired shell.
  final AttachKind kind;

  /// Show the on-screen modifier/arrow bar (mobile/touch only). Desktop relies
  /// on the physical keyboard, so this is false there.
  final bool showModifierBar;

  const TerminalBody({
    super.key,
    required this.api,
    required this.handle,
    required this.session,
    this.kind = AttachKind.agent,
    this.showModifierBar = true,
  });

  @override
  State<TerminalBody> createState() => _TerminalBodyState();
}

class _TerminalBodyState extends State<TerminalBody> {
  late final Terminal _terminal;
  StreamSubscription<TerminalEvent>? _sub;
  CommanderStore? _store;

  /// A fresh id per attach: keys this attach's control channel in the cdylib.
  /// Regenerated on every (re)connect so a reconnect never collides with the
  /// entry a just-ended attach is still tearing down.
  String _attachId = const Uuid().v4();

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
    // Forward each decoded chunk to the terminal as it arrives. A plain
    // `Sink<String>` emits per-`add` (unlike `StringConversionSink.withCallback`,
    // which only fires its callback on `close`), while the chunked UTF-8 decoder
    // still buffers a partial multibyte codepoint split across WS frames until
    // it completes.
    _decoder = utf8.decoder.startChunkedConversion(
      _ChunkSink((str) => _terminal.write(str)),
    );

    _terminal.onOutput = (data) {
      unawaited(
        widget.api.terminalSendInput(
          attachId: _attachId,
          bytes: utf8.encode(data),
        ),
      );
    };
    _terminal.onResize = (cols, rows, pixelWidth, pixelHeight) {
      unawaited(
        widget.api.terminalResize(
          attachId: _attachId,
          cols: cols,
          rows: rows,
        ),
      );
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

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // Register the current attach with the store (if one is in scope) so its
    // reconnect/dispose tears the attach down before releasing the handle.
    _store = CommanderStoreScope.of(context);
    _store?.setActiveTerminalAttach(_attachId);
  }

  /// Open (or re-open) the WS attach with a fresh attach id. A re-attach replays
  /// tmux's pane, so output simply continues appending.
  void _connect() {
    _sub?.cancel();
    _attachId = const Uuid().v4();
    _store?.setActiveTerminalAttach(_attachId);
    setState(() {
      _status = 'connecting…';
      _ended = false;
    });
    _sub = widget.api
        .attachTerminal(
          handle: widget.handle,
          attachId: _attachId,
          sessionId: widget.session.id,
          kind: widget.kind,
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

  void _onEvent(TerminalEvent e) {
    switch (e.kind) {
      case TerminalEventKind.output:
        _totalBytes += e.bytes.length;
        _windowBytes += e.bytes.length;
        _decoder.add(e.bytes);
      case TerminalEventKind.ready:
        setState(() => _status = 'attached: ${e.text}');
        // The server spawns each attach at its default 80x24 and only ever
        // learns our size from an explicit Resize. xterm's onResize fires only
        // when dimensions change, so on a same-size (re)connect it never does —
        // re-announce our current size on every ready.
        unawaited(
          widget.api.terminalResize(
            attachId: _attachId,
            cols: _terminal.viewWidth,
            rows: _terminal.viewHeight,
          ),
        );
      case TerminalEventKind.detached:
        setState(() {
          _status = 'detached: ${e.text}';
          _ended = true;
        });
      case TerminalEventKind.error:
        setState(() {
          _status = 'error: ${e.text}';
          _ended = true;
        });
    }
  }

  void _send(List<int> bytes) => unawaited(
    widget.api.terminalSendInput(attachId: _attachId, bytes: bytes),
  );

  @override
  void dispose() {
    _meter?.cancel();
    unawaited(widget.api.terminalDetach(attachId: _attachId));
    // Guarded clear: if the wide pane already swapped in another attach (agent↔
    // shell), its initState registered the new id before this dispose runs, so
    // only clear when we're still the registered attach.
    _store?.clearActiveTerminalAttach(_attachId);
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
    return Column(
      children: [
        _statusBar(context),
        Expanded(
          child: TerminalView(
            _terminal,
            autofocus: true,
            backgroundOpacity: 1,
          ),
        ),
        if (widget.showModifierBar) _ModifierBar(onSend: _send),
      ],
    );
  }

  Widget _statusBar(BuildContext context) {
    return Container(
      padding: const EdgeInsets.only(left: 12, right: 4, top: 2, bottom: 2),
      color: Theme.of(context).colorScheme.surfaceContainerHighest,
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
          IconButton(
            visualDensity: VisualDensity.compact,
            onPressed: _ended ? _reconnect : null,
            icon: const Icon(Icons.refresh, size: 18),
            tooltip: 'Reconnect',
          ),
        ],
      ),
    );
  }
}

/// The phone (stacked-navigation) terminal screen: a Scaffold titled by the
/// session, wrapping a [TerminalBody] with the on-screen modifier bar enabled.
class TerminalPage extends StatelessWidget {
  final CommanderApi api;
  final String handle;
  final SessionInfo session;

  /// Which pane to attach to: the agent pane (default) or the paired shell.
  final AttachKind kind;

  const TerminalPage({
    super.key,
    required this.api,
    required this.handle,
    required this.session,
    this.kind = AttachKind.agent,
  });

  @override
  Widget build(BuildContext context) {
    final isShell = kind == AttachKind.shell;
    return Scaffold(
      appBar: AppBar(
        title: Text(
          isShell ? '${session.title} · shell' : session.title,
          overflow: TextOverflow.ellipsis,
        ),
      ),
      body: SafeArea(
        child: TerminalBody(
          api: api,
          handle: handle,
          session: session,
          kind: kind,
        ),
      ),
    );
  }
}

/// A minimal `Sink<String>` that forwards each decoded chunk to [onData] the
/// moment it arrives — so terminal output renders live rather than only when
/// the decoder is closed.
class _ChunkSink implements Sink<String> {
  final void Function(String chunk) onData;
  const _ChunkSink(this.onData);

  @override
  void add(String data) => onData(data);

  @override
  void close() {}
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
