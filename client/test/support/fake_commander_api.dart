import 'dart:async';
import 'dart:typed_data';

import 'package:claude_commander_client/services/commander_api.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/src/rust/api/review.dart';

/// One recorded call: the method name plus its positional arg values, so tests
/// can assert both that a method fired and what it was passed.
class RecordedCall {
  final String method;
  final Map<String, Object?> args;
  const RecordedCall(this.method, this.args);

  @override
  String toString() => '$method($args)';
}

/// A hand-rolled [CommanderApi] for widget tests — no bridge, no mocktail.
///
/// Set the public `...Response` fields for canned results (each has a sensible
/// default so a test only overrides what it exercises); set a `...Error` field
/// to make that method throw. Every call is appended to [calls] and the
/// last-call args are also mirrored onto per-method fields for convenience.
/// `attachTerminal` returns a stream fed by [terminalController], which the test
/// drives with [emit].
class FakeCommanderApi implements CommanderApi {
  final List<RecordedCall> calls = [];

  // --- canned responses --------------------------------------------------
  bool healthResponse = true;
  bool healthTmuxResponse = true;
  List<SessionInfo> listSessionsResponse = const [];
  SessionDetail? getSessionDetailResponse;
  String? getPaneResponse;
  String createSessionResponse = 'new-session-id';
  ReviewSnapshotDto? openReviewResponse;
  ReviewSnapshotDto? refreshReviewResponse;
  List<CommentDto> listCommentsResponse = const [];
  String createCommentResponse = 'new-comment-id';
  ApplyResult applyCommentsResponse = const ApplyResult(
    kind: ApplyResultKind.applied,
    driftedIds: [],
    count: 1,
  );
  bool toggleFileReviewedResponse = true;
  Uint8List fetchBlobResponse = Uint8List(0);

  // --- error injection ---------------------------------------------------
  Object? healthError;
  Object? healthTmuxError;
  Object? listSessionsError;
  Object? getSessionDetailError;
  Object? createSessionError;
  Object? killSessionError;
  Object? restartSessionError;
  Object? deleteSessionError;
  Object? openReviewError;
  Object? createCommentError;
  Object? deleteCommentError;
  Object? applyCommentsError;
  Object? toggleFileReviewedError;

  // --- terminal ----------------------------------------------------------
  /// The controller behind the current [attachTerminal]. A test pushes events
  /// via [emit]. A fresh (single-subscription) controller is minted on each
  /// attach so a reconnect gets a clean stream.
  StreamController<TerminalEvent> terminalController =
      StreamController<TerminalEvent>();
  int attachTerminalCount = 0;

  void emit(TerminalEvent event) => terminalController.add(event);

  void _record(String method, [Map<String, Object?> args = const {}]) =>
      calls.add(RecordedCall(method, args));

  int countOf(String method) => calls.where((c) => c.method == method).length;

  RecordedCall? lastCall(String method) {
    for (final c in calls.reversed) {
      if (c.method == method) return c;
    }
    return null;
  }

  @override
  Future<bool> health({required String baseUrl}) async {
    _record('health', {'baseUrl': baseUrl});
    if (healthError != null) throw healthError!;
    return healthResponse;
  }

  @override
  Future<bool> healthTmux({
    required String baseUrl,
    required String token,
  }) async {
    _record('healthTmux', {'baseUrl': baseUrl, 'token': token});
    if (healthTmuxError != null) throw healthTmuxError!;
    return healthTmuxResponse;
  }

  @override
  Future<List<SessionInfo>> listSessions({
    required String baseUrl,
    required String token,
    required bool includeStopped,
  }) async {
    _record('listSessions', {'includeStopped': includeStopped});
    if (listSessionsError != null) throw listSessionsError!;
    return listSessionsResponse;
  }

  @override
  Future<SessionDetail?> getSessionDetail({
    required String baseUrl,
    required String token,
    required String query,
    int? lines,
  }) async {
    _record('getSessionDetail', {'query': query, 'lines': lines});
    if (getSessionDetailError != null) throw getSessionDetailError!;
    return getSessionDetailResponse;
  }

  @override
  Future<String?> getPane({
    required String baseUrl,
    required String token,
    required String query,
    int? lines,
  }) async {
    _record('getPane', {'query': query, 'lines': lines});
    return getPaneResponse;
  }

