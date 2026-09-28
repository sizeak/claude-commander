import 'dart:async';

import 'package:claude_commander_client/services/window_service.dart';

/// A hand-rolled [WindowService] for tests — no plugin, no platform channel.
///
/// Records every call in [calls] (the same shape [FakeCommanderApi] uses), holds
/// the bounds [WindowController] reads back, and lets a test push the events a
/// real desktop would emit via [emit].
class FakeWindowService implements WindowService {
  final List<String> calls = [];
  final _events = StreamController<WindowEvent>.broadcast();

  /// What [getBounds] answers. A test that expects a geometry save asserts the
  /// persisted string against this.
  WindowBounds bounds = const WindowBounds(
    x: 0,
    y: 0,
    width: 1280,
    height: 720,
  );

  bool maximized = false;
  bool fullScreen = false;
  bool titleBarHidden = false;
  String? title;

  /// When set, the named method throws — for the "a failing window must not stop
  /// the app" cases.
  String? failingMethod;

  @override
  Stream<WindowEvent> get events => _events.stream;

  void emit(WindowEvent event) => _events.add(event);

  void _record(String method) {
    calls.add(method);
    if (method == failingMethod) throw StateError('fake window failure');
  }

  @override
  Future<void> initialize() async => _record('initialize');

  @override
  Future<void> setTitle(String value) async {
    _record('setTitle($value)');
    title = value;
  }

  @override
  Future<void> setTitleBarHidden(bool hidden) async {
    _record('setTitleBarHidden($hidden)');
    titleBarHidden = hidden;
  }

  @override
  Future<void> setFullScreen(bool value) async {
    _record('setFullScreen($value)');
    fullScreen = value;
  }

  @override
  Future<WindowBounds> getBounds() async {
    _record('getBounds');
    return bounds;
  }

  @override
  Future<void> setBounds(WindowBounds value) async {
    _record('setBounds(${value.wire})');
    bounds = value;
  }

  @override
  Future<bool> isMaximized() async {
    _record('isMaximized');
    return maximized;
  }

  @override
  Future<void> setMaximized(bool value) async {
    _record('setMaximized($value)');
    maximized = value;
  }

  @override
  Future<void> minimize() async => _record('minimize');

  @override
  Future<void> startDragging() async => _record('startDragging');

  @override
  Future<void> close() async => _record('close');

  void dispose() => _events.close();
}
