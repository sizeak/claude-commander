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
      HardwareKeyboard.instance.addHandler(_onKey);
    } else if (_controller != null && controller == null) {
      HardwareKeyboard.instance.removeHandler(_onKey);
    }
    _controller = controller;
  }

  @override
  void dispose() {
    if (_controller != null) HardwareKeyboard.instance.removeHandler(_onKey);
    super.dispose();
  }

  /// F11 toggles fullscreen, Shift+F11 the window frame.
  ///
  /// A [HardwareKeyboard] handler rather than a `Shortcuts` widget: these run
  /// *before* the focus tree, so F11 reaches us ahead of the terminal view, which
  /// would otherwise forward it to the remote PTY and never let it out.
  bool _onKey(KeyEvent event) {
    final controller = _controller;
    if (controller == null) return false;
    if (event is! KeyDownEvent) return false;
    if (event.logicalKey != LogicalKeyboardKey.f11) return false;
    if (HardwareKeyboard.instance.isShiftPressed) {
      controller.toggleTitleBar();
    } else {
      controller.toggleFullscreen();
    }
    return true;
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
