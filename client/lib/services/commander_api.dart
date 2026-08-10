import 'dart:typed_data';

import 'package:flutter/foundation.dart' show visibleForTesting;

import '../src/rust/api/diff.dart' as diff;
import '../src/rust/api/diff.dart'
    show DiffExpansion, DiffLayoutDto, DiffLayoutMode;
import '../src/rust/api/mirrors.dart';
import '../src/rust/api/registry.dart' as registry;
import '../src/rust/api/review.dart' as review;
// The DTO types unprefixed, so the abstract signatures read cleanly; the
// prefixed alias above carries the forwarded functions.
import '../src/rust/api/review.dart'
    show ApplyResult, ReviewFileDto, ReviewSnapshotDto;
import '../src/rust/api/simple.dart' as simple;
import '../src/rust/api/simple.dart' show ScanResultDto;
import '../src/rust/api/terminal.dart' as terminal;

export '../src/rust/api/terminal.dart' show TerminalEvent, TerminalEventKind;

/// The single seam between the pages and the Rust bridge. Its methods mirror the
/// generated frb functions 1:1.
///
/// Every route/terminal/feed call is keyed by an opaque server `handle` obtained
/// from [connectServer] (the seam for a future multi-server client). Only the
/// two `health*` probes still take a raw `baseUrl`/`token`, because the connect
/// screen calls them *before* a handle exists.
///
/// [RustCommanderApi] is a thin forwarder; tests substitute a hand-rolled fake
/// without a live bridge.
abstract class CommanderApi {
  /// Connect to a server and return its opaque handle. Validates the URL; a
  /// non-empty token is sent as the bearer.
  Future<String> connectServer({required String baseUrl, String? token});

  /// Disconnect a server (drops its poller). A no-op for an unknown handle.
  Future<void> disconnectServer({required String handle});

  Future<bool> health({required String baseUrl});

  Future<bool> healthTmux({required String baseUrl, required String token});

  Future<WorkspaceSnapshotDto> workspaceSnapshot({required String handle});

  Future<AgentStatesSnapshotDto> agentStates({
    required String handle,
    required bool fresh,
  });

  Future<List<SessionInfo>> listSessions({
    required String handle,
    required bool includeStopped,
  });

  Future<SessionDetail?> getSessionDetail({
    required String handle,
    required String query,
    int? lines,
  });

  Future<PreviewDataDto> sessionPreview({
    required String handle,
    required String id,
    int? lines,
  });

  Future<PreviewDataDto> projectPreview({
    required String handle,
    required String id,
  });

  Future<String> branchDiff({required String handle, required String id});

  Future<List<BranchInfo>> listBranches({
    required String handle,
    required String projectId,
    required bool fetch,
  });

  Future<CreateOptions> createOptions({required String handle});

  Future<void> setPrograms({
    required String handle,
    required List<ProgramInfo> programs,
  });

  Future<List<SessionId>> pendingCommentSessions({required String handle});

  Future<String> createSession({
    required String handle,
    required String projectPath,
    required String title,
    String? program,
    String? initialPrompt,
    String? effort,
    String? mode,
    String? baseBranch,
  });

  Future<void> killSession({required String handle, required String id});

  Future<void> restartSession({required String handle, required String id});

  Future<void> deleteSession({required String handle, required String id});

  Future<void> renameSession({
    required String handle,
    required String id,
    required String title,
  });

  Future<void> setSection({
    required String handle,
    required String id,
    String? section,
  });

  Future<void> markRead({required String handle, required String id});

  Future<void> markUnread({required String handle, required List<String> ids});

  Future<bool> toggleKeepAlive({required String handle, required String id});

  /// Upload an image to a session's agent pane. The server writes it to a temp
  /// file and types the path into the pane without pressing Enter, so it shows
  /// up in the terminal view through the normal attach stream — no success
  /// feedback needed. Throws if the bytes aren't an allow-listed image or exceed
  /// [imageMaxBytes]; the message is safe to show the user.
  Future<void> pasteImage({
    required String handle,
    required String id,
    required Uint8List bytes,
  });

