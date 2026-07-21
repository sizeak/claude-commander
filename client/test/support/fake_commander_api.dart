import 'dart:async';
import 'dart:typed_data';

import 'package:claude_commander_client/services/commander_api.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/src/rust/api/review.dart';
import 'package:claude_commander_client/src/rust/api/simple.dart'
    show ScanResultDto;

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
/// to make that method throw. Every call is appended to [calls]. `attachTerminal`
/// returns a stream fed by [terminalController], which the test drives with
/// [emit].
class FakeCommanderApi implements CommanderApi {
  final List<RecordedCall> calls = [];

  // --- canned responses --------------------------------------------------
  String connectServerResponse = 'fake-handle';

  /// When set, `connectServer` awaits this before returning — lets a test hold a
  /// connect in flight (e.g. to dispose the store mid-connect).
  Completer<void>? connectGate;
  Object? workspaceSnapshotError;
  bool healthResponse = true;
  bool healthTmuxResponse = true;
  List<SessionInfo> listSessionsResponse = const [];
  AgentStatesSnapshotDto agentStatesResponse = const AgentStatesSnapshotDto(
    states: [],
    commanderRunning: false,
  );
  PreviewDataDto previewResponse = const PreviewDataDto(diffText: '');
  CreateOptions createOptionsResponse = const CreateOptions(
    defaultProgram: 'claude',
    programs: [],
    sections: [],
  );
  List<BranchInfo> listBranchesResponse = const [];
  List<SessionId> pendingCommentSessionsResponse = const [];
  ScanResultDto scanDirectoryResponse = const ScanResultDto(
    added: 0,
    skipped: 0,
  );
  OperationStatusDto operationStatusResponse = OperationStatusDto(
    id: BigInt.zero,
    kind: OperationKind.cascade,
    outcome: const OperationOutcomeDto(
      kind: OperationOutcomeKind.succeeded,
      detail: '',
    ),
  );
  SessionDetail? getSessionDetailResponse;
  String createSessionResponse = 'new-session-id';
  String addProjectResponse = 'new-project-id';
  ReviewSnapshotDto? openReviewResponse;
  ReviewSnapshotDto? refreshReviewResponse;
  String createCommentResponse = 'new-comment-id';
  ApplyResult applyCommentsResponse = const ApplyResult(
    kind: ApplyResultKind.applied,
    driftedIds: [],
    count: 1,
  );
  bool toggleKeepAliveResponse = false;
  bool toggleFileReviewedResponse = true;
  Uint8List fetchBlobResponse = Uint8List(0);

  /// The session whose cascade is paused, surfaced in the workspace snapshot.
  /// Null (the default) means no cascade is paused.
  SessionId? cascadePausedResponse;

  /// Explicit project list for the snapshot. When null (the default) projects
  /// are synthesized from [listSessionsResponse]; set it to render projects that
  /// have no sessions (e.g. the projects manager).
  List<ProjectInfoDto>? projectsResponse;

  /// The default workspace snapshot echoes [listSessionsResponse] so a test that
  /// only sets sessions gets a coherent snapshot for free. It synthesizes one
  /// project per distinct session `projectId` (in first-seen order) so grouped
  /// views — which read `sessionsByProject` — have projects to group under, as a
  /// real server snapshot always would.
  WorkspaceSnapshotDto get workspaceSnapshotResponse {
    final projects = <String, ProjectInfoDto>{};
    for (final s in listSessionsResponse) {
      projects.putIfAbsent(
        s.projectId.field0.uuid,
        () => ProjectInfoDto(
          id: s.projectId,
          name: s.projectName,
          repoPath: '/repo/${s.projectName}',
          mainBranch: 'main',
          sessionIds: const [],
        ),
      );
    }
    return WorkspaceSnapshotDto(
      projects: projectsResponse ?? projects.values.toList(),
      sessions: listSessionsResponse,
      cascadePaused: cascadePausedResponse,
      pendingCommentSessions: const [],
      projectPull: const [],
      operations: const [],
      server: const ServerStatus(
        ghAvailable: true,
        tmuxOk: true,
        version: '0.0.0-test',
      ),
    );
  }

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

  // --- feeds -------------------------------------------------------------
  /// Broadcast so a test can push events without a buffered listener, and so a
  /// reconnect (which re-listens) works against the same controller. A test
  /// drives them via [emitChange] / [emitConnection].
  final StreamController<BigInt> changeController =
      StreamController<BigInt>.broadcast();
  final StreamController<ConnectionStateDto> connectionController =
      StreamController<ConnectionStateDto>.broadcast();

