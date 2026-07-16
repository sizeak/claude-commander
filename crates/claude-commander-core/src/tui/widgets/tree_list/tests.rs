use super::*;
use crate::git::PrState;
use crate::session::{ProjectId, SessionId};
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::{Terminal, backend::TestBackend, widgets::ListState};
use std::path::PathBuf;

fn make_project(name: &str, count: usize) -> SessionListItem {
    SessionListItem::Project {
        id: ProjectId::new(),
        name: name.to_string(),
        repo_path: PathBuf::from("/tmp/test"),
        main_branch: "main".to_string(),
        worktree_count: count,
        nested: false,
    }
}

fn make_worktree(title: &str) -> SessionListItem {
    make_worktree_with_stack(title, false)
}

fn make_worktree_with_stack(title: &str, stacked_child: bool) -> SessionListItem {
    SessionListItem::Worktree {
        id: SessionId::new(),
        project_id: ProjectId::new(),
        title: title.to_string(),
        branch: "feat".to_string(),
        status: SessionStatus::Running,
        program: "claude".to_string(),
        pr_number: None,
        pr_url: None,
        pr_merged: false,
        pr_state: None,
        pr_draft: false,
        pr_labels: Vec::new(),
        worktree_path: PathBuf::from("/tmp/test"),
        created_at: chrono::Utc::now(),
        agent_state: None,
        unread: false,
        keep_alive: false,
        lfs_pulling: false,
        stacked_child,
    }
}

/// Render a TreeList to a buffer and return lines as strings
fn render_tree(items: &[SessionListItem], width: u16, height: u16) -> Vec<String> {
    render_tree_with(items, width, height, |t| t)
}