  /// The pasted-image size cap in bytes, from the shared wire contract. Lets the
  /// UI refuse an oversized pick from its file length instead of reading the
  /// whole thing into memory first.
  Future<int> imageMaxBytes();

  /// How long a silent client can be away before the server has certainly torn
  /// its terminal attach down, from the shared wire contract. A backgrounded app
  /// can't answer the server's heartbeat pings, so the terminal uses this on
  /// resume to tell "definitely dead, re-attach" from "might still be live,
  /// leave it alone".
  Future<Duration> attachDeadAfter();

  Future<String> addProject({required String handle, required String path});

  Future<void> removeProject({required String handle, required String id});

  Future<ScanResultDto> scanDirectory({
    required String handle,
    required String path,
  });

  /// Every repo the server-side `gh` user can clone. The list is the *server's*
  /// to produce, so a phone with no `gh` still gets a picker; a server without
  /// `gh` throws, which the picker words as an inline banner rather than a dead
  /// screen.
  Future<List<GithubRepo>> githubRepos({required String handle});

  /// Start a clone. The returned job's status is **not** terminal — every
  /// outcome arrives through [cloneJob], so the caller polls from here.
  Future<CloneJobDto> startClone({
    required String handle,
    required CloneRequestDto request,
  });

  /// One poll of a clone job. **Null is a normal answer**: the server prunes
  /// finished jobs, so "gone" is not a failure.
  Future<CloneJobDto?> cloneJob({
    required String handle,
    required CloneJobId id,
  });

  /// Reduce a clone source to a stable `host/owner/name` identity, or null when
  /// it has none (a local path, a `file://` URL, or a repo with no origin).
  ///
  /// Pure string work with no server involved, hence no handle. It sits on the
  /// seam anyway so widget tests can reach it without a live bridge — and so the
  /// picker's badge compares canonical forms produced by the *one* definition in
  /// `claude_commander_protocol::github`, never a second Dart implementation of
  /// the rule.
  ///
  /// **Two nulls are not a match.** Callers must treat a null on either side as
  /// "no identity", not as an identity that can be equal to another null.
  Future<String?> canonicalRepoSlug({required String url});

  Future<OperationStatusDto> cascadeMerge({
    required String handle,
    required String id,
  });

  Future<OperationStatusDto> pushStack({
    required String handle,
    required String id,
  });

  Future<OperationStatusDto> cascadeResume({required String handle});

  Future<void> cascadeAbandon({required String handle});

  Future<void> requestPrRefresh({required String handle});

  Future<ReviewSnapshotDto> openReview({
    required String handle,
    required String sessionId,
  });

  Future<ReviewSnapshotDto?> refreshReview({
    required String handle,
    required String sessionId,
    required String prevHash,
  });

  Future<String> createComment({
    required String handle,
    required String sessionId,
    required String file,
    required String side,
    required int lineStart,
    required int lineEnd,
    required String snippet,
    required String comment,
  });

  Future<void> deleteComment({
    required String handle,
    required String sessionId,
    required String commentId,
  });

  Future<ApplyResult> applyComments({
    required String handle,
    required String sessionId,
  });

  Future<bool> toggleFileReviewed({
    required String handle,
    required String sessionId,
    required String displayPath,
  });

  Future<Uint8List> fetchBlob({
    required String handle,
    required String sessionId,
    required String side,
    required String path,
  });

  /// Lay one file of a review diff out into rows of styled runs.
  ///
  /// Pure computation in the cdylib — no server involved, hence no `handle` —
  /// but it goes through this seam like everything else so widget tests can
  /// substitute a layout without loading the native library.
  Future<DiffLayoutDto> diffRows({
    required String? raw,
    required ReviewFileDto file,
    required DiffLayoutMode mode,
    String? fileText,
    List<DiffExpansion> expansions,
  });

