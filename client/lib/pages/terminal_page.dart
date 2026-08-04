import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:uuid/uuid.dart';
import 'package:xterm/xterm.dart';

import '../chrome/chrome.dart';
import '../services/clipboard_image_reader.dart';
import '../services/commander_api.dart';
import '../services/image_picker_service.dart';
import '../src/rust/api/mirrors.dart';
import '../state/commander_store.dart';
import '../state/commander_store_scope.dart';
import '../theme/terminal_theme.dart';
import '../theme/tokens.dart';

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
///
/// The attach is also re-opened when the app returns to the foreground, because a
/// backgrounded process cannot keep the attach alive: the server pings every
/// attached socket and kills the attach once too many pings go unanswered, and a
/// frozen Android process answers none of them. See [_onResumed] for why the
/// resume only re-attaches when the attach is *known* dead rather than always.
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

  /// Image sources for the attach-image action. Injectable because both drive
  /// platform channels a widget test cannot exercise; `null` means "use the real
  /// platform implementation".
  final ImagePickerService? imagePicker;
  final ClipboardImageReader? clipboardImages;

  /// Wall clock for the how-long-were-we-away measurement, injectable so a widget
  /// test can cross the heartbeat deadline without waiting a real minute; `null`
  /// means [DateTime.now].
  final DateTime Function()? clock;

  const TerminalBody({
    super.key,
    required this.api,
    required this.handle,
    required this.session,
    this.kind = AttachKind.agent,
    this.showModifierBar = true,
    this.imagePicker,
    this.clipboardImages,
    this.clock,
  });

  @override
  State<TerminalBody> createState() => _TerminalBodyState();
}