/// Like `render_tree`, but lets the caller configure the TreeList (e.g. to
/// toggle `show_session_program`).
fn render_tree_with<F>(
    items: &[SessionListItem],
    width: u16,
    height: u16,
    configure: F,
) -> Vec<String>
where
    F: for<'a> FnOnce(TreeList<'a>) -> TreeList<'a>,
{
    let buf = draw_tree_buffer(items, width, height, configure);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

/// Shared draw step for the rendering helpers below: builds a `TestBackend`
/// of the requested size, renders the (optionally configured) `TreeList`, and
/// returns the resulting buffer for the caller to extract symbols or styles.
fn draw_tree_buffer<F>(items: &[SessionListItem], width: u16, height: u16, configure: F) -> Buffer
where
    F: for<'a> FnOnce(TreeList<'a>) -> TreeList<'a>,
{
    let theme = Theme::basic();
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            let tree = configure(TreeList::new(items, &theme));
            frame.render_stateful_widget(tree, frame.area(), &mut ListState::default());
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

/// A `Worktree` row with a caller-chosen id, so a recents shortcut can point
/// at the exact same session.
fn make_worktree_with_id(title: &str, id: SessionId) -> SessionListItem {
    let mut w = make_worktree(title);
    if let SessionListItem::Worktree { id: wid, .. } = &mut w {
        *wid = id;
    }
    w
}

fn make_recent(title: &str, id: SessionId) -> SessionListItem {
    SessionListItem::RecentSession {
        session: crate::backend::SessionRef::local(id),
        project_id: ProjectId::new(),
        title: title.to_string(),
        status: SessionStatus::Running,
        agent_state: None,
        unread: false,
        branch: "feat".to_string(),
        program: "claude".to_string(),
        keep_alive: false,
        lfs_pulling: false,
        pr_number: None,
        pr_url: None,
        pr_merged: false,
        pr_state: None,
        pr_draft: false,
        pr_labels: Vec::new(),
    }
}

/// A recents shortcut carrying an open PR, so the badge-rendering path is
/// exercised.
fn make_recent_with_pr(title: &str, id: SessionId, pr_number: u32) -> SessionListItem {
    let mut r = make_recent(title, id);
    if let SessionListItem::RecentSession {
        pr_number: n,
        pr_url,
        pr_state,
        ..
    } = &mut r
    {
        *n = Some(pr_number);
        *pr_url = Some(format!("https://example.com/pr/{pr_number}"));
        *pr_state = Some(PrState::Open);
    }
    r
}

#[test]
fn recent_row_mirrors_the_real_rows_number() {
    // The second project's session is #2 in its real position. A recents
    // shortcut to that same session at the top must show "2" too, even though
    // it renders before any project row.
    let target = SessionId::new();
    let items = vec![
        SessionListItem::RecentsHeader,
        make_recent("session-b", target),
        SessionListItem::Spacer,
        make_project("proj-a", 1),
        make_worktree("session-a"), // #1
        make_project("proj-b", 1),
        make_worktree_with_id("session-b", target), // #2
    ];
    let lines = render_tree(&items, 40, 8);
    assert!(lines[0].contains("Recent"), "header line: '{}'", lines[0]);
    // The recent shortcut row (line 1) carries the mirrored number 2.
    assert!(
        lines[1].trim_start().starts_with("2 "),
        "recent row should mirror number 2: '{}'",
        lines[1]
    );
    assert!(
        lines[1].contains("session-b"),
        "recent title: '{}'",
        lines[1]
    );
    // The real row below still numbers normally.
    assert!(
        lines[6].trim_start().starts_with("2 "),
        "real row #2: '{}'",
        lines[6]
    );
}

#[test]
fn pinned_recents_slice_mirrors_numbers_from_full_list_map() {
    // The pinned recents panel renders ONLY its own slice, so it can't derive
    // worktree numbers itself. The caller computes the map over the full list
    // and passes it in; the recent row must then show the real row's number.
    let target = SessionId::new();
    let full = vec![
        SessionListItem::RecentsHeader,
        make_recent("session-b", target),
        SessionListItem::Spacer,
        make_project("proj-a", 1),
        make_worktree("session-a"), // #1
        make_project("proj-b", 1),
        make_worktree_with_id("session-b", target), // #2
    ];
    let theme = Theme::basic();
    let info = super::worktree_display_info(&full, &theme);
    assert_eq!(info.get(&target).map(|(n, _)| *n), Some(2));

    // Render only the pinned slice (header + recent + divider) with the map.
    let slice = &full[..3];
    let lines = render_tree_with(slice, 40, 3, |t| t.recent_display_info(info));
    assert!(lines[0].contains("Recent"), "header: '{}'", lines[0]);
    assert!(
        lines[1].trim_start().starts_with("2 "),
        "recent row mirrors #2 from the full-list map: '{}'",
        lines[1]
    );
}

#[test]
fn recent_row_shows_pr_badge() {
    // A recents shortcut to a session with an open PR must show the same
    // "PR #<n>" badge the real Worktree row shows below.
    let target = SessionId::new();
    let items = vec![
        SessionListItem::RecentsHeader,
        make_recent_with_pr("session-b", target, 42),
        SessionListItem::Spacer,
        make_project("proj-a", 1),
        make_worktree_with_id("session-b", target),
    ];
    let lines = render_tree(&items, 40, 6);
    // The badge text is wrapped per-character in OSC 8 hyperlink escapes (the
    // same treatment real Worktree rows get), so strip those before matching.
    assert!(
        strip_osc8(&lines[1]).contains("PR #42"),
        "recent row should show the PR badge: '{}'",
        lines[1]
    );
}

#[test]
fn recent_row_renders_identically_to_its_real_row() {
    // A recents shortcut must be indistinguishable from the real row it points
    // at — same number, glyph, title, keep-alive anchor, branch, and PR badge.
    let target = SessionId::new();
    let branch = "my-feature-branch";

    let mut recent = make_recent_with_pr("session-x", target, 7);
    if let SessionListItem::RecentSession {
        branch: b,
        keep_alive,
        ..
    } = &mut recent
    {
        *b = branch.to_string();
        *keep_alive = true;
    }

    let mut real = make_worktree_with_id("session-x", target);
    if let SessionListItem::Worktree {
        branch: b,
        keep_alive,
        pr_number,
        pr_url,
        pr_state,
        ..
    } = &mut real
    {
        *b = branch.to_string();
        *keep_alive = true;
        *pr_number = Some(7);
        *pr_url = Some("https://example.com/pr/7".to_string());
        *pr_state = Some(PrState::Open);
    }

    let items = vec![
        SessionListItem::RecentsHeader,
        recent,
        SessionListItem::Spacer,
        make_project("proj", 1),
        real, // #1
    ];
    let lines = render_tree(&items, 60, 6);
    assert_eq!(
        strip_osc8(&lines[1]),
        strip_osc8(&lines[4]),
        "recent row must render identically to its real row\nrecent: '{}'\nreal:   '{}'",
        lines[1],
        lines[4],
    );
}

/// Remove OSC 8 hyperlink escape sequences (`ESC ] 8 ; ; <url> BEL`) so a
/// rendered line can be matched against its plain badge text.
fn strip_osc8(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1B' {
            // Skip up to and including the terminating BEL.
            for c in chars.by_ref() {
                if c == '\x07' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn make_worktree_with_program(title: &str, program: &str) -> SessionListItem {
    let mut w = make_worktree(title);
    if let SessionListItem::Worktree { program: p, .. } = &mut w {
        *p = program.to_string();
    }
    w
}

#[test]
fn server_header_renders_version_mismatch_annotation() {
    use crate::backend::{BackendId, ConnectionState, VersionMismatch};

    let base = SessionListItem::ServerHeader {
        backend: BackendId(1),
        name: "buildbox".to_string(),
        connection: ConnectionState::Connected,
        version_warning: None,
    };
    // No warning → just the name, no ⚠.
    let plain = render_tree(std::slice::from_ref(&base), 60, 2).join("\n");
    assert!(plain.contains("buildbox"), "{plain}");
    assert!(!plain.contains('⚠'), "unexpected warning glyph:\n{plain}");

    // With a mismatch → the ⚠ annotation with both full versions.
    let warned = SessionListItem::ServerHeader {
        backend: BackendId(1),
        name: "buildbox".to_string(),
        connection: ConnectionState::Connected,
        version_warning: Some(VersionMismatch {
            server: "0.24.0".to_string(),
            client: "0.25.0".to_string(),
        }),
    };
    let out = render_tree(&[warned], 60, 2).join("\n");
    assert!(
        out.contains("⚠ v0.24.0 (client v0.25.0)"),
        "expected version-mismatch annotation:\n{out}"
    );
}

#[test]
fn server_header_shows_annotation_alongside_degraded_reason() {
    use crate::backend::{BackendId, ConnectionState, VersionMismatch};

    // A stale server can also be Degraded: both the ⚠ annotation and the
    // degraded reason must show (the annotation is independent of connection).
    let item = SessionListItem::ServerHeader {
        backend: BackendId(1),
        name: "buildbox".to_string(),
        connection: ConnectionState::Degraded {
            reason: "offline".to_string(),
        },
        version_warning: Some(VersionMismatch {
            server: "0.24.0".to_string(),
            client: "0.25.0".to_string(),
        }),
    };
    let out = render_tree(&[item], 70, 2).join("\n");
    assert!(
        out.contains("⚠ v0.24.0 (client v0.25.0)"),
        "annotation missing:\n{out}"
    );
    assert!(out.contains("(offline)"), "degraded reason missing:\n{out}");
}

#[test]
fn worktree_shows_keep_alive_marker() {
    let mut wt = make_worktree("Feature");
    let items_off = vec![make_project("proj", 1), make_worktree("Feature")];

    // No anchor marker when the session is not kept alive.
    let plain = render_tree(&items_off, 40, 4).join("\n");
    assert!(
        !plain.contains(KEEP_ALIVE_MARKER),
        "unexpected keep-alive marker:\n{plain}"
    );

    // The anchor appears once the session is kept alive.
    if let SessionListItem::Worktree { keep_alive, .. } = &mut wt {
        *keep_alive = true;
    }
    let items_on = vec![make_project("proj", 1), wt];
    let marked = render_tree(&items_on, 40, 4).join("\n");
    assert!(
        marked.contains(KEEP_ALIVE_MARKER),
        "expected keep-alive marker:\n{marked}"
    );
}

#[test]
fn worktree_shows_pending_comment_marker() {
    let wt = make_worktree("Feature");
    let id = match &wt {
        SessionListItem::Worktree { id, .. } => *id,
        _ => unreachable!(),
    };
    let items = vec![make_project("proj", 1), wt];

    // No marker when the session has no pending comments.
    let plain = render_tree(&items, 40, 4).join("\n");
    assert!(
        !plain.contains(COMMENT_MARKER),
        "unexpected marker:\n{plain}"
    );

    // The `*` marker appears once the session is flagged.
    let flagged: HashSet<SessionId> = [id].into_iter().collect();
    let marked =
        render_tree_with(&items, 40, 4, |t| t.comment_sessions(flagged.clone())).join("\n");
    assert!(
        marked.contains(COMMENT_MARKER),
        "expected pending-comment marker:\n{marked}"
    );
}

#[test]
fn test_worktree_rows_use_number_prefix() {
    let items = vec![
        make_project("proj", 2),
        make_worktree("session-a"),
        make_worktree("session-b"),
    ];
    let lines = render_tree(&items, 40, 4);
    // First worktree starts with right-aligned "1"
    assert!(
        lines[1].trim_start().starts_with("1 "),
        "Expected number prefix in: '{}'",
        lines[1]
    );
    // Second worktree starts with "2"
    assert!(
        lines[2].trim_start().starts_with("2 "),
        "Expected number prefix in: '{}'",
        lines[2]
    );
    // No ASCII tree glyph — numbering is the only supported prefix.
    assert!(
        !lines[1].contains("└──"),
        "Tree glyph should no longer appear"
    );
}

#[test]
fn test_numbers_are_sequential_across_projects() {
    let items = vec![
        make_project("proj-a", 1),
        make_worktree("session-1"),
        make_project("proj-b", 1),
        make_worktree("session-2"),
    ];
    let lines = render_tree(&items, 40, 5);
    // Session under proj-a is #1
    assert!(
        lines[1].trim_start().starts_with("1 "),
        "Expected 1 in: '{}'",
        lines[1]
    );
    // Session under proj-b is #2 (not restarting)
    assert!(
        lines[3].trim_start().starts_with("2 "),
        "Expected 2 in: '{}'",
        lines[3]
    );
}

#[test]
fn test_double_digit_number_formatting() {
    let mut items = vec![make_project("proj", 12)];
    for i in 1..=12 {
        items.push(make_worktree(&format!("s-{}", i)));
    }
    let lines = render_tree(&items, 40, 14);
    // Single digit right-aligned
    assert!(
        lines[1].trim_start().starts_with("1 "),
        "line 1: '{}'",
        lines[1]
    );
    // Double digit
    assert!(
        lines[10].trim_start().starts_with("10 "),
        "line 10: '{}'",
        lines[10]
    );
    assert!(
        lines[12].trim_start().starts_with("12 "),
        "line 12: '{}'",
        lines[12]
    );
}

#[test]
fn test_tree_list_state_navigation() {
    let mut state = TreeListState::new();
    state.set_item_count(3);

    assert_eq!(state.selected(), None);

    state.next();
    assert_eq!(state.selected(), Some(0));

    state.next();
    assert_eq!(state.selected(), Some(1));

    state.next();
    assert_eq!(state.selected(), Some(2));

    // Wrap around
    state.next();
    assert_eq!(state.selected(), Some(0));

    // Previous
    state.previous();
    assert_eq!(state.selected(), Some(2));
}

#[test]
fn test_navigation_skips_unselectable_rows() {
    let mut state = TreeListState::new();
    // 5 rows; indices 0 and 3 are section headers (unselectable), rest are selectable.
    state.set_selectable(vec![false, true, true, false, true]);

    state.next();
    assert_eq!(state.selected(), Some(1));

    state.next();
    assert_eq!(state.selected(), Some(2));

    state.next();
    // skips 3, lands on 4
    assert_eq!(state.selected(), Some(4));

    state.next();
    // wraps; skips 0, lands on 1
    assert_eq!(state.selected(), Some(1));

    state.previous();
    // wraps backwards skipping 0
    assert_eq!(state.selected(), Some(4));

    state.previous();
    // skips 3
    assert_eq!(state.selected(), Some(2));
}

#[test]
fn test_select_nearest_lands_on_row_that_slid_up() {
    // Deleting the row at index 2 shifts the list down to 4 rows; selecting
    // the old index 2 lands on whatever moved into that slot (the next
    // sibling), not the top.
    let mut state = TreeListState::new();
    state.set_item_count(4);
    state.select_nearest(2);
    assert_eq!(state.selected(), Some(2));
}

#[test]
fn test_select_nearest_falls_back_to_previous_when_last_removed() {
    // Deleting the last row leaves 3 rows; the old index 3 is now past the
    // end, so the cursor falls back to the closest selectable row before it.
    let mut state = TreeListState::new();
    state.set_item_count(3);
    state.select_nearest(3);
    assert_eq!(state.selected(), Some(2));
}

#[test]
fn test_select_nearest_skips_unselectable_at_and_after_index() {
    // After a rebuild the old index lands on a spacer/header; prefer the
    // next selectable row below it rather than snapping to the top.
    let mut state = TreeListState::new();
    // index 2 unselectable (e.g. a spacer), index 3 selectable.
    state.set_selectable(vec![true, true, false, true, true]);
    state.select_nearest(2);
    assert_eq!(state.selected(), Some(3));
}

#[test]
fn test_select_nearest_falls_back_upward_past_trailing_unselectable() {
    // Everything at and after the old index is unselectable (deleted the
    // last sibling, leaving a trailing spacer) — fall back to the closest
    // selectable row above.
    let mut state = TreeListState::new();
    state.set_selectable(vec![true, true, false]);
    state.select_nearest(2);
    assert_eq!(state.selected(), Some(1));
}

#[test]
fn test_select_nearest_clears_selection_when_nothing_selectable() {
    let mut state = TreeListState::new();
    state.set_selectable(vec![false, false, false]);
    state.select(Some(1));
    state.select_nearest(1);
    assert_eq!(state.selected(), None);
}

#[test]
fn test_tree_list_state_empty() {
    let mut state = TreeListState::new();
    state.set_item_count(0);

    state.next();
    assert_eq!(state.selected(), None);

    state.previous();
    assert_eq!(state.selected(), None);
}

#[test]
fn test_previous_wraps_to_last() {
    let mut state = TreeListState::new();
    state.set_item_count(5);
    state.select(Some(0));

    state.previous();
    assert_eq!(state.selected(), Some(4));
}

#[test]
fn test_next_wraps_to_first() {
    let mut state = TreeListState::new();
    state.set_item_count(5);
    state.select(Some(4));

    state.next();
    assert_eq!(state.selected(), Some(0));
}

#[test]
fn test_next_group_jumps_between_group_starts() {
    let mut state = TreeListState::new();
    // Project-grouped layout: headers at 0 and 3, worktrees between.
    state.set_item_count(6);
    state.set_group_starts(vec![true, false, false, true, false, false]);

    state.select(Some(1));
    state.next_group();
    assert_eq!(state.selected(), Some(3));

    // Wraps past the end back to the first header.
    state.next_group();
    assert_eq!(state.selected(), Some(0));
}

#[test]
fn test_next_group_with_no_selection_selects_first_group() {
    let mut state = TreeListState::new();
    state.set_item_count(4);
    state.set_group_starts(vec![false, true, false, false]);

    state.next_group();
    assert_eq!(state.selected(), Some(1));
}

#[test]
fn test_previous_group_goes_to_current_group_header_first() {
    let mut state = TreeListState::new();
    state.set_item_count(6);
    state.set_group_starts(vec![true, false, false, true, false, false]);

    // From a row inside a group, land on that group's own header…
    state.select(Some(5));
    state.previous_group();
    assert_eq!(state.selected(), Some(3));

    // …and from a header, on the previous group's header.
    state.previous_group();
    assert_eq!(state.selected(), Some(0));
}

#[test]
fn test_previous_group_wraps_to_last_group() {
    let mut state = TreeListState::new();
    state.set_item_count(6);
    state.set_group_starts(vec![true, false, false, true, false, false]);

    state.select(Some(0));
    state.previous_group();
    assert_eq!(state.selected(), Some(3));
}

#[test]
fn test_group_jump_without_group_starts_is_noop() {
    let mut state = TreeListState::new();
    state.set_item_count(3);
    state.set_group_starts(vec![false, false, false]);
    state.select(Some(1));

    state.next_group();
    assert_eq!(state.selected(), Some(1));

    state.previous_group();
    assert_eq!(state.selected(), Some(1));
}

#[test]
fn test_group_jump_on_empty_list_is_noop() {
    let mut state = TreeListState::new();

    state.next_group();
    assert_eq!(state.selected(), None);

    state.previous_group();
    assert_eq!(state.selected(), None);
}

#[test]
fn test_group_jump_skips_unselectable_group_starts() {
    let mut state = TreeListState::new();
    state.set_selectable(vec![false, true, true, true]);
    state.set_group_starts(vec![true, false, true, false]);

    // Wrapping forward from the end: index 0 is a group start but
    // unselectable, so the jump continues to the next header at 2.
    state.select(Some(3));
    state.next_group();
    assert_eq!(state.selected(), Some(2));
}

#[test]
fn test_set_item_count_clears_stale_group_starts() {
    let mut state = TreeListState::new();
    state.set_item_count(4);
    state.set_group_starts(vec![false, true, false, false]);

    // Re-entering via set_item_count means "fresh list, no masks" — a
    // stale group mask would send the cursor to a row that may no longer
    // be a header.
    state.set_item_count(4);
    state.select(Some(2));
    state.next_group();
    assert_eq!(state.selected(), Some(2));
}

#[test]
fn test_select_first_and_last() {
    let mut state = TreeListState::new();
    state.set_item_count(5);

    state.select_first();
    assert_eq!(state.selected(), Some(0));

    state.select_last();
    assert_eq!(state.selected(), Some(4));
}

#[test]
fn test_select_first_and_last_skip_unselectable_edges() {
    let mut state = TreeListState::new();
    // Spacer rows at both edges.
    state.set_selectable(vec![false, true, true, false]);

    state.select_first();
    assert_eq!(state.selected(), Some(1));

    state.select_last();
    assert_eq!(state.selected(), Some(2));
}

#[test]
fn test_select_first_and_last_on_empty_list_are_noops() {
    let mut state = TreeListState::new();

    state.select_first();
    assert_eq!(state.selected(), None);

    state.select_last();
    assert_eq!(state.selected(), None);
}

#[test]
fn test_set_item_count_clears_stale_selectable_mask() {
    // Regression: cycling view modes used to leave the previous view's
    // per-index selectable mask in place. When the next view called
    // `set_item_count` (i.e. "no mask, just count — treat all rows as
    // selectable"), the stale mask still drove `is_selectable`, so some
    // Worktree rows in the project-grouped view became unreachable with
    // up/down navigation.
    let mut state = TreeListState::new();
    state.set_selectable(vec![true, false, true, false, true]);
    // Move to a row the mask still allows.
    state.select(Some(0));

    // Switching to "no mask" mode (what ProjectGrouped does).
    state.set_item_count(5);

    // Every index in [0, 5) should now be selectable, so `next()` from
    // 0 must land on 1, not skip to 2.
    state.next();
    assert_eq!(
        state.selected(),
        Some(1),
        "set_item_count should clear the prior set_selectable mask"
    );
    state.next();
    assert_eq!(state.selected(), Some(2));
    state.next();
    assert_eq!(state.selected(), Some(3));
}

#[test]
fn test_set_item_count_clamps_selection() {
    let mut state = TreeListState::new();
    state.set_item_count(10);
    state.select(Some(7));

    state.set_item_count(5);
    assert_eq!(state.selected(), Some(4));
}

#[test]
fn test_set_item_count_zero_clears_selection() {
    let mut state = TreeListState::new();
    state.set_item_count(5);
    state.select(Some(3));

    state.set_item_count(0);
    assert_eq!(state.selected(), None);
}

#[test]
fn test_set_item_count_preserves_valid_selection() {
    let mut state = TreeListState::new();
    state.set_item_count(10);
    state.select(Some(3));

    state.set_item_count(8);
    assert_eq!(state.selected(), Some(3));
}

#[test]
fn test_single_item_navigation() {
    let mut state = TreeListState::new();
    state.set_item_count(1);
    state.select(Some(0));

    state.next();
    assert_eq!(state.selected(), Some(0));

    state.previous();
    assert_eq!(state.selected(), Some(0));
}

#[test]
fn test_next_from_none_selects_first() {
    let mut state = TreeListState::new();
    state.set_item_count(3);
    assert_eq!(state.selected(), None);

    state.next();
    assert_eq!(state.selected(), Some(0));
}

#[test]
fn test_previous_from_none_selects_first() {
    let mut state = TreeListState::new();
    state.set_item_count(3);
    assert_eq!(state.selected(), None);

    state.previous();
    assert_eq!(state.selected(), Some(0));
}

// -- pr_badge_color --

fn review_labels() -> Vec<String> {
    vec![
        "dev-review-required".into(),
        "ready-for-test".into(),
        "trivial".into(),
    ]
}

#[test]
fn test_pr_badge_color_open() {
    let theme = Theme::basic();
    let c = pr_colors::pr_badge_color(
        &theme,
        Some(PrState::Open),
        false,
        false,
        &[],
        &review_labels(),
    );
    assert_eq!(c, theme.pr_open);
}

#[test]
fn test_pr_badge_color_merged() {
    let theme = Theme::basic();
    let c = pr_colors::pr_badge_color(
        &theme,
        Some(PrState::Merged),
        true,
        false,
        &[],
        &review_labels(),
    );
    assert_eq!(c, theme.status_pr_merged);
}

#[test]
fn test_pr_badge_color_closed() {
    let theme = Theme::basic();
    let c = pr_colors::pr_badge_color(
        &theme,
        Some(PrState::Closed),
        false,
        false,
        &[],
        &review_labels(),
    );
    assert_eq!(c, theme.pr_closed);
}

#[test]
fn test_pr_badge_color_draft_takes_precedence_over_label() {
    let theme = Theme::basic();
    let labels = vec!["dev-review-required".into()];
    let c = pr_colors::pr_badge_color(
        &theme,
        Some(PrState::Open),
        false,
        true,
        &labels,
        &review_labels(),
    );
    assert_eq!(c, theme.pr_draft);
}

#[test]
fn test_pr_badge_color_review_label_match() {
    let theme = Theme::basic();
    let labels = vec!["unrelated".into(), "ready-for-test".into()];
    let c = pr_colors::pr_badge_color(
        &theme,
        Some(PrState::Open),
        false,
        false,
        &labels,
        &review_labels(),
    );
    assert_eq!(c, theme.status_pr);
}

#[test]
fn test_pr_badge_color_review_label_case_insensitive() {
    let theme = Theme::basic();
    let labels = vec!["Dev-Review-Required".into()];
    let c = pr_colors::pr_badge_color(
        &theme,
        Some(PrState::Open),
        false,
        false,
        &labels,
        &review_labels(),
    );
    assert_eq!(c, theme.status_pr);
}

#[test]
fn test_pr_badge_color_non_matching_labels_fall_through_to_open() {
    let theme = Theme::basic();
    let labels = vec!["bug".into(), "documentation".into()];
    let c = pr_colors::pr_badge_color(
        &theme,
        Some(PrState::Open),
        false,
        false,
        &labels,
        &review_labels(),
    );
    assert_eq!(c, theme.pr_open);
}

#[test]
fn test_pr_badge_color_unknown_state_uses_pr_merged_flag_for_merged() {
    // Backward compat: pre-pr_state state.json with pr_merged=true
    let theme = Theme::basic();
    let c = pr_colors::pr_badge_color(&theme, None, true, false, &[], &review_labels());
    assert_eq!(c, theme.status_pr_merged);
}

#[test]
fn test_pr_badge_color_unknown_state_falls_back_to_open() {
    // Backward compat: pre-pr_state state.json with pr_merged=false
    let theme = Theme::basic();
    let c = pr_colors::pr_badge_color(&theme, None, false, false, &[], &review_labels());
    assert_eq!(c, theme.pr_open);
}

// -- session_status_glyph (single unified icon column) --

fn empty_items() -> [SessionListItem; 0] {
    []
}

fn make_tree<'a>(theme: &'a Theme, items: &'a [SessionListItem]) -> TreeList<'a> {
    TreeList::new(items, theme)
}

#[test]
fn test_glyph_working_shows_spinner() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, c) = tree
        .session_status_glyph(SessionStatus::Running, Some(AgentState::Working), false)
        .unwrap();
    assert!(SPINNER_FRAMES.contains(&g.as_str()));
    // Default theme uses Rainbow → colour comes from the rainbow palette
    assert!(crate::config::theme::RAINBOW_PALETTE.contains(&c));
}

#[test]
fn test_glyph_working_beats_unread() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, _) = tree
        .session_status_glyph(SessionStatus::Running, Some(AgentState::Working), true)
        .unwrap();
    assert!(SPINNER_FRAMES.contains(&g.as_str()));
}