  /// Open a live terminal attach. [attachId] is a caller-supplied per-attach id
  /// (a fresh UUID) that keys the control channel — so several attaches can be
  /// live against one server (e.g. a persistent desktop terminal pane). The
  /// server is resolved via [handle]; the control calls below key by [attachId].
  Stream<terminal.TerminalEvent> attachTerminal({
    required String handle,
    required String attachId,
    required String sessionId,
    required AttachKind kind,
  });

  Future<void> terminalSendInput({
    required String attachId,
    required List<int> bytes,
  });

  Future<void> terminalResize({
    required String attachId,
    required int cols,
    required int rows,
  });

  Future<void> terminalDetach({required String attachId});

  /// The change-feed generation counter: bumped whenever server state moves.
  Stream<BigInt> changeFeed({required String handle});

  /// The server's connection health, for the status header.
  Stream<ConnectionStateDto> connectionFeed({required String handle});
}

/// Restores FIFO delivery for one attach's terminal control calls.
///
/// `flutter_rust_bridge` runs a non-`sync` call on a worker thread pool, so two
/// calls issued back-to-back from Dart can execute — and so reach the attach's
/// channel — in either order. That transposes fast keystrokes, and a burst of
/// resizes can leave the remote PTY at a stale size while the local terminal has
/// moved on. Chaining each call onto the previous one's completion means only one
/// is ever in flight per attach, so order is the order they were issued in.
class AttachControlLane {
  final Map<String, Future<void>> _tails = {};

  /// Run [op] after every call already queued for [attachId].
  Future<void> run(String attachId, Future<void> Function() op) {
    final previous = _tails[attachId] ?? Future<void>.value();
    final result = previous.then((_) => op());
    // The tail swallows failures: one failed call must not wedge the lane, and
    // callers that fire-and-forget shouldn't strand an error on it either.
    late final Future<void> tail;
    tail = result.catchError((Object _) {}).whenComplete(() {
      // Self-cleaning: drop the queue once it drains, but only if nothing has
      // been chained on since. Attach ids are per-attach UUIDs and a reconnect
      // simply abandons the old one, so without this the map would grow by an
      // entry per reconnect for the lifetime of the app.
      if (_tails[attachId] == tail) _tails.remove(attachId);
    });
    _tails[attachId] = tail;
    return result;
  }

  /// How many attaches still have queued work. Test-only seam for the
  /// self-cleaning behaviour above.
  @visibleForTesting
  int get pendingAttaches => _tails.length;
}

/// The production [CommanderApi]: every method forwards straight to the
/// generated `lib/src/rust/api/*.dart` bridge functions.
class RustCommanderApi implements CommanderApi {
  RustCommanderApi();

  /// Serialises `terminalSendInput`/`terminalResize`/`terminalDetach`, which are
  /// order-sensitive; see [AttachControlLane].
  final AttachControlLane _control = AttachControlLane();

  @override
  Future<String> connectServer({required String baseUrl, String? token}) =>
      registry.connectServer(baseUrl: baseUrl, token: token);

  @override
  Future<void> disconnectServer({required String handle}) =>
      registry.disconnectServer(handle: handle);

  @override
  Future<bool> health({required String baseUrl}) =>
      simple.health(baseUrl: baseUrl);

  @override
  Future<bool> healthTmux({required String baseUrl, required String token}) =>
      simple.healthTmux(baseUrl: baseUrl, token: token);

  @override
  Future<WorkspaceSnapshotDto> workspaceSnapshot({required String handle}) =>
      simple.workspaceSnapshot(handle: handle);

  @override
  Future<AgentStatesSnapshotDto> agentStates({
    required String handle,
    required bool fresh,
  }) => simple.agentStates(handle: handle, fresh: fresh);

  @override
  Future<List<SessionInfo>> listSessions({
    required String handle,
    required bool includeStopped,
  }) => simple.listSessions(handle: handle, includeStopped: includeStopped);