class _TerminalBodyState extends State<TerminalBody>
    with WidgetsBindingObserver {
  late final Terminal _terminal;

  /// Owned rather than left to `TerminalView` to create, so the Ctrl+V
  /// text-paste fallback can clear the selection the way xterm's own paste
  /// action does. Because we pass it in, we own disposing it.
  final TerminalController _terminalController = TerminalController();
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

  /// True from the moment an attach-image action starts until it finishes —
  /// covering the clipboard read, the picker round trip and the upload. Set
  /// **synchronously** at each entry point, before the first `await`, so two
  /// fast Ctrl+V presses (or a press racing the bottom sheet) can't both get
  /// through and inject the path twice. A flag set only once the upload began
  /// would leave exactly that window open, since the clipboard read is itself a
  /// platform round trip with a multi-second timeout.
  bool _imageBusy = false;

  /// True only around the upload itself, which is what the spinner reports.
  /// Deliberately narrower than [_imageBusy]: a spinner while the bottom sheet
  /// or the OS picker is in front of the user tells them nothing, and an
  /// indeterminate progress indicator animates forever, so widening it would
  /// also stop `pumpAndSettle` ever settling in widget tests.
  bool _uploading = false;

  late final ImagePickerService _imagePicker =
      widget.imagePicker ?? PlatformImagePicker();
  late final ClipboardImageReader _clipboardImages =
      widget.clipboardImages ?? const SuperClipboardImageReader();

  /// Whether this attach can take an image. The server always injects the path
  /// into the session's *agent* pane, so offering it on a shell attach would
  /// type into a pane the user isn't looking at.
  bool get _canAttachImage => widget.kind == AttachKind.agent;

  /// Wall clock, not a [Stopwatch]: on Android a device in deep sleep stops
  /// advancing the monotonic clock a stopwatch reads, while the server's
  /// heartbeat teardown happens in real time — so a monotonic measure would
  /// under-report exactly the long absences this exists to catch.
  late final DateTime Function() _now = widget.clock ?? DateTime.now;

  /// How long a *silent* client can be away before the server has certainly
  /// killed the attach, from the shared wire contract. Null until the bridge
  /// answers (or if it fails), and treated as "don't guess": without it, a resume
  /// only re-attaches something already reported dead.
  Duration? _deadAfter;

  /// When the app last dropped out of the foreground, or null while it is in
  /// front.
  DateTime? _leftForegroundAt;

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

    WidgetsBinding.instance.addObserver(this);
    unawaited(_loadDeadAfter());

    _meter = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      setState(() {
        _bytesPerSec = _windowBytes;
        _windowBytes = 0;
      });
    });
  }

  /// Cache the heartbeat deadline. No [setState]: nothing renders from it.
  Future<void> _loadDeadAfter() async {
    try {
      final deadAfter = await widget.api.attachDeadAfter();
      if (mounted) _deadAfter = deadAfter;
    } catch (_) {
      // Leave it null. A resume then only re-attaches an attach we were *told*
      // had ended — never one we merely suspect, since without the contract's
      // deadline we'd be guessing at the cost of the user's scrollback position.
    }
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    switch (state) {
      case AppLifecycleState.paused:
        // `paused` is the marker, not `inactive`: it is the earliest point at
        // which Android may freeze the process and stop our heartbeat pongs. It
        // is only an upper bound on when they actually stop (see [_onResumed]).
        // `inactive` would be worse: it also fires for a pulled-down notification
        // shade or a permission dialog, where the app keeps answering pings and
        // the attach is fine, so starting the clock there would re-attach healthy
        // sockets. `??=` keeps the earliest of a repeated pause.
        _leftForegroundAt ??= _now();
      case AppLifecycleState.resumed:
        _onResumed();
      // `hidden`/`inactive` are transient steps on the way to and from `paused`,
      // and `detached` means we're being torn down.
      case AppLifecycleState.hidden:
      case AppLifecycleState.inactive:
      case AppLifecycleState.detached:
        break;
    }
  }

  /// Back in the foreground: re-open the attach when it is dead, or long enough
  /// gone that it almost certainly is.
  ///
  /// Two triggers. Either the attach already reported detached/error (possibly
  /// delivered while we were away), which is certain; or we were away longer than
  /// the server's heartbeat tolerance, which no *silent* attach survives. The
  /// second is what a half-open socket needs: when the network path vanishes
  /// without a TCP FIN, no detach frame ever arrives and the UI would otherwise
  /// sit on a frozen pane that still claims to be attached.
  ///
  /// The second trigger is a heuristic, not a proof, because `paused` is not the
  /// same event as *frozen*: Android does not stop the cdylib's tokio threads at
  /// `paused`, so they keep answering pings until the cached-app freezer actually
  /// hits — which can lag by minutes, or never come (a paused-but-visible app in
  /// legacy split-screen, some OEM/charging configurations). Such a resume
  /// re-attaches a live socket and costs a scrolled copy-mode position. There is
  /// no client-observable freeze signal to do better with, and the alternative
  /// failure — coming back to a permanently dead pane — is the bug being fixed.
  ///
  /// A shorter absence deliberately changes nothing: the attach is probably still
  /// live, and re-attaching spawns a fresh `tmux attach-session` child, so a glance
  /// at a notification must not cost the user their place in the scrollback.
  void _onResumed() {
    final leftAt = _leftForegroundAt;
    _leftForegroundAt = null;
    if (_ended) {
      _connect();
      return;
    }
    final deadAfter = _deadAfter;
    if (leftAt == null || deadAfter == null) return;
    if (_now().difference(leftAt) > deadAfter) _connect();
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
  ///
  /// The outgoing attach is detached explicitly, because cancelling `_sub` alone
  /// does not stop it: its cdylib registry entry keeps the pump's control sender
  /// alive, so the pump's `rx.recv()` never ends, and the pump otherwise only
  /// learns Dart is gone by failing to push an Output frame — which never comes on
  /// an idle pane, and *never* on the half-open socket the reconnect button exists
  /// to escape. That left a zombie pump holding the WS open and still answering
  /// the server's pings, so the server kept its `tmux attach-session` child alive.
  /// Both callers can now run against a live attach (the always-enabled button, a
  /// resume past the deadline), so this is no longer a dead-attach-only path.
  void _connect() {
    _sub?.cancel();
    // A documented no-op for an id that was never attached, which covers both the
    // initial call from `initState` and an already-ended attach.
    unawaited(widget.api.terminalDetach(attachId: _attachId));
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

  // -- image attach --------------------------------------------------------

  /// Offer the available image sources and act on the choice. Camera only
  /// appears where the platform supports it (Android); Linux gets the file
  /// dialog and the clipboard.
  Future<void> _attachImage() async {
    if (_imageBusy) return;
    setState(() => _imageBusy = true);
    try {
      await _pickAndAttach();
    } finally {
      if (mounted) setState(() => _imageBusy = false);
    }
  }

  Future<void> _pickAndAttach() async {
    final source = await showModalBottomSheet<_ImageSource>(
      context: context,
      backgroundColor: CommanderTokens.of(context).canvasRaised,
      builder: (sheetContext) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            _sheetTile(
              sheetContext,
              Icons.photo_library_outlined,
              _imagePicker.supportsCamera ? 'Photo library' : 'Choose file',
              _ImageSource.gallery,
            ),
            if (_imagePicker.supportsCamera)
              _sheetTile(
                sheetContext,
                Icons.photo_camera_outlined,
                'Take photo',
                _ImageSource.camera,
              ),
            _sheetTile(
              sheetContext,
              Icons.content_paste,
              'Paste from clipboard',
              _ImageSource.clipboard,
            ),
          ],
        ),
      ),
    );
    if (source == null) return;
    await _attachFrom(source);
  }

  Widget _sheetTile(
    BuildContext sheetContext,
    IconData icon,
    String label,
    _ImageSource source,
  ) => ListTile(
    leading: Icon(
      icon,
      color: CommanderTokens.of(sheetContext).textMuted,
      size: 20,
    ),
    title: Text(label),
    onTap: () => Navigator.of(sheetContext).pop(source),
  );

  /// Resolve `source` to bytes and upload them. Cancellation is silent; every
  /// other failure surfaces as a snackbar.
  Future<void> _attachFrom(_ImageSource source) async {
    try {
      final bytes = source == _ImageSource.clipboard
          ? await _readClipboardImage()
          : await _readPickedImage(source);
      if (bytes == null) return;
      await _uploadImage(bytes);
    } catch (e) {
      _notify('Could not attach image: $e');
    }
  }

  /// Clipboard bytes, or null (with a note) when it holds no image.
  Future<Uint8List?> _readClipboardImage() async {
    final bytes = await _clipboardImages.readImage();
    if (bytes == null) {
      _notify('No image on the clipboard');
    }
    return bytes;
  }

  /// Picked-file bytes, or null when the user cancelled or the file is over the
  /// cap. Size is checked from the file *length* first, so a huge phone photo is
  /// refused without being read into memory.
  Future<Uint8List?> _readPickedImage(_ImageSource source) async {
    final file = await _imagePicker.pick(
      source == _ImageSource.camera
          ? ImagePickSource.camera
          : ImagePickSource.gallery,
    );
    if (file == null) return null; // cancelled
    final maxBytes = await widget.api.imageMaxBytes();
    final length = await file.length();
    if (length > maxBytes) {
      _notify(
        'Image is ${_fmtSize(length)} — the limit is ${_fmtSize(maxBytes)}',
      );
      return null;
    }
    return file.readAsBytes();
  }

  /// Upload to the agent pane. No success message: the server types the path
  /// into the pane, so it arrives on screen through the attach output stream.
  /// The re-entrancy guard ([_imageBusy]) is owned by the callers
  /// ([_attachImage] / [_pasteClipboard]), which set it before their first
  /// `await`; this only drives the spinner.
  Future<void> _uploadImage(Uint8List bytes) async {
    if (mounted) setState(() => _uploading = true);
    try {
      await widget.api.pasteImage(
        handle: widget.handle,
        id: widget.session.id,
        bytes: bytes,
      );
    } finally {
      if (mounted) setState(() => _uploading = false);
    }
  }

  /// Ctrl+V: attach a clipboard image if there is one, otherwise fall back to
  /// the plain text paste that `xterm` would have done.
  ///
  /// `xterm` binds Ctrl+V to `PasteTextIntent`, handled by a `TerminalActions`
  /// widget *inside* `TerminalView` — so an outer `Actions` override would be
  /// shadowed. `TerminalView.onKeyEvent` has higher priority than both its
  /// shortcuts and its input handler, which makes it the one place this can be
  /// intercepted; that also means the text-paste fallback has to be reproduced
  /// here, since pre-empting the key skips xterm's own handler.
  KeyEventResult _onKeyEvent(FocusNode node, KeyEvent event) {
    // Alt/Meta must be excluded, not ignored: xterm's own activator requires
    // them absent, so Ctrl+Meta+V previously reached the PTY as 0x16. Matching
    // loosely here would silently steal that.
    final keyboard = HardwareKeyboard.instance;
    final isPasteChord =
        event.logicalKey == LogicalKeyboardKey.keyV &&
        keyboard.isControlPressed &&
        !keyboard.isShiftPressed &&
        !keyboard.isAltPressed &&
        !keyboard.isMetaPressed;
    if (!isPasteChord || !_canAttachImage || _ended) {
      return KeyEventResult.ignored;
    }
    // `KeyRepeatEvent` is a *sibling* of `KeyDownEvent`, not a subclass, so a
    // held key must be matched explicitly — and it must still be swallowed.
    // xterm's `SingleActivator` defaults to `includeRepeats: true`, so letting a
    // repeat through would fire its text paste on every tick while our upload
    // was still running.
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    if (event is KeyRepeatEvent || _imageBusy) return KeyEventResult.handled;
    unawaited(_pasteClipboard());
    return KeyEventResult.handled;
  }

  /// Sets [_imageBusy] synchronously before its first `await`, so a second press
  /// arriving during the clipboard read is dropped by [_onKeyEvent].
  Future<void> _pasteClipboard() async {
    if (_imageBusy) return;
    setState(() => _imageBusy = true);
    try {
      final image = await _clipboardImages.readImage();
      if (image != null) {
        await _uploadImage(image);
        return;
      }
    } catch (e) {
      _notify('Could not attach image: $e');
      return;
    } finally {
      if (mounted) setState(() => _imageBusy = false);
    }
    // No image — behave exactly as xterm's own Ctrl+V would have, including
    // clearing the selection (see `TerminalActions`' PasteTextIntent handler).
    // Pre-empting the key skips xterm's handler, so this fidelity is ours to keep.
    final text = (await Clipboard.getData(Clipboard.kTextPlain))?.text;
    if (text != null && text.isNotEmpty) {
      _terminal.paste(text);
      _terminalController.clearSelection();
    }
  }

  /// `maybeOf`, not `of`: this widget also renders inside desktop panes, and a
  /// missing messenger must not turn a minor notice into a crash.
  void _notify(String message) {
    if (!mounted) return;
    ScaffoldMessenger.maybeOf(
      context,
    )?.showSnackBar(SnackBar(content: Text(message)));
  }

  /// MiB, not MB: the cap is a binary quantity (`MAX_IMAGE_BYTES` is
  /// `10 * 1024 * 1024`), so dividing by 1024² and calling it "MB" would misstate
  /// the limit the user is being held to.
  static String _fmtSize(int bytes) =>
      '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MiB';

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _meter?.cancel();
    unawaited(widget.api.terminalDetach(attachId: _attachId));
    // Guarded clear: if the wide pane already swapped in another attach (agent↔
    // shell), its initState registered the new id before this dispose runs, so
    // only clear when we're still the registered attach.
    _store?.clearActiveTerminalAttach(_attachId);
    _sub?.cancel();
    _decoder.close();
    // Ours to dispose: `TerminalView` only disposes a controller it created
    // itself, and we pass one in.
    _terminalController.dispose();
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
    final t = CommanderTokens.of(context);

    return ColoredBox(
      color: t.terminalBg,
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
                        theme: terminalThemeFor(t),
                        textStyle: TerminalStyle(fontFamily: t.mono),
                        padding: const EdgeInsets.symmetric(
                          horizontal: 14,
                          vertical: 8,
                        ),
                        controller: _terminalController,
                        onKeyEvent: _onKeyEvent,
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

  /// The dot colour reflects the link state: the working accent while attached,
  /// danger once the attach has ended (reconnect offered), attention while
  /// connecting.
  Color get _statusColor {
    final t = CommanderTokens.of(context);
    if (_ended) return t.danger;
    if (_status.startsWith('attached')) return t.working;
    return t.attention;
  }

  Widget _statusBar(BuildContext context) {
    final t = CommanderTokens.of(context);
    return Container(
      padding: const EdgeInsets.only(left: 14, right: 4, top: 4, bottom: 4),
      decoration: BoxDecoration(
        color: t.canvasRaised,
        border: Border(bottom: BorderSide(color: t.borderSubtle)),
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
              style: t.meta(size: 10, color: t.textMuted),
            ),
          ),
          Text(
            '${_fmtRate(_bytesPerSec)} · ${_totalBytes ~/ 1024} KB',
            style: t.meta(size: 10, color: t.textFaint),
          ),
          // Agent attaches only: the server injects the image path into the
          // agent pane, so on a shell attach this would type somewhere the user
          // can't see. Lives here rather than in the modifier bar so desktop
          // layouts — which run without that bar — get it too.
          if (_canAttachImage)
            IconButton(
              visualDensity: VisualDensity.compact,
              onPressed: _imageBusy || _ended ? null : _attachImage,
              icon: _uploading
                  ? SizedBox.square(
                      dimension: 18,
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        color: t.textMuted,
                      ),
                    )
                  : const Icon(Icons.image_outlined, size: 18),
              color: t.textMuted,
              disabledColor: t.textDim,
              tooltip: 'Attach image',
            ),
          // Never gated on [_ended]. A half-open socket — the network path gone
          // without a TCP FIN, so no detach frame ever arrives — leaves the UI
          // reading "attached" over a frozen pane, and that is precisely when the
          // user needs this button. Disabling it there turns a recoverable stall
          // into a dead end.
          IconButton(
            visualDensity: VisualDensity.compact,
            onPressed: _reconnect,
            icon: const Icon(Icons.refresh, size: 18),
            color: t.textMuted,
            tooltip: 'Reconnect',
          ),
        ],
      ),
    );
  }
}