#[test]
fn test_glyph_waiting_for_input() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, c) = tree
        .session_status_glyph(
            SessionStatus::Running,
            Some(AgentState::WaitingForInput),
            false,
        )
        .unwrap();
    assert_eq!(g, "?");
    assert_eq!(c, theme.agent_waiting);
}

#[test]
fn test_glyph_waiting_beats_unread() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, _) = tree
        .session_status_glyph(
            SessionStatus::Running,
            Some(AgentState::WaitingForInput),
            true,
        )
        .unwrap();
    assert_eq!(g, "?");
}

#[test]
fn test_glyph_unread_when_idle() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, c) = tree
        .session_status_glyph(SessionStatus::Running, Some(AgentState::Idle), true)
        .unwrap();
    assert_eq!(g, "◆");
    assert_eq!(c, theme.unread_indicator);
}

#[test]
fn test_glyph_running_idle_no_unread() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, c) = tree
        .session_status_glyph(SessionStatus::Running, Some(AgentState::Idle), false)
        .unwrap();
    assert_eq!(g, "●");
    assert_eq!(c, theme.status_running);
}

#[test]
fn test_glyph_running_unknown_no_unread() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, c) = tree
        .session_status_glyph(SessionStatus::Running, Some(AgentState::Unknown), false)
        .unwrap();
    assert_eq!(g, "●");
    assert_eq!(c, theme.status_running);
}