  void emitChange([BigInt? generation]) =>
      changeController.add(generation ?? BigInt.one);

  void emitConnection(ConnectionStateDto state) =>
      connectionController.add(state);

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
  Future<String> connectServer({required String baseUrl, String? token}) async {
    _record('connectServer', {'baseUrl': baseUrl, 'token': token});
    if (connectGate != null) await connectGate!.future;
    return connectServerResponse;
  }

  @override
  Future<void> disconnectServer({required String handle}) async {
    _record('disconnectServer', {'handle': handle});
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
  Future<WorkspaceSnapshotDto> workspaceSnapshot({
    required String handle,
  }) async {
    _record('workspaceSnapshot', {'handle': handle});
    if (workspaceSnapshotError != null) throw workspaceSnapshotError!;
    return workspaceSnapshotResponse;
  }

  @override
  Future<AgentStatesSnapshotDto> agentStates({
    required String handle,
    required bool fresh,
  }) async {
    _record('agentStates', {'fresh': fresh});
    return agentStatesResponse;
  }

  @override
  Future<List<SessionInfo>> listSessions({
    required String handle,
    required bool includeStopped,
  }) async {
    _record('listSessions', {'includeStopped': includeStopped});
    if (listSessionsError != null) throw listSessionsError!;
    return listSessionsResponse;
  }

  @override
  Future<SessionDetail?> getSessionDetail({
    required String handle,
    required String query,
    int? lines,
  }) async {
    _record('getSessionDetail', {'query': query, 'lines': lines});
    if (getSessionDetailError != null) throw getSessionDetailError!;
    return getSessionDetailResponse;
  }

  @override
  Future<PreviewDataDto> sessionPreview({
    required String handle,
    required String id,
    int? lines,
  }) async {
    _record('sessionPreview', {'id': id, 'lines': lines});
    return previewResponse;
  }

  @override
  Future<PreviewDataDto> projectPreview({
    required String handle,
    required String id,
  }) async {
    _record('projectPreview', {'id': id});
    return previewResponse;
  }

  @override
  Future<String> branchDiff({
    required String handle,
    required String id,
  }) async {
    _record('branchDiff', {'id': id});
    return previewResponse.diffText;
  }

  @override
  Future<List<BranchInfo>> listBranches({
    required String handle,
    required String projectId,
    required bool fetch,
  }) async {
    _record('listBranches', {'projectId': projectId, 'fetch': fetch});
    return listBranchesResponse;
  }

  @override
  Future<CreateOptions> createOptions({required String handle}) async {
    _record('createOptions', {'handle': handle});
    return createOptionsResponse;
  }

  /// The program list passed to the most recent [setPrograms] call, so a test
  /// can assert the exact rows saved (not just the count).
  List<ProgramInfo>? lastSetPrograms;

  @override
  Future<void> setPrograms({
    required String handle,
    required List<ProgramInfo> programs,
  }) async {
    lastSetPrograms = programs;
    _record('setPrograms', {'programs': programs.length});
  }

  @override
  Future<List<SessionId>> pendingCommentSessions({
    required String handle,
  }) async {
    _record('pendingCommentSessions', {'handle': handle});
    return pendingCommentSessionsResponse;
  }

  @override
  Future<String> createSession({
    required String handle,
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
  Future<void> killSession({required String handle, required String id}) async {
    _record('killSession', {'id': id});
    if (killSessionError != null) throw killSessionError!;
  }

  @override
  Future<void> restartSession({
    required String handle,
    required String id,
  }) async {
    _record('restartSession', {'id': id});
    if (restartSessionError != null) throw restartSessionError!;
  }

  @override
  Future<void> deleteSession({
    required String handle,
    required String id,
  }) async {
    _record('deleteSession', {'id': id});
    if (deleteSessionError != null) throw deleteSessionError!;
  }

  @override
  Future<void> renameSession({
    required String handle,
    required String id,
    required String title,
  }) async {
    _record('renameSession', {'id': id, 'title': title});
  }

  Object? setSectionError;

  @override
  Future<void> setSection({
    required String handle,
    required String id,
    String? section,
  }) async {
    _record('setSection', {'id': id, 'section': section});
    if (setSectionError != null) throw setSectionError!;
  }

  @override
  Future<void> markRead({required String handle, required String id}) async {
    _record('markRead', {'id': id});
  }

  @override
  Future<void> markUnread({
    required String handle,
    required List<String> ids,
  }) async {
    _record('markUnread', {'ids': ids});
  }

  @override
  Future<bool> toggleKeepAlive({
    required String handle,
    required String id,
  }) async {
    _record('toggleKeepAlive', {'id': id});
    return toggleKeepAliveResponse;
  }

  @override
  Future<String> addProject({
    required String handle,
    required String path,
  }) async {
    _record('addProject', {'path': path});
    return addProjectResponse;
  }

  @override
  Future<void> removeProject({
    required String handle,
    required String id,
  }) async {
    _record('removeProject', {'id': id});
  }

  @override
  Future<ScanResultDto> scanDirectory({
    required String handle,
    required String path,
  }) async {
    _record('scanDirectory', {'path': path});
    return scanDirectoryResponse;
  }

  @override
  Future<OperationStatusDto> cascadeMerge({
    required String handle,
    required String id,
  }) async {
    _record('cascadeMerge', {'id': id});
    return operationStatusResponse;
  }

  @override
  Future<OperationStatusDto> pushStack({
    required String handle,
    required String id,
  }) async {
    _record('pushStack', {'id': id});
    return operationStatusResponse;
  }

  @override
  Future<OperationStatusDto> cascadeResume({required String handle}) async {
    _record('cascadeResume', {'handle': handle});
    return operationStatusResponse;
  }

  @override
  Future<void> cascadeAbandon({required String handle}) async {
    _record('cascadeAbandon', {'handle': handle});
  }

  @override
  Future<void> requestPrRefresh({required String handle}) async {
    _record('requestPrRefresh', {'handle': handle});
  }

  @override
  Future<ReviewSnapshotDto> openReview({
    required String handle,
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
    required String handle,
    required String sessionId,
    required String prevHash,
  }) async {
    _record('refreshReview', {'sessionId': sessionId, 'prevHash': prevHash});
    return refreshReviewResponse;
  }

  @override
  Future<String> createComment({
    required String handle,
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
    required String handle,
    required String sessionId,
    required String commentId,
  }) async {
    _record('deleteComment', {'commentId': commentId});
    if (deleteCommentError != null) throw deleteCommentError!;
  }

  @override
  Future<ApplyResult> applyComments({
    required String handle,
    required String sessionId,
  }) async {
    _record('applyComments', {'sessionId': sessionId});
    if (applyCommentsError != null) throw applyCommentsError!;
    return applyCommentsResponse;
  }

  @override
  Future<bool> toggleFileReviewed({
    required String handle,
    required String sessionId,
    required String displayPath,
  }) async {
    _record('toggleFileReviewed', {'displayPath': displayPath});
    if (toggleFileReviewedError != null) throw toggleFileReviewedError!;
    return toggleFileReviewedResponse;
  }

  @override
  Future<Uint8List> fetchBlob({
    required String handle,
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
    required String attachId,
    required String sessionId,
    required AttachKind kind,
  }) {
    attachTerminalCount++;
    _record('attachTerminal', {
      'handle': handle,
      'attachId': attachId,
      'sessionId': sessionId,
      'kind': kind,
    });
    // Each attach gets a fresh single-subscription controller: the page cancels
    // its old subscription on reconnect, and a new listen needs a clean stream.
    terminalController = StreamController<TerminalEvent>();
    return terminalController.stream;
  }

  @override
  Future<void> terminalSendInput({
    required String attachId,
    required List<int> bytes,
  }) async {
    _record('terminalSendInput', {'attachId': attachId, 'bytes': bytes});
  }

  @override
  Future<void> terminalResize({
    required String attachId,
    required int cols,
    required int rows,
  }) async {
    _record('terminalResize', {'attachId': attachId, 'cols': cols, 'rows': rows});
  }

  @override
  Future<void> terminalDetach({required String attachId}) async {
    _record('terminalDetach', {'attachId': attachId});
  }

  @override
  Stream<BigInt> changeFeed({required String handle}) {
    _record('changeFeed', {'handle': handle});
    return changeController.stream;
  }

  @override
  Stream<ConnectionStateDto> connectionFeed({required String handle}) {
    _record('connectionFeed', {'handle': handle});
    return connectionController.stream;
  }
}
