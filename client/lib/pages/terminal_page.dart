import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:uuid/uuid.dart';
import 'package:xterm/xterm.dart';

import '../services/commander_api.dart';
import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../theme/app_colors.dart';
import '../theme/app_theme.dart';

/// The xterm palette, built from the app tokens: the deepest terminal bg, the
/// soft off-white foreground the deck uses for pane text, and the violet accent
/// for the cursor/selection. The ANSI ramp is nudged toward the palette's
/// semantic accents (teal/green/amber/red/violet) so agent output reads in the
/// same colour language as the rest of the app.
const _terminalTheme = TerminalTheme(
  cursor: AppColors.accent,
  selection: Color(0x407C6CFF), // AppColors.accent @ ~25% alpha
  foreground: AppColors.terminalFg,
  background: AppColors.bgTerminal,
  black: Color(0xFF1C1F28), // AppColors.borderSubtle (darkest ANSI slot)
  red: AppColors.red,
  green: AppColors.green,
  yellow: AppColors.amber,
  blue: AppColors.accentSoft,
  magenta: AppColors.accent,
  cyan: AppColors.teal,
  white: AppColors.textBright,
  brightBlack: AppColors.textDim,
  brightRed: AppColors.red,
  brightGreen: AppColors.green,
  brightYellow: AppColors.amberText,
  brightBlue: AppColors.accentSoft,
  brightMagenta: AppColors.accentSoft,
  brightCyan: AppColors.teal,
  brightWhite: AppColors.text,
  searchHitBackground: Color(0x66F5B545), // AppColors.amber @ ~40% alpha
  searchHitBackgroundCurrent: AppColors.amber,
  searchHitForeground: AppColors.bg,
);

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
        widget.api.terminalResize(attachId: _attachId, cols: cols, rows: rows),
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
    // How much of us the soft keyboard covers. Zero when there is no keyboard —
    // and also zero when an ancestor Scaffold already consumed the inset by
    // shrinking us (`resizeToAvoidBottomInset: true`), which makes the panning
    // below a no-op. That is the case in the wide shell, which therefore still
    // resizes the pane on a soft keyboard — a known limitation, and a
    // touch-device-only one, since a desktop has no soft keyboard.
    final obscured = MediaQuery.viewInsetsOf(context).bottom;

    return ColoredBox(
      color: AppColors.bgTerminal,
      child: Column(
        children: [
          // Fixed: the status line stays put while the pane pans beneath it.
          _statusBar(context),
          Expanded(
            // Pan, don't resize. `xterm` derives the PTY's cols/rows from the
            // view's laid-out size, so letting the keyboard shrink the view
            // would resize the remote pane — and tmux answers a resize by
            // sliding a scrolled copy-mode view forward by a viewport height (it
            // doesn't compensate for the lines the shrink pushes into the
            // history), losing the user's place for good.
            //
            // So the pannable stack always fills this box — its height is a
            // function of the body alone, which the page holds constant — and we
            // translate it up instead. The pane's geometry is therefore constant
            // *by construction*: no keyboard-dependent arithmetic to get wrong,
            // and nothing to overflow when the keyboard is taller than the space
            // we have (landscape), where the worst case is simply that the pane
            // slides out of view rather than being resized.
            child: ClipRect(
              child: Transform.translate(
                offset: Offset(0, -obscured),
                child: Column(
                  children: [
                    Expanded(
                      child: TerminalView(
                        _terminal,
                        autofocus: true,
                        backgroundOpacity: 1,
                        theme: _terminalTheme,
                        textStyle: const TerminalStyle(
                          fontFamily: AppFonts.mono,
                        ),
                        padding: const EdgeInsets.symmetric(
                          horizontal: 14,
                          vertical: 8,
                        ),
                      ),
                    ),
                    // Rides up with the pane, landing just above the keyboard.
                    if (widget.showModifierBar) _ModifierBar(onSend: _send),
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// The dot colour reflects the link state: teal while attached, red once the
  /// attach has ended (reconnect offered), amber while connecting.
  Color get _statusColor {
    if (_ended) return AppColors.red;
    if (_status.startsWith('attached')) return AppColors.teal;
    return AppColors.amber;
  }

  Widget _statusBar(BuildContext context) {
    return Container(
      padding: const EdgeInsets.only(left: 14, right: 4, top: 4, bottom: 4),
      decoration: const BoxDecoration(
        color: AppColors.bgRaised,
        border: Border(bottom: BorderSide(color: AppColors.borderSubtle)),
      ),
      child: Row(
        children: [
          Container(
            width: 7,
            height: 7,
            decoration: BoxDecoration(
              color: _statusColor,
              shape: BoxShape.circle,
            ),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              _status,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: AppTheme.mono(size: 10, color: AppColors.textMuted),
            ),
          ),
          Text(
            '${_fmtRate(_bytesPerSec)} · ${_totalBytes ~/ 1024} KB',
            style: AppTheme.mono(size: 10, color: AppColors.textFaint),
          ),
          IconButton(
            visualDensity: VisualDensity.compact,
            onPressed: _ended ? _reconnect : null,
            icon: const Icon(Icons.refresh, size: 18),
            color: AppColors.textMuted,
            disabledColor: AppColors.textDim,
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
      // The keyboard must not shrink the body: [TerminalBody] insets its own
      // chrome and pans the pane instead, so the remote PTY never sees a resize.
      resizeToAvoidBottomInset: false,
      body: SafeArea(
        // Keep reserving the bottom system chrome even while the keyboard covers
        // it. Without this, SafeArea's bottom padding collapses to zero when the
        // keyboard appears and the pane's row count would move with it — the
        // resize [TerminalBody] exists to avoid.
        maintainBottomViewPadding: true,
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
    // No SafeArea here: [TerminalBody] already insets the bottom chrome, and a
    // bar whose height changed with the keyboard would change the pane's rows.
    return Container(
      height: 48,
      decoration: const BoxDecoration(
        color: AppColors.bgTerminal,
        border: Border(top: BorderSide(color: AppColors.borderSubtle)),
      ),
      child: ListView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
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
    );
  }

  /// A single key pill: a raised mono chip that fires its raw byte sequence on
  /// tap. Deliberately not a Material button so it matches the deck's flat pills
  /// and stays compact in the horizontal strip.
  Widget _key(BuildContext context, String label, VoidCallback onTap) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 3),
      child: Material(
        color: AppColors.surface,
        borderRadius: BorderRadius.circular(7),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(7),
          child: Container(
            alignment: Alignment.center,
            padding: const EdgeInsets.symmetric(horizontal: 12),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(7),
              border: Border.all(color: AppColors.border),
            ),
            child: Text(
              label,
              style: AppTheme.mono(
                size: 10.5,
                weight: FontWeight.w600,
                color: AppColors.textBright,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