#[test]
fn test_glyph_running_no_agent_state_no_unread() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, c) = tree
        .session_status_glyph(SessionStatus::Running, None, false)
        .unwrap();
    assert_eq!(g, "●");
    assert_eq!(c, theme.status_running);
}

#[test]
fn test_glyph_stopped() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, c) = tree
        .session_status_glyph(SessionStatus::Stopped, None, false)
        .unwrap();
    assert_eq!(g, "○");
    assert_eq!(c, theme.status_stopped);
}

#[test]
fn test_glyph_stopped_ignores_unread_and_agent_state() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    // Stopped sessions can't have a meaningful agent state, but ensure
    // the glyph is consistent regardless.
    let (g, _) = tree
        .session_status_glyph(SessionStatus::Stopped, Some(AgentState::Working), true)
        .unwrap();
    assert_eq!(g, "○");
}

#[test]
fn test_glyph_creating_shows_spinner() {
    let theme = Theme::basic();
    let items = empty_items();
    let tree = make_tree(&theme, &items);
    let (g, c) = tree
        .session_status_glyph(SessionStatus::Creating, None, false)
        .unwrap();
    assert!(SPINNER_FRAMES.contains(&g.as_str()));
    assert_eq!(c, theme.status_creating);
}

// Cycle-arithmetic tests for the spinner index computation in
// `session_status_glyph`: `SPINNER_FRAMES[(tick as usize / 3) % SPINNER_FRAMES.len()]`.
// SPINNER_FRAMES has 10 entries, so the cycle is `(tick / 3) % 10`. Each
// test below picks tick values whose original-vs-mutant outputs differ:
//
//   tick=3  → original `(3/3)%10 = 1` ⇒ frame[1] = "⠙"
//             `/`→`%`: `(3%3)%10 = 0` ⇒ "⠋"  (distinguishes / from %)
//             `/`→`*`: `(3*3)%10 = 9` ⇒ "⠏"  (distinguishes / from *)
//             `%`→`/`: `(3/3)/10 = 0` ⇒ "⠋"  (distinguishes % from /)
//   tick=30 → original `(30/3)%10 = 0` ⇒ "⠋"
//             `%`→`/`: `(30/3)/10 = 1` ⇒ "⠙"  (extra guard for % from /)

