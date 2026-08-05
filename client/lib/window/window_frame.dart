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
        return Column(
          children: [
            // The bar needs a `Material` for its buttons' ink, and an `Overlay`
            // for their tooltips: this sits above the `Navigator`, so the app's
            // own overlay is *below* us and `Overlay.of` would find nothing.
            Material(
              type: MaterialType.transparency,
              child: Overlay.wrap(
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
            ),
            Expanded(child: widget.child),
          ],
        );
      },
    );
  }
}
