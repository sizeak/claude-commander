import 'package:claude_commander_client/server_config.dart';
import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/src/rust/api/review.dart';
import 'package:uuid/uuid.dart';

/// Test data builders for the frb mirror/DTO types. They carry required fields
/// with plausible defaults so tests only pass what they assert on.

const testConfig = ServerConfig(
  baseUrl: 'http://127.0.0.1:7878',
  token: 'test-token',
);

SessionInfo sessionInfo({
  String id = '11111111-2222-3333-4444-555555555555',
  String title = 'Test session',
  String branch = 'test-branch',
  SessionStatus status = SessionStatus.running,
  String program = 'bash',
  String projectName = 'my-repo',
  int? prNumber,
  PrState prState = PrState.open,
}) {
  final uuid = UuidValue.fromString(id);
  return SessionInfo(
    id: id,
    sessionId: SessionId(field0: uuid),
    title: title,
    branch: branch,
    status: status,
    program: program,
    projectId: ProjectId(field0: uuid),
    projectName: projectName,
    prNumber: prNumber,
    prUrl: null,
    prState: prState,
    prDraft: false,
    prLabels: const [],
    reviewDecision: null,
    prReviewers: const [],
    createdAt: DateTime.utc(2026, 1, 1),
    unread: false,
    stackParentSessionId: null,
    prBaseBranch: null,
    prMerged: false,
    currentSection: null,
    sectionOverride: null,
    enteredSectionAt: null,
    lastAttachedAt: null,
    worktreePath: '/tmp/test-worktree',
    tmuxSessionName: 'cc-test',
    keepAlive: false,
  );
}

SessionDetail sessionDetail({
  SessionInfo? info,
  AgentState agentState = AgentState.idle,
  String? diffStat = '2 files changed',
  String? paneContent = 'pane snapshot',
}) => SessionDetail(
  info: info ?? sessionInfo(),
  agentState: agentState,
  diffStat: diffStat,
  paneContent: paneContent,
);

ReviewLineDto line(
  ReviewLineOrigin origin,
  String content, {
  int? oldLineno,
  int? newLineno,
}) => ReviewLineDto(
  origin: origin,
  oldLineno: oldLineno,
  newLineno: newLineno,
  content: content,
);

ReviewHunkDto hunk({
  int oldStart = 1,
  int oldLines = 1,
  int newStart = 1,
  int newLines = 2,
  String header = '',
  List<ReviewLineDto>? lines,
}) => ReviewHunkDto(
  oldStart: oldStart,
  oldLines: oldLines,
  newStart: newStart,
  newLines: newLines,
  header: header,
  lines:
      lines ??
      [
        line(
          ReviewLineOrigin.context,
          'context line',
          oldLineno: 1,
          newLineno: 1,
        ),
        line(ReviewLineOrigin.addition, 'added line', newLineno: 2),
      ],
);

ReviewFileDto reviewFile({
  String displayPath = 'src/main.rs',
  ReviewFileStatus status = ReviewFileStatus.modified,
  int added = 1,
  int removed = 0,
  List<ReviewHunkDto>? hunks,
  bool isBinary = false,
  String? binaryMime,
}) => ReviewFileDto(
  displayPath: displayPath,
  oldPath: displayPath,
  newPath: displayPath,
  status: status,
  added: added,
  removed: removed,
  hunks: hunks ?? [hunk()],
  isBinary: isBinary,
  binaryMime: binaryMime,
);

CommentDto comment({
  String id = 'comment-1',
  String file = 'src/main.rs',
  ReviewCommentSide side = ReviewCommentSide.new_,
  int lineStart = 2,
  int lineEnd = 2,
  String snippet = 'added line',
  String text = 'Please fix this',
  ReviewCommentStatus status = ReviewCommentStatus.staged,
}) => CommentDto(
  id: id,
  file: file,
  side: side,
  lineStart: lineStart,
  lineEnd: lineEnd,
  snippet: snippet,
  comment: text,
  status: status,
  createdAt: DateTime.utc(2026, 1, 1),
);

ReviewSnapshotDto reviewSnapshot({
  String base = 'main',
  String contentHash = '42',
  List<ReviewFileDto>? files,
  List<CommentDto>? comments,
  List<String>? reviewed,
}) => ReviewSnapshotDto(
  base: base,
  contentHash: contentHash,
  files: files ?? [reviewFile()],
  comments: comments ?? const [],
  reviewed: reviewed ?? const [],
);