#[test]
fn test_glyph_creating_spinner_uses_tick_div_three_mod_len() {
    let theme = Theme::basic();
    let items = empty_items();

    // tick=3: original index 1 → "⠙"
    let (g, _) = make_tree(&theme, &items)
        .tick(3)
        .session_status_glyph(SessionStatus::Creating, None, false)
        .unwrap();
    assert_eq!(
        g, "⠙",
        "tick=3 must select SPINNER_FRAMES[(3/3)%10] = SPINNER_FRAMES[1]"
    );

    // tick=30: original index 0 → "⠋" (catches `%` → `/` which would give 1)
    let (g, _) = make_tree(&theme, &items)
        .tick(30)
        .session_status_glyph(SessionStatus::Creating, None, false)
        .unwrap();
    assert_eq!(
        g, "⠋",
        "tick=30 must select SPINNER_FRAMES[(30/3)%10] = SPINNER_FRAMES[0]"
    );
}

#[test]
fn test_glyph_working_spinner_uses_tick_div_three_mod_len() {
    let theme = Theme::basic();
    let items = empty_items();

    // tick=3: original index 1 → "⠙"
    let (g, _) = make_tree(&theme, &items)
        .tick(3)
        .session_status_glyph(SessionStatus::Running, Some(AgentState::Working), false)
        .unwrap();
    assert_eq!(
        g, "⠙",
        "tick=3 must select SPINNER_FRAMES[(3/3)%10] = SPINNER_FRAMES[1]"
    );

    // tick=30: original index 0 → "⠋" (catches `%` → `/` which would give 1)
    let (g, _) = make_tree(&theme, &items)
        .tick(30)
        .session_status_glyph(SessionStatus::Running, Some(AgentState::Working), false)
        .unwrap();
    assert_eq!(
        g, "⠋",
        "tick=30 must select SPINNER_FRAMES[(30/3)%10] = SPINNER_FRAMES[0]"
    );
}