/// The phone (stacked-navigation) terminal screen: a [ChromePage] titled by the
/// session, wrapping a [TerminalBody] with the on-screen modifier bar enabled.
class TerminalPage extends StatelessWidget {
  final CommanderApi api;
  final String handle;
  final SessionInfo session;

  /// Which pane to attach to: the agent pane (default) or the paired shell.
  final AttachKind kind;

  /// Forwarded to [TerminalBody] so tests can inject fake image sources; `null`
  /// means "use the real platform implementation".
  final ImagePickerService? imagePicker;
  final ClipboardImageReader? clipboardImages;

  /// Forwarded to [TerminalBody] so a test can drive the foreground-reconnect
  /// deadline; `null` means [DateTime.now].
  final DateTime Function()? clock;

  const TerminalPage({
    super.key,
    required this.api,
    required this.handle,
    required this.session,
    this.kind = AttachKind.agent,
    this.imagePicker,
    this.clipboardImages,
    this.clock,
  });

  @override
  Widget build(BuildContext context) {
    final isShell = kind == AttachKind.shell;
    return ChromePage(
      code: '47-T',
      title: isShell ? '${session.title} · shell' : session.title,
      // The keyboard must not shrink the body: [TerminalBody] insets its own
      // chrome and pans the pane instead, so the remote PTY never sees a resize.
      // ChromeInsets.pan *is* main's resizeToAvoidBottomInset:false plus
      // SafeArea(maintainBottomViewPadding: true) — see applyChromeInsets. It
      // lives in the chrome so LCARS cannot diverge from it.
      insets: ChromeInsets.pan,
      body: TerminalBody(
        api: api,
        handle: handle,
        session: session,
        kind: kind,
        imagePicker: imagePicker,
        clipboardImages: clipboardImages,
        clock: clock,
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

/// Where the attach-image action should get its bytes from. Distinct from
/// [ImagePickSource] because the clipboard isn't a picker source.
enum _ImageSource { gallery, camera, clipboard }

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
    final t = CommanderTokens.of(context);
    // No SafeArea here: [TerminalBody] already insets the bottom chrome, and a
    // bar whose height changed with the keyboard would change the pane's rows.
    return Container(
      height: 48,
      decoration: BoxDecoration(
        color: t.terminalBg,
        border: Border(top: BorderSide(color: t.borderSubtle)),
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
    final t = CommanderTokens.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 3),
      child: Material(
        color: t.surface,
        borderRadius: BorderRadius.circular(7),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(7),
          child: Container(
            alignment: Alignment.center,
            padding: const EdgeInsets.symmetric(horizontal: 12),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(7),
              border: Border.all(color: t.border),
            ),
            child: Text(
              label,
              style: t.meta(
                size: 10.5,
                weight: FontWeight.w600,
                color: t.textBright,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
