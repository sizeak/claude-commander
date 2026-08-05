import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../chrome/chrome_forms.dart';
import 'window_controller.dart';

/// Puts the app's own window bar above the whole app, and owns the F11 shortcuts.
///
/// Mount it from `MaterialApp.builder`, which is inside the app's `Theme` (so
/// `CommanderTokens.of` resolves) and above the `Navigator` (so the bar is not
/// covered by a pushed route).
///
/// With no [WindowController] in scope — Android, or any platform with no window
/// to manage — this is a pass-through: no bar, no key handler, nothing to check.
class WindowFrame extends StatefulWidget {
  final Widget child;

  const WindowFrame({super.key, required this.child});

  @override
  State<WindowFrame> createState() => _WindowFrameState();
}

class _WindowFrameState extends State<WindowFrame> {
  WindowController? _controller;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final controller = WindowScope.of(context);
    if (controller == _controller) return;
    // Only carry a global key handler while there is a window it can act on.
    if (_controller == null && controller != null) {
      FocusManager.instance.addEarlyKeyEventHandler(_onKey);
    } else if (_controller != null && controller == null) {
      FocusManager.instance.removeEarlyKeyEventHandler(_onKey);
    }
    _controller = controller;
  }

  @override
  void dispose() {
    if (_controller != null) {
      FocusManager.instance.removeEarlyKeyEventHandler(_onKey);
    }
    super.dispose();
  }

  /// F11 toggles fullscreen, Shift+F11 the window frame.
  ///
  /// An **early** [FocusManager] handler, and the choice is load-bearing twice
  /// over. A `Shortcuts` widget would sit above the focused terminal view, which
  /// maps F11 to `TerminalKey.f11` and forwards the escape sequence to the remote
  /// PTY — the key would toggle the window *and* type `\x1b[23~` into the user's
  /// agent session. A [HardwareKeyboard] handler does not fix that either,
  /// despite running first: `KeyEventManager` calls `_dispatchKeyMessage`
  /// unconditionally afterwards, so returning true there only answers the engine
  /// and the focus tree still gets the event. Early handlers run before the focus
  /// walk *and* [KeyEventResult.handled] short-circuits it.
  KeyEventResult _onKey(KeyEvent event) {
    final controller = _controller;
    if (controller == null) return KeyEventResult.ignored;
    if (event.logicalKey != LogicalKeyboardKey.f11) {
      return KeyEventResult.ignored;
    }
    // Act on the press only — a held key must not thrash the window — but claim
    // every F11 event, repeats and the release included. Letting those through
    // would hand the terminal the very sequence this exists to stop.
    if (event is KeyDownEvent) {
      if (HardwareKeyboard.instance.isShiftPressed) {
        controller.toggleTitleBar();
      } else {
        controller.toggleFullscreen();
      }
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final controller = _controller;
    if (controller == null) return widget.child;
    return ListenableBuilder(
      listenable: controller,
      builder: (context, _) {
        if (!controller.showWindowBar) return widget.child;
        // The bar's controls need an `Overlay` for their tooltips, because this
        // sits above the `Navigator` and the app's own overlay is *below* us, so
        // `Overlay.of` finds nothing. It wraps the whole column rather than just
        // the bar: an overlay the height of the bar gives a tooltip a 32px box to
        // live in, which paints the label and then clips it to a sliver.
        return Overlay.wrap(
          child: Column(
            children: [
              // A `Material` for the buttons' ink, which the bar is otherwise
              // outside of — the app's own starts inside the routes below.
              Material(
                type: MaterialType.transparency,
                child: ChromeWindowBar(
                  ChromeWindowBarSpec(
                    title: WindowController.windowTitle,
                    isMaximized: controller.maximized,
                    onDragStart: controller.startDragging,
                    onDoubleTap: controller.toggleMaximize,
                    onMinimize: controller.minimize,
                    onToggleMaximize: controller.toggleMaximize,
                    onClose: controller.close,
                  ),
                ),
              ),
              Expanded(child: widget.child),
            ],
          ),
        );
      },
    );
  }
}