#[test]
fn test_stacked_child_row_has_extra_indent() {
    // The stacked child should sit one extra indent (STACK_INDENT) further
    // right than its base. The prefix is the right-aligned session number.
    let items = vec![
        make_project("proj", 2),
        make_worktree_with_stack("base", false),
        make_worktree_with_stack("child", true),
    ];
    let lines = render_tree(&items, 60, 4);

    let base_line = &lines[1];
    let child_line = &lines[2];

    // "1 " marks the base row, "2 " marks the child row. The child's number
    // should sit STACK_INDENT columns further right.
    let base_idx = base_line
        .find("1 ")
        .expect("base should have number prefix");
    let child_idx = child_line
        .find("2 ")
        .expect("child should have number prefix");
    assert_eq!(
        child_idx - base_idx,
        STACK_INDENT.chars().count(),
        "stacked child indent should be exactly STACK_INDENT wider than the base\nbase:  {base_line:?}\nchild: {child_line:?}"
    );
}

// -- section header collapsible rendering --

fn make_section_header(name: &str, count: usize, collapsed: bool) -> SessionListItem {
    SessionListItem::SectionHeader {
        name: name.to_string(),
        count,
        collapsed,
        max_sessions: None,
    }
}

/// Section header with a WIP limit set. Used by the tests that exercise the
/// `(count/limit)` suffix rendering (text + colour ramp).
fn make_section_header_with_limit(count: usize, limit: u32) -> SessionListItem {
    let mut h = make_section_header("Review", count, false);
    if let SessionListItem::SectionHeader { max_sessions, .. } = &mut h {
        *max_sessions = Some(limit);
    }
    h
}