  @override
  Future<SessionDetail?> getSessionDetail({
    required String handle,
    required String query,
    int? lines,
  }) => simple.getSessionDetail(handle: handle, query: query, lines: lines);

  @override
  Future<PreviewDataDto> sessionPreview({
    required String handle,
    required String id,
    int? lines,
  }) => simple.sessionPreview(handle: handle, id: id, lines: lines);

  @override
  Future<PreviewDataDto> projectPreview({
    required String handle,
    required String id,
  }) => simple.projectPreview(handle: handle, id: id);

  @override
  Future<String> branchDiff({required String handle, required String id}) =>
      simple.branchDiff(handle: handle, id: id);

  @override
  Future<List<BranchInfo>> listBranches({
    required String handle,
    required String projectId,
    required bool fetch,
  }) => simple.listBranches(handle: handle, projectId: projectId, fetch: fetch);

  @override
  Future<CreateOptions> createOptions({required String handle}) =>
      simple.createOptions(handle: handle);

  @override
  Future<void> setPrograms({
    required String handle,
    required List<ProgramInfo> programs,
  }) => simple.setPrograms(handle: handle, programs: programs);

  @override
  Future<List<SessionId>> pendingCommentSessions({required String handle}) =>
      simple.pendingCommentSessions(handle: handle);

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
  }) => simple.createSession(
    handle: handle,
    projectPath: projectPath,
    title: title,
    program: program,
    initialPrompt: initialPrompt,
    effort: effort,
    mode: mode,
    baseBranch: baseBranch,
  );

  @override
  Future<void> killSession({required String handle, required String id}) =>
      simple.killSession(handle: handle, id: id);

  @override
  Future<void> restartSession({required String handle, required String id}) =>
      simple.restartSession(handle: handle, id: id);

  @override
  Future<void> deleteSession({required String handle, required String id}) =>
      simple.deleteSession(handle: handle, id: id);

  @override
  Future<void> renameSession({
    required String handle,
    required String id,
    required String title,
  }) => simple.renameSession(handle: handle, id: id, title: title);

  @override
  Future<void> setSection({
    required String handle,
    required String id,
    String? section,
  }) => simple.setSection(handle: handle, id: id, section: section);

  @override
  Future<void> markRead({required String handle, required String id}) =>
      simple.markRead(handle: handle, id: id);

  @override
  Future<void> markUnread({
    required String handle,
    required List<String> ids,
  }) => simple.markUnread(handle: handle, ids: ids);

  @override
  Future<bool> toggleKeepAlive({required String handle, required String id}) =>
      simple.toggleKeepAlive(handle: handle, id: id);

  @override
  Future<void> pasteImage({
    required String handle,
    required String id,
    required Uint8List bytes,
  }) => simple.pasteImage(handle: handle, id: id, bytes: bytes);

  @override
  Future<int> imageMaxBytes() => simple.imageMaxBytes();

  /// The bridge carries plain milliseconds (a `u32`, so Dart sees an `int` rather
  /// than a `BigInt`); rewrap it as a [Duration] here so no call site has to
  /// remember the unit.
  @override
  Future<Duration> attachDeadAfter() async =>
      Duration(milliseconds: await simple.attachDeadAfterMillis());

  @override
  Future<String> addProject({required String handle, required String path}) =>
      simple.addProject(handle: handle, path: path);

  @override
  Future<void> removeProject({required String handle, required String id}) =>
      simple.removeProject(handle: handle, id: id);

  @override
  Future<ScanResultDto> scanDirectory({
    required String handle,
    required String path,
  }) => simple.scanDirectory(handle: handle, path: path);

  @override
  Future<List<GithubRepo>> githubRepos({required String handle}) =>
      simple.githubRepos(handle: handle);

  @override
  Future<CloneJobDto> startClone({
    required String handle,
    required CloneRequestDto request,
  }) => simple.startClone(handle: handle, request: request);

  @override
  Future<CloneJobDto?> cloneJob({
    required String handle,
    required CloneJobId id,
  }) => simple.cloneJob(handle: handle, id: id);

  @override
  Future<String?> canonicalRepoSlug({required String url}) =>
      simple.canonicalRepoSlug(url: url);

  @override
  Future<OperationStatusDto> cascadeMerge({
    required String handle,
    required String id,
  }) => simple.cascadeMerge(handle: handle, id: id);

  @override
  Future<OperationStatusDto> pushStack({
    required String handle,
    required String id,
  }) => simple.pushStack(handle: handle, id: id);

  @override
  Future<OperationStatusDto> cascadeResume({required String handle}) =>
      simple.cascadeResume(handle: handle);

  @override
  Future<void> cascadeAbandon({required String handle}) =>
      simple.cascadeAbandon(handle: handle);

  @override
  Future<void> requestPrRefresh({required String handle}) =>
      simple.requestPrRefresh(handle: handle);

  @override
  Future<ReviewSnapshotDto> openReview({
    required String handle,
    required String sessionId,
  }) => review.openReview(handle: handle, sessionId: sessionId);

  @override
  Future<ReviewSnapshotDto?> refreshReview({
    required String handle,
    required String sessionId,
    required String prevHash,
  }) => review.refreshReview(
    handle: handle,
    sessionId: sessionId,
    prevHash: prevHash,
  );

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
  }) => review.createComment(
    handle: handle,
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
    required String handle,
    required String sessionId,
    required String commentId,
  }) => review.deleteComment(
    handle: handle,
    sessionId: sessionId,
    commentId: commentId,
  );

  @override
  Future<ApplyResult> applyComments({
    required String handle,
    required String sessionId,
  }) => review.applyComments(handle: handle, sessionId: sessionId);

  @override
  Future<bool> toggleFileReviewed({
    required String handle,
    required String sessionId,
    required String displayPath,
  }) => review.toggleFileReviewed(
    handle: handle,
    sessionId: sessionId,
    displayPath: displayPath,
  );

  @override
  Future<Uint8List> fetchBlob({
    required String handle,
    required String sessionId,
    required String side,
    required String path,
  }) => review.fetchBlob(
    handle: handle,
    sessionId: sessionId,
    side: side,
    path: path,
  );

  @override
  Future<DiffLayoutDto> diffRows({
    required String? raw,
    required ReviewFileDto file,
    required DiffLayoutMode mode,
    String? fileText,
    List<DiffExpansion> expansions = const [],
  }) => diff.diffRows(
    raw: raw,
    fallback: file,
    mode: mode,
    fileText: fileText,
    expansions: expansions,
    // Four columns, matching the TUI's default tab width.
    tabWidth: 4,
  );

  @override
  Stream<terminal.TerminalEvent> attachTerminal({
    required String handle,
    required String attachId,
    required String sessionId,
    required AttachKind kind,
  }) => terminal.attachTerminal(
    handle: handle,
    attachId: attachId,
    sessionId: sessionId,
    kind: kind,
  );

  @override
  Future<void> terminalSendInput({
    required String attachId,
    required List<int> bytes,
  }) => _control.run(
    attachId,
    () => terminal.terminalSendInput(attachId: attachId, bytes: bytes),
  );

  @override
  Future<void> terminalResize({
    required String attachId,
    required int cols,
    required int rows,
  }) => _control.run(
    attachId,
    () => terminal.terminalResize(attachId: attachId, cols: cols, rows: rows),
  );

  @override
  Future<void> terminalDetach({required String attachId}) => _control.run(
    // Still queued behind any pending input, so a detach can't overtake the
    // keystrokes it was meant to follow.
    attachId,
    () => terminal.terminalDetach(attachId: attachId),
  );

  @override
  Stream<BigInt> changeFeed({required String handle}) =>
      terminal.changeFeed(handle: handle);

  @override
  Stream<ConnectionStateDto> connectionFeed({required String handle}) =>
      terminal.connectionFeed(handle: handle);
}
