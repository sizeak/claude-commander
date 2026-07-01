import 'dart:typed_data';

import '../src/rust/api/mirrors.dart';
import '../src/rust/api/review.dart' as review;
// The DTO types unprefixed, so the abstract signatures read cleanly; the
// prefixed alias above carries the forwarded functions.
import '../src/rust/api/review.dart'
    show ApplyResult, CommentDto, ReviewSnapshotDto;
import '../src/rust/api/simple.dart' as simple;
import '../src/rust/api/terminal.dart' as terminal;

export '../src/rust/api/terminal.dart' show TerminalEvent, TerminalEventKind;

/// The single seam between the pages and the Rust bridge. Its methods mirror the
/// generated frb functions 1:1 (explicit `baseUrl`/`token` per call, matching
/// how pages already hold `widget.config.*`), so [RustCommanderApi] is a thin
/// forwarder and tests can substitute a hand-rolled fake without a live bridge.
abstract class CommanderApi {
  Future<bool> health({required String baseUrl});

  Future<bool> healthTmux({required String baseUrl, required String token});

  Future<List<SessionInfo>> listSessions({
    required String baseUrl,
    required String token,
    required bool includeStopped,
  });

  Future<SessionDetail?> getSessionDetail({
    required String baseUrl,
    required String token,
    required String query,
    int? lines,
  });

  Future<String?> getPane({
    required String baseUrl,
    required String token,
    required String query,
    int? lines,
  });

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
  });

  Future<void> killSession({
    required String baseUrl,
    required String token,
    required String id,
  });

  Future<void> restartSession({
    required String baseUrl,
    required String token,
    required String id,
  });

  Future<void> deleteSession({
    required String baseUrl,
    required String token,
    required String id,
  });

  Future<ReviewSnapshotDto> openReview({
    required String baseUrl,
    required String token,
    required String sessionId,
  });

  Future<ReviewSnapshotDto?> refreshReview({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String prevHash,
  });

  Future<List<CommentDto>> listComments({
    required String baseUrl,
    required String token,
    required String sessionId,
  });

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
  });

  Future<void> deleteComment({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String commentId,
  });

  Future<ApplyResult> applyComments({
    required String baseUrl,
    required String token,
    required String sessionId,
  });

  Future<bool> toggleFileReviewed({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String displayPath,
  });

  Future<Uint8List> fetchBlob({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String side,
    required String path,
  });

  Stream<terminal.TerminalEvent> attachTerminal({
    required String handle,
    required String baseUrl,
    required String token,
    required String sessionId,
  });

  Future<void> terminalSendInput({
    required String handle,
    required List<int> bytes,
  });

  Future<void> terminalResize({
    required String handle,
    required int cols,
    required int rows,
  });

  Future<void> terminalDetach({required String handle});
}

/// The production [CommanderApi]: every method forwards straight to the
/// generated `lib/src/rust/api/*.dart` bridge functions.
class RustCommanderApi implements CommanderApi {
  const RustCommanderApi();

  @override
  Future<bool> health({required String baseUrl}) =>
      simple.health(baseUrl: baseUrl);

  @override
  Future<bool> healthTmux({required String baseUrl, required String token}) =>
      simple.healthTmux(baseUrl: baseUrl, token: token);

  @override
  Future<List<SessionInfo>> listSessions({
    required String baseUrl,
    required String token,
    required bool includeStopped,
  }) => simple.listSessions(
    baseUrl: baseUrl,
    token: token,
    includeStopped: includeStopped,
  );

  @override
  Future<SessionDetail?> getSessionDetail({
    required String baseUrl,
    required String token,
    required String query,
    int? lines,
  }) => simple.getSessionDetail(
    baseUrl: baseUrl,
    token: token,
    query: query,
    lines: lines,
  );

  @override
  Future<String?> getPane({
    required String baseUrl,
    required String token,
    required String query,
    int? lines,
  }) => simple.getPane(
    baseUrl: baseUrl,
    token: token,
    query: query,
    lines: lines,
  );

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
  }) => simple.createSession(
    baseUrl: baseUrl,
    token: token,
    projectPath: projectPath,
    title: title,
    program: program,
    initialPrompt: initialPrompt,
    effort: effort,
    mode: mode,
    baseBranch: baseBranch,
  );

  @override
  Future<void> killSession({
    required String baseUrl,
    required String token,
    required String id,
  }) => simple.killSession(baseUrl: baseUrl, token: token, id: id);

  @override
  Future<void> restartSession({
    required String baseUrl,
    required String token,
    required String id,
  }) => simple.restartSession(baseUrl: baseUrl, token: token, id: id);

  @override
  Future<void> deleteSession({
    required String baseUrl,
    required String token,
    required String id,
  }) => simple.deleteSession(baseUrl: baseUrl, token: token, id: id);

  @override
  Future<ReviewSnapshotDto> openReview({
    required String baseUrl,
    required String token,
    required String sessionId,
  }) => review.openReview(baseUrl: baseUrl, token: token, sessionId: sessionId);

  @override
  Future<ReviewSnapshotDto?> refreshReview({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String prevHash,
  }) => review.refreshReview(
    baseUrl: baseUrl,
    token: token,
    sessionId: sessionId,
    prevHash: prevHash,
  );

  @override
  Future<List<CommentDto>> listComments({
    required String baseUrl,
    required String token,
    required String sessionId,
  }) =>
      review.listComments(baseUrl: baseUrl, token: token, sessionId: sessionId);

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
  }) => review.createComment(
    baseUrl: baseUrl,
    token: token,
    sessionId: sessionId,
    file: file,
    side: side,
    lineStart: lineStart,
    lineEnd: lineEnd,
    snippet: snippet,
    comment: comment,
  );

  @override
  Future<void> deleteComment({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String commentId,
  }) => review.deleteComment(
    baseUrl: baseUrl,
    token: token,
    sessionId: sessionId,
    commentId: commentId,
  );

  @override
  Future<ApplyResult> applyComments({
    required String baseUrl,
    required String token,
    required String sessionId,
  }) => review.applyComments(
    baseUrl: baseUrl,
    token: token,
    sessionId: sessionId,
  );

  @override
  Future<bool> toggleFileReviewed({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String displayPath,
  }) => review.toggleFileReviewed(
    baseUrl: baseUrl,
    token: token,
    sessionId: sessionId,
    displayPath: displayPath,
  );

  @override
  Future<Uint8List> fetchBlob({
    required String baseUrl,
    required String token,
    required String sessionId,
    required String side,
    required String path,
  }) => review.fetchBlob(
    baseUrl: baseUrl,
    token: token,
    sessionId: sessionId,
    side: side,
    path: path,
  );

  @override
  Stream<terminal.TerminalEvent> attachTerminal({
    required String handle,
    required String baseUrl,
    required String token,
    required String sessionId,
  }) => terminal.attachTerminal(
    handle: handle,
    baseUrl: baseUrl,
    token: token,
    sessionId: sessionId,
  );

  @override
  Future<void> terminalSendInput({
    required String handle,
    required List<int> bytes,
  }) => terminal.terminalSendInput(handle: handle, bytes: bytes);

  @override
  Future<void> terminalResize({
    required String handle,
    required int cols,
    required int rows,
  }) => terminal.terminalResize(handle: handle, cols: cols, rows: rows);

  @override
  Future<void> terminalDetach({required String handle}) =>
      terminal.terminalDetach(handle: handle);
}