#[test]
fn test_section_header_expanded_shows_down_twistie() {
    let items = vec![
        make_section_header("In Progress", 2, false),
        make_project("proj", 2),
        make_worktree("sess-a"),
    ];
    let lines = render_tree(&items, 60, 4);
    assert!(
        lines[0].contains("▾"),
        "Expected down-twistie for expanded section: {:?}",
        lines[0]
    );
    assert!(
        !lines[0].contains("▸"),
        "Should not have right-twistie when expanded: {:?}",
        lines[0]
    );
}

#[test]
fn test_section_header_collapsed_shows_right_twistie() {
    let items = vec![make_section_header("Done", 3, true)];
    let lines = render_tree(&items, 60, 2);
    assert!(
        lines[0].contains("▸"),
        "Expected right-twistie for collapsed section: {:?}",
        lines[0]
    );
    assert!(
        !lines[0].contains("▾"),
        "Should not have down-twistie when collapsed: {:?}",
        lines[0]
    );
}

#[test]
fn test_section_header_shows_count() {
    let items = vec![make_section_header("Review", 5, false)];
    let lines = render_tree(&items, 60, 2);
    assert!(
        lines[0].contains("(5)"),
        "Expected count in section header: {:?}",
        lines[0]
    );
}

#[test]
fn test_section_header_shows_count_over_limit_when_max_sessions_set() {
    let lines = render_tree(&[make_section_header_with_limit(3, 2)], 60, 2);
    assert!(
        lines[0].contains("(3/2)"),
        "Expected count/limit display when over limit: {:?}",
        lines[0]
    );
}

#[test]
fn test_section_header_shows_count_under_limit_when_max_sessions_set() {
    let lines = render_tree(&[make_section_header_with_limit(1, 5)], 60, 2);
    assert!(
        lines[0].contains("(1/5)"),
        "Expected count/limit display when under limit: {:?}",
        lines[0]
    );
}

/// Assert that the `(count/limit)` suffix rendered on the section header uses
/// `expected` as its foreground colour. Uses `find_text_in_row` (the same
/// needle-scanner production code uses) so it can't mis-anchor on a stray `(`
/// in the section name.
fn assert_count_suffix_colour(count: usize, limit: u32, expected: Color) {
    let buf = draw_tree_buffer(
        &[make_section_header_with_limit(count, limit)],
        60,
        2,
        |t| t,
    );
    let needle = format!("({count}/{limit})");
    let x = super::render::find_text_in_row(&buf, 0, 0, 60, &needle)
        .unwrap_or_else(|| panic!("count suffix {needle:?} not found in rendered row"));
    assert_eq!(buf[(x, 0)].style().fg, Some(expected));
}

#[test]
fn section_header_under_limit_uses_secondary_colour() {
    assert_count_suffix_colour(1, 5, Theme::basic().text_secondary);
}

#[test]
fn section_header_at_limit_uses_warning_colour() {
    assert_count_suffix_colour(2, 2, Theme::basic().modal_warning);
}

#[test]
fn section_header_over_limit_uses_error_colour() {
    assert_count_suffix_colour(3, 2, Theme::basic().modal_error);
}

#[test]
fn test_section_header_is_selectable() {
    let header = make_section_header("In Progress", 2, false);
    assert!(header.is_selectable());
}

#[test]
fn test_navigation_lands_on_section_headers() {
    let mut state = TreeListState::new();
    // Section header, spacer, section header — headers selectable, spacers not.
    state.set_selectable(vec![true, false, true]);

    state.next();
    assert_eq!(state.selected(), Some(0));

    state.next();
    assert_eq!(state.selected(), Some(2));

    state.previous();
    assert_eq!(state.selected(), Some(0));
}

// -- show_session_program toggle --

#[test]
fn test_program_suffix_shown_by_default_when_mixed() {
    let items = vec![
        make_project("proj", 2),
        make_worktree_with_program("sess-a", "claude"),
        make_worktree_with_program("sess-b", "codex"),
    ];
    let lines = render_tree(&items, 60, 4);
    assert!(
        lines[1].contains("(claude)"),
        "expected program suffix: {:?}",
        lines[1]
    );
    assert!(
        lines[2].contains("(codex)"),
        "expected program suffix: {:?}",
        lines[2]
    );
}

#[test]
fn test_program_suffix_hidden_when_flag_disabled() {
    let items = vec![
        make_project("proj", 2),
        make_worktree_with_program("sess-a", "claude"),
        make_worktree_with_program("sess-b", "codex"),
    ];
    let lines = render_tree_with(&items, 60, 4, |t| t.show_session_program(false));
    assert!(
        !lines[1].contains("(claude)"),
        "expected no program suffix: {:?}",
        lines[1]
    );
    assert!(
        !lines[2].contains("(codex)"),
        "expected no program suffix: {:?}",
        lines[2]
    );
}

#[test]
fn test_program_suffix_hidden_when_programs_uniform() {
    // Uniform program: suffix hidden regardless of the flag.
    let items = vec![
        make_project("proj", 2),
        make_worktree_with_program("sess-a", "claude"),
        make_worktree_with_program("sess-b", "claude"),
    ];
    let lines = render_tree(&items, 60, 4);
    assert!(
        !lines[1].contains("(claude)"),
        "uniform programs should not render suffix: {:?}",
        lines[1]
    );
}