  @override
  Future<String> createSession({
    required String baseUrl,
    required String token,
    required String projectPath,
    required String title,
    String? program,
    String? initialPrompt,
    String? effort,
    String? mode,
    String? baseBranch,
  }) async {
    _record('createSession', {
      'projectPath': projectPath,
      'title': title,
      'program': program,
    });
    if (createSessionError != null) throw createSessionError!;
    return createSessionResponse;
  }

  @override
  Future<void> killSession({
    required String baseUrl,
    required String token,
    required String id,
  }) async {
    _record('killSession', {'id': id});
    if (killSessionError != null) throw killSessionError!;
  }

  @override
  Future<void> restartSession({
    required String baseUrl,
    required String token,
    required String id,
  }) async {
    _record('restartSession', {'id': id});
    if (restartSessionError != null) throw restartSessionError!;
  }

  @override
  Future<void> deleteSession({
    required String baseUrl,
    required String token,
    required String id,
  }) async {
    _record('deleteSession', {'id': id});
    if (deleteSessionError != null) throw deleteSessionError!;
  }

  @override
  Future<ReviewSnapshotDto> openReview({
    required String baseUrl,
    required String token,
    required String sessionId,
  }) async {
    _record('openReview', {'sessionId': sessionId});
    if (openReviewError != null) throw openReviewError!;
    final snap = openReviewResponse;
    if (snap == null) {
      throw StateError('openReviewResponse not set on FakeCommanderApi');
    }
    return snap;
  }

  @override
  Future<ReviewSnapshotDto?> refreshReview({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String prevHash,
  }) async {
    _record('refreshReview', {'sessionId': sessionId, 'prevHash': prevHash});
    return refreshReviewResponse;
  }

  @override
  Future<List<CommentDto>> listComments({
    required String baseUrl,
    required String token,
    required String sessionId,
  }) async {
    _record('listComments', {'sessionId': sessionId});
    return listCommentsResponse;
  }

  @override
  Future<String> createComment({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String file,
    required String side,
    required int lineStart,
    required int lineEnd,
    required String snippet,
    required String comment,
  }) async {
    _record('createComment', {
      'file': file,
      'side': side,
      'lineStart': lineStart,
      'lineEnd': lineEnd,
      'snippet': snippet,
      'comment': comment,
    });
    if (createCommentError != null) throw createCommentError!;
    return createCommentResponse;
  }

  @override
  Future<void> deleteComment({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String commentId,
  }) async {
    _record('deleteComment', {'commentId': commentId});
    if (deleteCommentError != null) throw deleteCommentError!;
  }

  @override
  Future<ApplyResult> applyComments({
    required String baseUrl,
    required String token,
    required String sessionId,
  }) async {
    _record('applyComments', {'sessionId': sessionId});
    if (applyCommentsError != null) throw applyCommentsError!;
    return applyCommentsResponse;
  }

  @override
  Future<bool> toggleFileReviewed({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String displayPath,
  }) async {
    _record('toggleFileReviewed', {'displayPath': displayPath});
    if (toggleFileReviewedError != null) throw toggleFileReviewedError!;
    return toggleFileReviewedResponse;
  }

  @override
  Future<Uint8List> fetchBlob({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String side,
    required String path,
  }) async {
    _record('fetchBlob', {'side': side, 'path': path});
    return fetchBlobResponse;
  }

  @override
  Stream<TerminalEvent> attachTerminal({
    required String handle,
    required String baseUrl,
    required String token,
    required String sessionId,
  }) {
    attachTerminalCount++;
    _record('attachTerminal', {'handle': handle, 'sessionId': sessionId});
    // Each attach gets a fresh single-subscription controller: the page cancels
    // its old subscription on reconnect, and a new listen needs a clean stream.
    terminalController = StreamController<TerminalEvent>();
    return terminalController.stream;
  }

  @override
  Future<void> terminalSendInput({
    required String handle,
    required List<int> bytes,
  }) async {
    _record('terminalSendInput', {'handle': handle, 'bytes': bytes});
  }

  @override
  Future<void> terminalResize({
    required String handle,
    required int cols,
    required int rows,
  }) async {
    _record('terminalResize', {'handle': handle, 'cols': cols, 'rows': rows});
  }

  @override
  Future<void> terminalDetach({required String handle}) async {
    _record('terminalDetach', {'handle': handle});
  }
}