#[test]
fn test_program_suffix_hidden_when_only_args_differ() {
    // Same base program with differing args is not "mixed": no suffix shown.
    let items = vec![
        make_project("proj", 2),
        make_worktree_with_program("sess-a", "claude"),
        make_worktree_with_program("sess-b", "claude --mode auto"),
    ];
    let lines = render_tree(&items, 60, 4);
    assert!(
        !lines[1].contains("(claude)"),
        "args-only difference should not render suffix: {:?}",
        lines[1]
    );
    assert!(
        !lines[2].contains("(claude"),
        "args-only difference should not render suffix: {:?}",
        lines[2]
    );
}

#[test]
fn test_program_suffix_shows_base_name_only_when_mixed() {
    // Different base programs trigger the suffix, but args are stripped from display.
    let items = vec![
        make_project("proj", 2),
        make_worktree_with_program("sess-a", "codex"),
        make_worktree_with_program("sess-b", "claude --mode auto"),
    ];
    let lines = render_tree(&items, 60, 4);
    assert!(
        lines[1].contains("(codex)"),
        "expected base program suffix: {:?}",
        lines[1]
    );
    assert!(
        lines[2].contains("(claude)") && !lines[2].contains("--mode"),
        "expected base name only, no args: {:?}",
        lines[2]
    );
}

// -- TreeListState boundary tests (cargo-mutants gap closure) --

#[test]
fn test_previous_on_empty_state_with_zero_index_does_not_underflow() {
    // Regression: `any_selectable()` must report `false` for an empty state so
    // that `previous()` early-returns before computing `count - 1`. If
    // `any_selectable` were to always return `true`, or if `item_count > 0`
    // were weakened to `>= 0`, this call would underflow `count - 1` (count=0)
    // and panic in debug builds.
    let mut state = TreeListState::new();
    // No `set_item_count`/`set_selectable` calls: item_count == 0, selectable empty.
    // Force selection to Some(0) so that `previous()` hits the `i == 0` arm
    // that does `count - 1` once it gets past the early-return guard.
    state.select(Some(0));
    state.previous();
    // No panic, no spurious selection change.
    assert_eq!(state.selected(), Some(0));
}

#[test]
fn test_next_on_empty_state_with_zero_index_is_a_noop() {
    // Mirror of the above test for `next()`: even if the early-return guard
    // were flipped, the modulo loop would refuse to select on an empty count,
    // but we still pin the observable behaviour to a no-op.
    let mut state = TreeListState::new();
    state.select(Some(0));
    state.next();
    assert_eq!(state.selected(), Some(0));
}

#[test]
fn test_navigation_noop_when_all_rows_unselectable() {
    // With every row unselectable, `any_selectable()` must return false. The
    // existing `selectable.iter().any(|s| *s)` branch is exercised here: if it
    // were replaced with `true`, the loop would still find no selectable index
    // but we still want to assert the observable contract — `next()` does not
    // produce a selection from `None`.
    let mut state = TreeListState::new();
    state.set_selectable(vec![false, false, false]);
    assert_eq!(state.selected(), None);
    state.next();
    assert_eq!(state.selected(), None);
    state.previous();
    assert_eq!(state.selected(), None);
}

#[test]
fn test_next_wraps_via_modulo_not_addition() {
    // Pin the wrap-around contract: from the last index, `next()` selects 0.
    // If `(i + 1) % count` were replaced with `(i + 1) + count`, the start
    // value would be `count * 2` rather than `0`; the subsequent
    // `(start + offset) % count` still normalises to zero, but we drive the
    // selectable mask so the *first* attempt lands on the wrap target.
    let mut state = TreeListState::new();
    state.set_item_count(4);
    state.select(Some(3));
    state.next();
    assert_eq!(state.selected(), Some(0));

    // And with an unselectable mask that forces traversal past the wrap point,
    // confirm we wrap forward to the next selectable row rather than walking
    // off the end.
    let mut state = TreeListState::new();
    state.set_selectable(vec![true, false, false, false]);
    state.select(Some(0));
    state.next();
    // Only index 0 is selectable, so `next()` from 0 wraps to 0.
    assert_eq!(state.selected(), Some(0));
}

#[test]
fn test_set_selectable_clears_out_of_range_selection() {
    // Kills `>= -> <` and `|| -> &&` mutants in set_selectable: when the
    // current selection is past the end of the new mask, the selection must
    // be cleared. `is_selectable(sel)` on an out-of-range index returns `true`
    // (via `unwrap_or(true)`), so the `!is_selectable(sel)` half of the `||`
    // is `false`; only the `sel >= item_count` half is `true`. Replacing `>=`
    // with `<` makes the whole expression `false`; replacing `||` with `&&`
    // also yields `false`. Either way, the mutant would *keep* the selection.
    let mut state = TreeListState::new();
    state.set_item_count(10);
    state.select(Some(7));

    state.set_selectable(vec![true, true, true]);
    assert_eq!(
        state.selected(),
        None,
        "out-of-range selection must be cleared"
    );
}

#[test]
fn test_set_selectable_clears_selection_on_unselectable_index() {
    // Kills `delete !` mutant: when the current selection is in-range but
    // points at an unselectable row, the selection must be cleared. With the
    // `!` deleted, the condition becomes `sel >= item_count || is_selectable(sel)`,
    // which is `false || false = false`, leaving the selection intact.
    let mut state = TreeListState::new();
    state.set_item_count(3);
    state.select(Some(1));

    state.set_selectable(vec![true, false, true]);
    assert_eq!(
        state.selected(),
        None,
        "selection on an unselectable row must be cleared"
    );
}

#[test]
fn test_set_selectable_preserves_valid_selection() {
    // Companion to the above: when the selection is in-range AND selectable,
    // it must be preserved. Guards against an overly-aggressive clear.
    let mut state = TreeListState::new();
    state.set_item_count(3);
    state.select(Some(2));

    state.set_selectable(vec![true, false, true]);
    assert_eq!(state.selected(), Some(2));
}
