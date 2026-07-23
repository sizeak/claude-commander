use super::actions::{adjust_list_scroll, delete_confirm_message};
use super::modals::centered_rect;
use super::render::commander_chip_label;
use super::review::ReviewFocus;
use super::selection::{session_number_to_list_index, worktree_list_index};
use super::*;

#[test]
fn test_delete_confirm_message_names_session() {
    let message = delete_confirm_message(Some("fix-login-bug"), None);
    assert!(
        message.contains("\"fix-login-bug\""),
        "message should name the session: {message}"
    );
    assert!(message.contains("kill the tmux session"));
    assert!(
        !message.contains("retargeted"),
        "no retarget note when there are no stacked children: {message}"
    );
}

#[test]
fn restart_confirm_message_local_promises_resume_remote_stays_neutral() {
    use super::actions::restart_confirm_message;
    // Local sessions: the client's `resume_session` governs the wording, so it
    // can promise `/resume`.
    assert!(restart_confirm_message(true, true, Some("x")).contains("/resume"));
    assert!(restart_confirm_message(true, false, Some("x")).contains("/resume"));
    // Remote sessions: resume behaviour lives in the server's config, which the
    // client can't read — so the wording must NOT promise it, and should name
    // the session.
    let remote = restart_confirm_message(false, true, Some("my-sess"));
    assert!(
        !remote.contains("/resume"),
        "remote restart must not promise resume semantics: {remote}"
    );
    assert!(
        remote.contains("my-sess"),
        "should name the session: {remote}"
    );
}

#[test]
fn test_delete_confirm_message_falls_back_without_title() {
    let message = delete_confirm_message(None, None);
    assert!(message.contains("this session"));
    assert!(!message.contains('"'));
}

#[test]
fn test_delete_confirm_message_notes_stacked_child_retarget() {
    // Singular and plural phrasing, naming the branch children move onto.
    let one = delete_confirm_message(Some("c"), Some((1, "b")));
    assert!(
        one.contains("1 stacked session will be retargeted onto \"b\"."),
        "singular retarget note: {one}"
    );
    let many = delete_confirm_message(Some("c"), Some((3, "main")));
    assert!(
        many.contains("3 stacked sessions will be retargeted onto \"main\"."),
        "plural retarget note: {many}"
    );
}

#[test]
fn test_centered_rect() {
    let area = Rect::new(0, 0, 100, 50);
    let centered = centered_rect(50, 50, area);

    // Should be roughly centered
    assert!(centered.x > 0);
    assert!(centered.y > 0);
    assert!(centered.width < area.width);
    assert!(centered.height < area.height);
}

#[test]
fn test_should_auto_restart_regular_claude_session() {
    // A normal Claude session that just ended should be auto-restarted.
    assert!(should_auto_restart_ended("my-feature", 0));
}

#[test]
fn test_should_not_auto_restart_after_repeated_ends() {
    // Crash-loop guard: stop after 3 consecutive ends.
    assert!(!should_auto_restart_ended("my-feature", 3));
}

#[test]
fn test_should_not_auto_restart_shell_session() {
    // Shell sessions (suffix `-sh`) are not Claude sessions.
    assert!(!should_auto_restart_ended("my-feature-sh", 0));
}

#[test]
fn test_should_not_auto_restart_commander() {
    // The commander is project-less and absent from `state.sessions`, so a
    // restart-by-name would fail; it is revived lazily on next open instead.
    assert!(!should_auto_restart_ended(
        crate::commander::COMMANDER_TMUX_NAME,
        0
    ));
}

// The agent-state poll tick decisions (`poll_tick_can_skip`/`poll_tick_should_send`)
// moved into the service with the background loops; their tests now live in
// `crate::api`.

// --- commander status-bar chip label ---------------------------------------

#[test]
fn commander_chip_hidden_when_stopped() {
    // Not running → no chip, regardless of any stale agent state.
    assert_eq!(commander_chip_label(false, None), None);
    assert_eq!(commander_chip_label(false, Some(AgentState::Working)), None);
}

#[test]
fn commander_chip_label_per_agent_state() {
    // Running with a known state appends the state suffix.
    assert_eq!(
        commander_chip_label(true, Some(AgentState::Working)),
        Some("\u{25cf} Commander \u{00b7} working".to_string())
    );
    assert_eq!(
        commander_chip_label(true, Some(AgentState::WaitingForInput)),
        Some("\u{25cf} Commander \u{00b7} waiting".to_string())
    );
    assert_eq!(
        commander_chip_label(true, Some(AgentState::Idle)),
        Some("\u{25cf} Commander \u{00b7} idle".to_string())
    );
}

#[test]
fn commander_chip_label_running_without_state() {
    // Running but state not yet polled (or Unknown) → bare chip, no suffix.
    assert_eq!(
        commander_chip_label(true, None),
        Some("\u{25cf} Commander".to_string())
    );
    assert_eq!(
        commander_chip_label(true, Some(AgentState::Unknown)),
        Some("\u{25cf} Commander".to_string())
    );
}

#[test]
fn test_app_ui_state_default() {
    let state = AppUiState::default();
    assert!(state.list_items.is_empty());
    assert!(matches!(state.focused_pane, FocusedPane::SessionList));
    assert!(matches!(state.modal, Modal::None));
    assert!(!state.should_quit);
}

fn make_project() -> SessionListItem {
    SessionListItem::Project {
        id: ProjectId::new(),
        name: "test".to_string(),
        repo_path: std::path::PathBuf::from("/tmp/test"),
        main_branch: "main".to_string(),
        worktree_count: 0,
        nested: false,
    }
}

fn make_worktree() -> SessionListItem {
    make_worktree_with_id(SessionId::new())
}

fn make_worktree_with_id(id: SessionId) -> SessionListItem {
    SessionListItem::Worktree {
        id,
        project_id: ProjectId::new(),
        title: "test".to_string(),
        branch: "feat".to_string(),
        status: SessionStatus::Running,
        program: "claude".to_string(),
        pr_number: None,
        pr_url: None,
        pr_merged: false,
        pr_state: None,
        pr_draft: false,
        pr_labels: Vec::new(),
        worktree_path: std::path::PathBuf::from("/tmp/test"),
        created_at: chrono::Utc::now(),
        agent_state: None,
        unread: false,
        keep_alive: false,
        lfs_pulling: false,
        stacked_child: false,
    }
}

fn make_recent_session(id: SessionId) -> SessionListItem {
    SessionListItem::RecentSession {
        session: crate::backend::SessionRef::local(id),
        project_id: ProjectId::new(),
        title: "recent".to_string(),
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

#[test]
fn test_recent_rows_do_not_shift_session_numbers() {
    // A recents block (header + shortcut rows + divider) sits above the real
    // tree. Those rows must NOT be counted as worktrees, so the session
    // numbers still map to the real `Worktree` rows exactly as they would
    // without the block.
    let real_a = SessionId::new();
    let real_b = SessionId::new();
    let items = vec![
        SessionListItem::RecentsHeader,
        make_recent_session(real_b),
        make_recent_session(real_a),
        SessionListItem::Spacer,
        make_project(),
        make_worktree_with_id(real_a), // session #1
        make_worktree_with_id(real_b), // session #2
    ];
    // #1 and #2 resolve to the real worktree rows, not the recent shortcuts.
    assert_eq!(session_number_to_list_index(&items, 1), Some(5));
    assert_eq!(session_number_to_list_index(&items, 2), Some(6));
    // Only two sessions exist despite four session-bearing rows on screen.
    assert_eq!(session_number_to_list_index(&items, 3), None);
}

#[test]
fn test_recents_header_and_divider_are_not_selectable() {
    assert!(!SessionListItem::RecentsHeader.is_selectable());
    assert!(!SessionListItem::Spacer.is_selectable());
    // A recent-session row is a navigable shortcut.
    assert!(make_recent_session(SessionId::new()).is_selectable());
    // Neither the header nor a recent row is a group-jump target.
    assert!(!SessionListItem::RecentsHeader.is_group_header());
    assert!(!make_recent_session(SessionId::new()).is_group_header());
}

#[test]
fn test_session_number_to_list_index_basic() {
    let items = vec![
        make_project(),
        make_worktree(), // index 1, session #1
        make_worktree(), // index 2, session #2
        make_project(),
        make_worktree(), // index 4, session #3
    ];
    assert_eq!(session_number_to_list_index(&items, 1), Some(1));
    assert_eq!(session_number_to_list_index(&items, 2), Some(2));
    assert_eq!(session_number_to_list_index(&items, 3), Some(4));
}

#[test]
fn test_session_number_to_list_index_out_of_range() {
    let items = vec![make_project(), make_worktree()];
    assert_eq!(session_number_to_list_index(&items, 2), None);
    assert_eq!(session_number_to_list_index(&items, 0), None);
}

#[test]
fn test_session_number_to_list_index_empty() {
    let items: Vec<SessionListItem> = vec![];
    assert_eq!(session_number_to_list_index(&items, 1), None);
}

#[test]
fn test_session_number_to_list_index_projects_only() {
    let items = vec![make_project(), make_project()];
    assert_eq!(session_number_to_list_index(&items, 1), None);
}

#[test]
fn test_worktree_list_index_finds_session_row() {
    let target = SessionId::new();
    // `target` sits at index 2, between other worktrees and a project header.
    let items = vec![
        make_project(),
        make_worktree(),
        make_worktree_with_id(target),
        make_project(),
        make_worktree(),
    ];
    assert_eq!(worktree_list_index(&items, target), Some(2));
}

#[test]
fn test_worktree_list_index_absent_session_returns_none() {
    // A session that has no row in the list (e.g. it was deleted, or the
    // user detached from a session that has since ended) must not select
    // some other row by accident.
    let items = vec![make_project(), make_worktree(), make_worktree()];
    assert_eq!(worktree_list_index(&items, SessionId::new()), None);
}

// ---------------------------------------------------------------------------
// Quick-switch palette expansion: session + command matching
// ---------------------------------------------------------------------------

use crate::config::KeyBindings;

// --- effective_palette_mode -------------------------------------------------

#[test]
fn test_effective_mode_unified_plain_query_stays_unified() {
    assert_eq!(
        App::effective_palette_mode(PaletteMode::Unified, ""),
        PaletteMode::Unified
    );
    assert_eq!(
        App::effective_palette_mode(PaletteMode::Unified, "foo"),
        PaletteMode::Unified
    );
}

#[test]
fn test_effective_mode_unified_gt_prefix_promotes_to_command_only() {
    assert_eq!(
        App::effective_palette_mode(PaletteMode::Unified, ">"),
        PaletteMode::CommandOnly
    );
    assert_eq!(
        App::effective_palette_mode(PaletteMode::Unified, "> foo"),
        PaletteMode::CommandOnly
    );
    assert_eq!(
        App::effective_palette_mode(PaletteMode::Unified, ">foo"),
        PaletteMode::CommandOnly
    );
}

#[test]
fn test_effective_mode_command_only_is_sticky() {
    // Shift+leader entry mode stays CommandOnly regardless of query prefix
    assert_eq!(
        App::effective_palette_mode(PaletteMode::CommandOnly, ""),
        PaletteMode::CommandOnly
    );
    assert_eq!(
        App::effective_palette_mode(PaletteMode::CommandOnly, "foo"),
        PaletteMode::CommandOnly
    );
}

// --- palette_filter_query ---------------------------------------------------

#[test]
fn test_palette_filter_query_strips_gt_prefix_only_in_command_only() {
    // Unified keeps the query verbatim (session search uses `>` literally if typed)
    assert_eq!(
        App::palette_filter_query(PaletteMode::Unified, "foo"),
        "foo"
    );
    // CommandOnly derived from `>` prefix strips it and trims following space
    assert_eq!(App::palette_filter_query(PaletteMode::CommandOnly, ">"), "");
    assert_eq!(
        App::palette_filter_query(PaletteMode::CommandOnly, "> foo"),
        "foo"
    );
    assert_eq!(
        App::palette_filter_query(PaletteMode::CommandOnly, ">foo"),
        "foo"
    );
    // CommandOnly without a `>` prefix (Shift+leader entry) passes through
    assert_eq!(
        App::palette_filter_query(PaletteMode::CommandOnly, "foo"),
        "foo"
    );
}

// --- is_command_available ---------------------------------------------------

fn ui_state_with(
    session: Option<SessionId>,
    project: Option<ProjectId>,
    right_pane: RightPaneView,
) -> AppUiState {
    AppUiState {
        selected_session_id: session.map(crate::backend::SessionRef::local),
        selected_project_id: project.map(|p| (crate::backend::LOCAL_BACKEND_ID, p)),
        right_pane_view: right_pane,
        ..AppUiState::default()
    }
}

#[test]
fn test_is_command_available_session_scoped_hidden_without_session() {
    let s = ui_state_with(None, None, RightPaneView::Preview);
    for action in [
        BindableAction::Select,
        BindableAction::SelectShell,
        BindableAction::DeleteSession,
        BindableAction::RenameSession,
        BindableAction::RestartSession,
        BindableAction::ToggleKeepAlive,
        BindableAction::OpenInEditor,
        BindableAction::OpenPullRequest,
    ] {
        assert!(
            !s.is_command_available(action),
            "{action:?} should be hidden without a session"
        );
    }
}

#[test]
fn test_is_command_available_session_scoped_shown_with_session() {
    let s = ui_state_with(Some(SessionId::new()), None, RightPaneView::Preview);
    for action in [
        BindableAction::Select,
        BindableAction::SelectShell,
        BindableAction::DeleteSession,
        BindableAction::RenameSession,
        BindableAction::RestartSession,
        BindableAction::ToggleKeepAlive,
        BindableAction::OpenInEditor,
        BindableAction::OpenPullRequest,
    ] {
        assert!(
            s.is_command_available(action),
            "{action:?} should be available with a selected session"
        );
    }
}

#[test]
fn test_is_command_available_remove_project_requires_project_without_session() {
    // project selected, no session → shown
    let s = ui_state_with(None, Some(ProjectId::new()), RightPaneView::Preview);
    assert!(s.is_command_available(BindableAction::RemoveProject));
    // project selected but session also selected → hidden
    let s = ui_state_with(
        Some(SessionId::new()),
        Some(ProjectId::new()),
        RightPaneView::Preview,
    );
    assert!(!s.is_command_available(BindableAction::RemoveProject));
    // nothing selected → hidden
    let s = ui_state_with(None, None, RightPaneView::Preview);
    assert!(!s.is_command_available(BindableAction::RemoveProject));
}

#[test]
fn test_is_command_available_generate_summary_requires_info_pane_and_session() {
    // info pane + session → shown
    let s = ui_state_with(Some(SessionId::new()), None, RightPaneView::Info);
    assert!(s.is_command_available(BindableAction::GenerateSummary));
    // info pane but no session → hidden
    let s = ui_state_with(None, None, RightPaneView::Info);
    assert!(!s.is_command_available(BindableAction::GenerateSummary));
    // session but preview pane → hidden
    let s = ui_state_with(Some(SessionId::new()), None, RightPaneView::Preview);
    assert!(!s.is_command_available(BindableAction::GenerateSummary));
}

#[test]
fn new_session_and_checkout_gated_on_selected_backend_connected() {
    // A degraded/connecting remote backend can't service create-options or a
    // branch listing, and awaiting them would stall the event loop — so both
    // commands must drop out of the palette when the selected backend is down.
    let mut s = ui_state_with(None, Some(ProjectId::new()), RightPaneView::Preview);
    s.selected_backend_connected = false;
    assert!(!s.is_command_available(BindableAction::NewSession));
    assert!(!s.is_command_available(BindableAction::CheckoutBranch));

    // A live backend keeps them available.
    s.selected_backend_connected = true;
    assert!(s.is_command_available(BindableAction::NewSession));
    assert!(s.is_command_available(BindableAction::CheckoutBranch));
}

#[test]
fn test_is_command_available_unguarded_always_shown() {
    let s = ui_state_with(None, None, RightPaneView::Preview);
    for action in [
        BindableAction::NewSession,
        BindableAction::NewProject,
        BindableAction::CheckoutBranch,
        BindableAction::ScanDirectory,
        BindableAction::TogglePane,
        BindableAction::TogglePaneReverse,
        BindableAction::ShrinkLeftPane,
        BindableAction::GrowLeftPane,
        BindableAction::ShowHelp,
        BindableAction::ShowSettings,
        BindableAction::Quit,
        BindableAction::ScrollUp,
        BindableAction::ScrollDown,
        BindableAction::PageUp,
        BindableAction::PageDown,
    ] {
        assert!(
            s.is_command_available(action),
            "{action:?} should always be available"
        );
    }
}

// --- gather_command_entries -------------------------------------------------

#[test]
fn test_gather_command_entries_excludes_navigation() {
    let s = ui_state_with(Some(SessionId::new()), None, RightPaneView::Info);
    let kb = KeyBindings::default();
    let entries = s.gather_command_entries(&kb, "");
    for e in &entries {
        assert!(
            !matches!(
                e.action,
                BindableAction::NavigateUp | BindableAction::NavigateDown
            ),
            "palette should never list list-navigation actions, got {:?}",
            e.action
        );
    }
}

#[test]
fn test_gather_command_entries_hides_context_unavailable() {
    // nothing selected → session-scoped and GenerateSummary all hidden
    let s = ui_state_with(None, None, RightPaneView::Preview);
    let kb = KeyBindings::default();
    let entries = s.gather_command_entries(&kb, "");
    let actions: std::collections::HashSet<BindableAction> =
        entries.iter().map(|e| e.action).collect();
    for hidden in [
        BindableAction::DeleteSession,
        BindableAction::RenameSession,
        BindableAction::RestartSession,
        BindableAction::OpenInEditor,
        BindableAction::OpenPullRequest,
        BindableAction::GenerateSummary,
        BindableAction::RemoveProject,
    ] {
        assert!(
            !actions.contains(&hidden),
            "{hidden:?} should be hidden when nothing is selected"
        );
    }
}

#[test]
fn test_gather_command_entries_includes_actions_with_no_keybinding() {
    // ScrollUp/ScrollDown have no default binding (see KeyBindings::default()).
    // The palette is the primary access surface going forward, so commands
    // with no hotkey must still appear (with an empty `keys` field).
    let s = AppUiState::default();
    let kb = KeyBindings::default();
    let entries = s.gather_command_entries(&kb, "");

    let scroll_up = entries
        .iter()
        .find(|e| e.action == BindableAction::ScrollUp)
        .expect("ScrollUp should appear in the palette even without a keybinding");
    assert!(
        scroll_up.keys.is_empty(),
        "ScrollUp has no default binding, so `keys` must be empty"
    );

    let scroll_down = entries
        .iter()
        .find(|e| e.action == BindableAction::ScrollDown)
        .expect("ScrollDown should appear in the palette even without a keybinding");
    assert!(scroll_down.keys.is_empty());
}

#[test]
fn test_gather_command_entries_query_filters_by_label() {
    let s = ui_state_with(Some(SessionId::new()), None, RightPaneView::Info);
    let kb = KeyBindings::default();
    // "summary" matches only GenerateSummary (description "Generate AI summary")
    let entries = s.gather_command_entries(&kb, "summary");
    let actions: Vec<BindableAction> = entries.iter().map(|e| e.action).collect();
    assert_eq!(actions, vec![BindableAction::GenerateSummary]);
}

#[test]
fn test_gather_command_entries_case_insensitive() {
    let s = AppUiState::default();
    let kb = KeyBindings::default();
    let lower = s.gather_command_entries(&kb, "help");
    let upper = s.gather_command_entries(&kb, "HELP");
    let lower_actions: Vec<BindableAction> = lower.iter().map(|e| e.action).collect();
    let upper_actions: Vec<BindableAction> = upper.iter().map(|e| e.action).collect();
    assert_eq!(lower_actions, upper_actions);
    assert!(lower_actions.contains(&BindableAction::ShowHelp));
}

// ---------------------------------------------------------------------------
// Palette scroll: keep selection visible as the list exceeds max_visible
// ---------------------------------------------------------------------------

#[test]
fn test_adjust_list_scroll_noop_when_selection_in_window() {
    // selected in middle of window, scroll unchanged
    assert_eq!(adjust_list_scroll(5, 3, 10), 3);
    // selected at top of window, scroll unchanged
    assert_eq!(adjust_list_scroll(3, 3, 10), 3);
    // selected at bottom of window (scroll..scroll+visible is exclusive)
    assert_eq!(adjust_list_scroll(12, 3, 10), 3);
}

#[test]
fn test_adjust_list_scroll_pulls_up_when_selection_above() {
    // Pressing Up past the top of the window: scroll snaps to selected
    assert_eq!(adjust_list_scroll(2, 5, 10), 2);
    assert_eq!(adjust_list_scroll(0, 5, 10), 0);
}

#[test]
fn test_adjust_list_scroll_pushes_down_when_selection_below() {
    // Pressing Down off the bottom: scroll advances just enough to keep
    // the selection on the last visible row.
    assert_eq!(adjust_list_scroll(13, 3, 10), 4);
    assert_eq!(adjust_list_scroll(20, 0, 10), 11);
}

#[test]
fn test_adjust_list_scroll_wrap_up_from_top_lands_on_last_row() {
    // A 25-item list, currently at the top. Pressing Up wraps to index 24;
    // scroll must jump so 24 is visible.
    assert_eq!(adjust_list_scroll(24, 0, 10), 15);
}

#[test]
fn test_adjust_list_scroll_wrap_down_from_bottom_lands_on_first_row() {
    // 25-item list, selection on the last row; scrolled to the bottom.
    // Pressing Down wraps to 0, which is above the window — scroll to 0.
    assert_eq!(adjust_list_scroll(0, 15, 10), 0);
}

#[test]
fn test_adjust_list_scroll_zero_visible_rows_safe() {
    // Degenerate case: never panic.
    assert_eq!(adjust_list_scroll(5, 3, 0), 0);
}

#[test]
fn test_adjust_list_scroll_short_list_stays_at_top() {
    // When the list is shorter than the window, scroll should always be 0.
    // (The caller starts at 0; our function returns 0 because selected
    // is always in [0, visible).)
    assert_eq!(adjust_list_scroll(2, 0, 10), 0);
    assert_eq!(adjust_list_scroll(0, 0, 10), 0);
}

// ---------------------------------------------------------------------------
// ViewMode toggle
// ---------------------------------------------------------------------------

#[test]
fn test_view_mode_default_is_section_stacks() {
    let s = AppUiState::default();
    assert_eq!(s.view_mode, ViewMode::SectionStacks);
}

#[test]
fn test_toggle_view_mode_always_available_in_palette() {
    let s = AppUiState::default();
    assert!(s.is_command_available(BindableAction::ToggleViewMode));
}

#[test]
fn test_view_mode_cycles_through_three_views() {
    assert_eq!(ViewMode::ProjectGrouped.next(), ViewMode::SectionGrouped);
    assert_eq!(ViewMode::SectionGrouped.next(), ViewMode::SectionStacks);
    assert_eq!(ViewMode::SectionStacks.next(), ViewMode::ProjectGrouped);
}

#[test]
fn test_view_mode_heading_label() {
    assert_eq!(
        ViewMode::ProjectGrouped.heading_label(),
        " Sessions [Project]:"
    );
    assert_eq!(
        ViewMode::SectionGrouped.heading_label(),
        " Sessions [Sections]:"
    );
    assert_eq!(
        ViewMode::SectionStacks.heading_label(),
        " Sessions [Section Stacks]:"
    );
}

// ---------------------------------------------------------------------------
// Stack chain info in Info pane
// ---------------------------------------------------------------------------

#[test]
fn test_info_view_renders_stack_chain() {
    use crate::tui::widgets::{InfoContent, InfoSessionData, InfoView};

    let theme = crate::tui::theme::Theme::basic();
    let diff = crate::git::DiffInfo::empty();
    let chain = vec![
        StackChainEntry {
            title: "base".into(),
            status: SessionStatus::Running,
            is_current: false,
        },
        StackChainEntry {
            title: "child".into(),
            status: SessionStatus::Running,
            is_current: true,
        },
    ];
    let data = InfoSessionData {
        title: "child".into(),
        branch: "child-br".into(),
        created_at: "now".into(),
        status: SessionStatus::Running,
        program: "claude".into(),
        worktree_path: "/tmp".into(),
        diff_info: &diff,
        pr_number: None,
        pr_url: None,
        pr_merged: false,
        enriched_pr: None,
        ai_summary: None,
        summary_key_hint: None,
        stack_chain: &chain,
    };
    let view = InfoView::new(InfoContent::Session(data), &theme);
    let lines = view.build_lines();
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Stack (2 sessions)"),
        "should show stack header"
    );
    assert!(text.contains("base"), "should list base session");
    assert!(text.contains("← current"), "should mark current session");
}

#[test]
fn test_info_view_stack_section_renders_above_pr_section() {
    // The stack tells the user where this session sits in the PR graph,
    // which they typically scan before getting into PR-specific details
    // (state, labels, CI, body). Order in the rendered lines should be
    // Stack → PR, not the other way around.
    use crate::tui::widgets::{InfoContent, InfoSessionData, InfoView};

    let theme = crate::tui::theme::Theme::basic();
    let diff = crate::git::DiffInfo::empty();
    let chain = vec![
        StackChainEntry {
            title: "base".into(),
            status: SessionStatus::Running,
            is_current: false,
        },
        StackChainEntry {
            title: "child".into(),
            status: SessionStatus::Running,
            is_current: true,
        },
    ];
    let data = InfoSessionData {
        title: "child".into(),
        branch: "child-br".into(),
        created_at: "now".into(),
        status: SessionStatus::Running,
        program: "claude".into(),
        worktree_path: "/tmp".into(),
        diff_info: &diff,
        pr_number: Some(7),
        pr_url: Some("https://example.com/pr/7".into()),
        pr_merged: false,
        enriched_pr: None,
        ai_summary: None,
        summary_key_hint: None,
        stack_chain: &chain,
    };
    let view = InfoView::new(InfoContent::Session(data), &theme);
    let lines = view.build_lines();
    let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

    let stack_idx = text
        .iter()
        .position(|l| l.contains("Stack (2 sessions)"))
        .expect("stack header should be present");
    let pr_idx = text
        .iter()
        .position(|l| l.contains("PR #7"))
        .expect("PR header should be present");
    assert!(
        stack_idx < pr_idx,
        "stack section (line {stack_idx}) should appear before PR section (line {pr_idx})"
    );
}

#[test]
fn test_info_view_no_stack_section_for_unstacked() {
    use crate::tui::widgets::{InfoContent, InfoSessionData, InfoView};

    let theme = crate::tui::theme::Theme::basic();
    let diff = crate::git::DiffInfo::empty();
    let data = InfoSessionData {
        title: "solo".into(),
        branch: "solo-br".into(),
        created_at: "now".into(),
        status: SessionStatus::Running,
        program: "claude".into(),
        worktree_path: "/tmp".into(),
        diff_info: &diff,
        pr_number: None,
        pr_url: None,
        pr_merged: false,
        enriched_pr: None,
        ai_summary: None,
        summary_key_hint: None,
        stack_chain: &[],
    };
    let view = InfoView::new(InfoContent::Session(data), &theme);
    let lines = view.build_lines();
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !text.contains("Stack"),
        "unstacked session should not show stack section"
    );
}

// ---------------------------------------------------------------------------
// Settings: build_settings_rows + apply_settings_edit for worktrees_dir
// ---------------------------------------------------------------------------

use crate::config::{AppState, ConfigStore, StateStore};

fn make_test_app() -> App {
    make_test_app_with_path().0
}

#[test]
fn test_keybinding_rows_are_grouped_under_section_headers() {
    use crate::tui::app::SettingsRowKind;

    let app = make_test_app();
    let rows = app.build_settings_rows(SettingsTab::Keybindings);

    // The first row is a section header (not a blank spacer, not a binding).
    assert!(matches!(rows[0].kind, SettingsRowKind::Header));
    assert_eq!(rows[0].label, "Navigation");

    // One real (named) header per section, and one row per bindable action.
    let real_headers = rows
        .iter()
        .filter(|r| matches!(r.kind, SettingsRowKind::Header) && !r.label.is_empty())
        .count();
    let bindings = rows.iter().filter(|r| r.is_selectable()).count();
    assert_eq!(real_headers, app.config.keybindings.sections().len());
    assert_eq!(
        bindings,
        crate::config::keybindings::BindableAction::ALL.len()
    );

    // Every section after the first is preceded by exactly one blank spacer,
    // and no two named headers are adjacent.
    let spacers = rows
        .iter()
        .filter(|r| matches!(r.kind, SettingsRowKind::Header) && r.label.is_empty())
        .count();
    assert_eq!(spacers, real_headers - 1, "one blank line between sections");
    for pair in rows.windows(2) {
        let named =
            |r: &SettingsRow| matches!(r.kind, SettingsRowKind::Header) && !r.label.is_empty();
        assert!(
            !(named(&pair[0]) && named(&pair[1])),
            "empty section header rendered"
        );
    }
}

#[test]
fn test_general_rows_are_grouped_under_section_headers() {
    use crate::tui::app::SettingsRowKind;

    let app = make_test_app();
    let rows = app.build_settings_rows(SettingsTab::General);

    // The first row is a named section header, so the flat list is broken up.
    assert!(matches!(rows[0].kind, SettingsRowKind::Header));
    assert!(!rows[0].label.is_empty());

    // More than one section (i.e. it was actually split up), and every section
    // after the first is preceded by exactly one blank spacer.
    let named = |r: &SettingsRow| matches!(r.kind, SettingsRowKind::Header) && !r.label.is_empty();
    let named_headers = rows.iter().filter(|r| named(r)).count();
    assert!(
        named_headers > 1,
        "General tab should be split into sections"
    );
    let spacers = rows
        .iter()
        .filter(|r| matches!(r.kind, SettingsRowKind::Header) && r.label.is_empty())
        .count();
    assert_eq!(
        spacers,
        named_headers - 1,
        "one blank line between sections"
    );

    // No two named headers are adjacent (empty sections would render badly).
    for pair in rows.windows(2) {
        assert!(
            !(named(&pair[0]) && named(&pair[1])),
            "empty section header rendered"
        );
    }
}

#[test]
fn test_worktrees_dir_row_shows_default_when_none() {
    let app = make_test_app();
    let rows = app.build_settings_rows(SettingsTab::General);
    let row = rows
        .iter()
        .find(|r| r.field_key == "worktrees_dir")
        .unwrap();
    assert_eq!(row.text_value(), "(default)");
}

#[test]
fn test_worktrees_dir_row_shows_custom_path() {
    let mut app = make_test_app();
    app.config.worktrees_dir = Some(std::path::PathBuf::from("/custom/path"));
    let rows = app.build_settings_rows(SettingsTab::General);
    let row = rows
        .iter()
        .find(|r| r.field_key == "worktrees_dir")
        .unwrap();
    assert_eq!(row.text_value(), "/custom/path");
}

#[test]
fn test_apply_worktrees_dir_sets_custom_path() {
    let mut app = make_test_app();
    app.apply_settings_edit(SettingsTab::General, "worktrees_dir", "/my/worktrees");
    assert_eq!(
        app.config.worktrees_dir,
        Some(std::path::PathBuf::from("/my/worktrees"))
    );
}

#[test]
fn test_apply_worktrees_dir_empty_clears_to_none() {
    let mut app = make_test_app();
    app.config.worktrees_dir = Some(std::path::PathBuf::from("/custom"));
    app.apply_settings_edit(SettingsTab::General, "worktrees_dir", "");
    assert_eq!(app.config.worktrees_dir, None);
}

#[test]
fn test_apply_worktrees_dir_default_sentinel_clears_to_none() {
    let mut app = make_test_app();
    app.config.worktrees_dir = Some(std::path::PathBuf::from("/custom"));
    app.apply_settings_edit(SettingsTab::General, "worktrees_dir", "(default)");
    assert_eq!(app.config.worktrees_dir, None);
}

#[test]
fn test_commander_rows_present_with_defaults() {
    let app = make_test_app();
    let rows = app.build_settings_rows(SettingsTab::General);

    let kind_of = |key: &str| {
        rows.iter()
            .find(|r| r.field_key == key)
            .unwrap_or_else(|| panic!("missing row {key}"))
            .kind
            .clone()
    };

    // Disabled by default → toggle carrying false.
    assert_eq!(kind_of("commander_enabled"), SettingsRowKind::Toggle(false));
    // Program/dir fall back to "(default)" free-text when unset.
    assert_eq!(
        kind_of("commander_program"),
        SettingsRowKind::Text("(default)".to_string())
    );
    assert_eq!(
        kind_of("commander_dir"),
        SettingsRowKind::Text("(default)".to_string())
    );
}

#[test]
fn test_apply_commander_enabled_toggles_bool() {
    let mut app = make_test_app();
    app.apply_settings_edit(SettingsTab::General, "commander_enabled", "true");
    assert!(app.config.commander_enabled);
    app.apply_settings_edit(SettingsTab::General, "commander_enabled", "false");
    assert!(!app.config.commander_enabled);
}

#[test]
fn test_toggle_commander_enabled_via_bool_path() {
    // "Commander Enabled" is a Toggle row, so the in-app settings UI flips it
    // through `apply_bool_setting` (not `apply_settings_edit`, which only runs
    // for text/editing rows). This arm must exist or the toggle is a no-op.
    let mut app = make_test_app();
    assert!(!app.config.commander_enabled);
    app.apply_bool_setting("commander_enabled", true);
    assert!(app.config.commander_enabled);
    app.apply_bool_setting("commander_enabled", false);
    assert!(!app.config.commander_enabled);
}

#[test]
fn test_hide_empty_sections_toggle_and_apply() {
    let mut app = make_test_app();
    // Default is on.
    assert!(app.config.hide_empty_sections);
    // Row is present with correct default value.
    let rows = app.build_settings_rows(SettingsTab::General);
    let row = rows
        .iter()
        .find(|r| r.field_key == "hide_empty_sections")
        .unwrap_or_else(|| panic!("missing hide_empty_sections row"));
    assert_eq!(row.kind, SettingsRowKind::Toggle(true));
    // Apply false via bool path (what the toggle uses).
    app.apply_bool_setting("hide_empty_sections", false);
    assert!(!app.config.hide_empty_sections);
    // Flip back.
    app.apply_bool_setting("hide_empty_sections", true);
    assert!(app.config.hide_empty_sections);
}

#[test]
fn test_stt_rows_present_with_defaults() {
    let app = make_test_app();
    let rows = app.build_settings_rows(SettingsTab::Conversation);

    let kind_of = |key: &str| {
        rows.iter()
            .find(|r| r.field_key == key)
            .unwrap_or_else(|| panic!("missing row {key}"))
            .kind
            .clone()
    };

    assert_eq!(kind_of("stt_enabled"), SettingsRowKind::Toggle(false));
    assert_eq!(
        kind_of("stt_base_url"),
        SettingsRowKind::Text("http://127.0.0.1:8000/v1".to_string())
    );
    // Optional fields fall back to sentinel free-text when unset.
    assert_eq!(
        kind_of("stt_language"),
        SettingsRowKind::Text("(auto)".to_string())
    );
    assert_eq!(
        kind_of("stt_prompt"),
        SettingsRowKind::Text("(none)".to_string())
    );
    // Media pausing is on by default.
    assert_eq!(kind_of("stt_pause_media"), SettingsRowKind::Toggle(true));
}

#[test]
fn test_apply_stt_pause_media_toggle() {
    let mut app = make_test_app();
    assert!(app.config.stt.pause_media);
    app.apply_bool_setting("stt_pause_media", false);
    assert!(!app.config.stt.pause_media);
}

#[test]
fn test_apply_stt_text_fields() {
    let mut app = make_test_app();
    app.apply_settings_edit(
        SettingsTab::Conversation,
        "stt_base_url",
        "http://192.168.1.10:8080/v1",
    );
    app.apply_settings_edit(SettingsTab::Conversation, "stt_model", "large-v3-turbo");
    app.apply_settings_edit(SettingsTab::Conversation, "stt_language", "en");
    assert_eq!(app.config.stt.base_url, "http://192.168.1.10:8080/v1");
    assert_eq!(app.config.stt.model, "large-v3-turbo");
    assert_eq!(app.config.stt.language.as_deref(), Some("en"));

    // Sentinel / empty clears the optional fields back to None.
    app.apply_settings_edit(SettingsTab::Conversation, "stt_language", "(auto)");
    assert_eq!(app.config.stt.language, None);
    app.apply_settings_edit(SettingsTab::Conversation, "stt_prompt", "");
    assert_eq!(app.config.stt.prompt, None);
}

#[test]
fn test_toggle_stt_enabled_via_bool_path() {
    // "Enable Voice Input (STT)" is a Toggle row, flipped through
    // `apply_bool_setting`. This arm must exist or the toggle is a no-op.
    let mut app = make_test_app();
    assert!(!app.config.stt.enabled);
    app.apply_bool_setting("stt_enabled", true);
    assert!(app.config.stt.enabled);
    app.apply_bool_setting("stt_enabled", false);
    assert!(!app.config.stt.enabled);
}

#[test]
fn test_apply_commander_program_sets_and_clears() {
    let mut app = make_test_app();
    app.apply_settings_edit(
        SettingsTab::General,
        "commander_program",
        "claude --model opus",
    );
    assert_eq!(
        app.config.commander_program.as_deref(),
        Some("claude --model opus")
    );
    app.apply_settings_edit(SettingsTab::General, "commander_program", "");
    assert_eq!(app.config.commander_program, None);
}

#[test]
fn test_apply_commander_dir_sets_and_clears() {
    let mut app = make_test_app();
    app.apply_settings_edit(SettingsTab::General, "commander_dir", "/my/commander");
    assert_eq!(
        app.config.commander_dir,
        Some(std::path::PathBuf::from("/my/commander"))
    );
    app.apply_settings_edit(SettingsTab::General, "commander_dir", "(default)");
    assert_eq!(app.config.commander_dir, None);
}

#[tokio::test]
async fn open_commander_when_disabled_toasts_without_quitting() {
    // Default config has commander disabled, so `commander_enabled_at_init` is
    // false and the restart-required snapshot guard short-circuits before any
    // tmux work — this path is reachable in a unit test with no tmux server.
    let mut app = make_test_app();
    assert!(!app.config.commander_enabled);
    assert!(!app.commander_enabled_at_init);

    app.handle_open_commander().await;

    assert!(
        app.ui_state.status_message.is_some(),
        "disabled commander should surface a status message"
    );
    assert!(
        !app.ui_state.should_quit,
        "disabled commander must not quit the TUI to attach"
    );
    assert!(
        app.ui_state.attach_request.is_none(),
        "disabled commander must not queue an attach request"
    );
    assert!(
        !matches!(app.ui_state.modal, Modal::Error { .. }),
        "disabled commander is expected, not an error modal"
    );
}

#[test]
fn test_boolean_rows_are_toggle_kind() {
    let app = make_test_app();
    let rows = app.build_settings_rows(SettingsTab::General);

    let kind_of = |key: &str| {
        rows.iter()
            .find(|r| r.field_key == key)
            .unwrap_or_else(|| panic!("missing row {key}"))
            .kind
            .clone()
    };

    // Two-state booleans render as toggles carrying the live config value.
    assert_eq!(
        kind_of("fetch_before_create"),
        SettingsRowKind::Toggle(app.config.fetch_before_create)
    );
    assert_eq!(
        kind_of("rounded_borders"),
        SettingsRowKind::Toggle(app.config.rounded_borders)
    );
    // Tri-state and free-text fields stay on the text-input flow.
    assert!(matches!(kind_of("editor_gui"), SettingsRowKind::Text(_)));
    assert!(matches!(kind_of("branch_prefix"), SettingsRowKind::Text(_)));
}

#[test]
fn test_toggle_row_flips() {
    assert_eq!(SettingsRow::toggle("L", false, "k").toggled(), Some(true));
    assert_eq!(SettingsRow::toggle("L", true, "k").toggled(), Some(false));
    // Non-toggle rows have no toggled value.
    assert_eq!(SettingsRow::text("L", "v", "k").toggled(), None);
}

#[test]
fn test_apply_bool_setting_flips_config() {
    let mut app = make_test_app();
    app.config.fetch_before_create = false;
    app.apply_bool_setting("fetch_before_create", true);
    assert!(app.config.fetch_before_create);
    app.apply_bool_setting("fetch_before_create", false);
    assert!(!app.config.fetch_before_create);
}

#[tokio::test]
async fn ctrl_space_opens_quick_switch_in_tree() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = make_test_app();
    assert!(matches!(app.ui_state.modal, Modal::None));

    let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
    app.handle_input(InputEvent::Key(key)).await;

    assert!(
        matches!(
            app.ui_state.modal,
            Modal::QuickSwitch {
                mode: PaletteMode::Unified,
                ..
            }
        ),
        "Ctrl+Space should open the unified quick-switch palette, got {:?}",
        app.ui_state.modal
    );
}

#[test]
fn test_refilter_section_picker_keeps_section_rows() {
    use crate::session::SectionConfig;

    let mut app = make_test_app();
    app.config.sections = vec![
        SectionConfig {
            name: "Review".to_string(),
            ..Default::default()
        },
        SectionConfig {
            name: "Done".to_string(),
            ..Default::default()
        },
    ];
    let session_id = SessionId::new();
    app.ui_state.modal = Modal::QuickSwitch {
        mode: PaletteMode::SectionPicker { session_id },
        query: "re".into(),
        matches: Vec::new(),
        selected_idx: 0,
        scroll: 0,
    };

    app.refilter_quick_switch();

    let Modal::QuickSwitch { matches, .. } = &app.ui_state.modal else {
        panic!("modal should still be QuickSwitch");
    };
    assert!(
        matches
            .iter()
            .all(|m| matches!(m, QuickSwitchItem::SectionMove { .. })),
        "section picker must only show section rows after typing, got {matches:?}"
    );
    assert!(
        matches
            .iter()
            .any(|m| matches!(m, QuickSwitchItem::SectionMove { label, .. } if label == "Review")),
        "query 're' should match the Review section, got {matches:?}"
    );
}

#[test]
fn test_project_pull_rows_present_in_general_tab() {
    let app = make_test_app();
    let rows = app.build_settings_rows(SettingsTab::General);
    let enabled = rows
        .iter()
        .find(|r| r.field_key == "project_pull_enabled")
        .expect("project_pull_enabled row missing");
    assert_eq!(enabled.kind, SettingsRowKind::Toggle(true));
    let interval = rows
        .iter()
        .find(|r| r.field_key == "project_pull_interval_secs")
        .expect("project_pull_interval_secs row missing");
    assert_eq!(interval.text_value(), "3600");
}

#[test]
fn test_nix_develop_row_present_in_general_tab() {
    let app = make_test_app();
    let rows = app.build_settings_rows(SettingsTab::General);
    let row = rows
        .iter()
        .find(|r| r.field_key == "nix_develop")
        .expect("nix_develop row missing");
    assert_eq!(row.kind, SettingsRowKind::Toggle(true));
}

#[test]
fn test_apply_nix_develop_round_trip() {
    let mut app = make_test_app();
    app.apply_bool_setting("nix_develop", false);
    assert!(!app.config.nix_develop);
    app.apply_bool_setting("nix_develop", true);
    assert!(app.config.nix_develop);
}

#[test]
fn test_apply_project_pull_enabled_round_trip() {
    let mut app = make_test_app();
    app.apply_bool_setting("project_pull_enabled", true);
    assert!(app.config.project_pull_enabled);
    app.apply_bool_setting("project_pull_enabled", false);
    assert!(!app.config.project_pull_enabled);
}

#[test]
fn test_apply_project_pull_interval_accepts_60_and_above() {
    let mut app = make_test_app();
    app.apply_settings_edit(SettingsTab::General, "project_pull_interval_secs", "120");
    assert_eq!(app.config.project_pull_interval_secs, 120);
    app.apply_settings_edit(SettingsTab::General, "project_pull_interval_secs", "60");
    assert_eq!(app.config.project_pull_interval_secs, 60);
}

#[test]
fn test_apply_project_pull_interval_rejects_below_60() {
    let mut app = make_test_app();
    app.config.project_pull_interval_secs = 3600;
    app.apply_settings_edit(SettingsTab::General, "project_pull_interval_secs", "30");
    assert_eq!(
        app.config.project_pull_interval_secs, 3600,
        "values below 60 must be rejected"
    );
    assert!(
        app.ui_state.status_message.is_some(),
        "rejection should surface a status message"
    );
}

#[test]
fn test_apply_ui_refresh_fps_accepts_positive_values() {
    let mut app = make_test_app();
    app.apply_settings_edit(SettingsTab::General, "ui_refresh_fps", "60");
    assert_eq!(app.config.ui_refresh_fps, 60);
    app.apply_settings_edit(SettingsTab::General, "ui_refresh_fps", "1");
    assert_eq!(app.config.ui_refresh_fps, 1);
}

#[test]
fn test_apply_ui_refresh_fps_rejects_zero() {
    // Regression: a persisted fps of 0 divides by zero computing the tick
    // rate at next launch, crash-looping until config.toml is hand-edited.
    let mut app = make_test_app();
    app.config.ui_refresh_fps = 30;
    app.apply_settings_edit(SettingsTab::General, "ui_refresh_fps", "0");
    assert_eq!(app.config.ui_refresh_fps, 30, "zero fps must be rejected");
    assert!(
        app.ui_state.status_message.is_some(),
        "rejection should surface a status message"
    );
}

#[test]
fn test_apply_max_concurrent_tmux_accepts_positive_values() {
    let mut app = make_test_app();
    app.apply_settings_edit(SettingsTab::General, "max_concurrent_tmux", "8");
    assert_eq!(app.config.max_concurrent_tmux, 8);
}

#[test]
fn test_apply_max_concurrent_tmux_rejects_zero() {
    // Regression: a persisted value of 0 becomes Semaphore::new(0) at next
    // launch, deadlocking every tmux command.
    let mut app = make_test_app();
    app.config.max_concurrent_tmux = 16;
    app.apply_settings_edit(SettingsTab::General, "max_concurrent_tmux", "0");
    assert_eq!(
        app.config.max_concurrent_tmux, 16,
        "zero concurrency must be rejected"
    );
    assert!(
        app.ui_state.status_message.is_some(),
        "rejection should surface a status message"
    );
}

#[tokio::test]
async fn apply_section_move_keeps_moved_session_selected() {
    use crate::session::{Project, SectionConfig, WorktreeSession};
    use std::path::PathBuf;

    let mut app = make_test_app();
    // A manual-only section the session can be moved into.
    app.config.sections = vec![SectionConfig {
        name: "Beta".to_string(),
        ..Default::default()
    }];
    app.ui_state.view_mode = ViewMode::SectionGrouped;

    let project = Project::new("proj", PathBuf::from("/tmp/proj"), "main");
    let project_id = project.id;
    let s1 = WorktreeSession::new(
        project_id,
        "one",
        "br-one",
        PathBuf::from("/tmp/w1"),
        "claude",
    );
    let s2 = WorktreeSession::new(
        project_id,
        "two",
        "br-two",
        PathBuf::from("/tmp/w2"),
        "claude",
    );
    let s2_id = s2.id;

    app.service
        .store()
        .mutate(move |state| {
            state.add_project(project);
            state.add_session(s1);
            state.add_session(s2);
        })
        .await
        .unwrap();

    app.sync_local_view_from_store_for_test().await;
    app.refresh_list_items().await;

    // Move session two into "Beta" — it was in the "In Progress" catch-all.
    // The move runs on a background task that posts `SessionMutationApplied`
    // once the store is updated; drive that event to apply the view/tree refresh
    // and reselection (as the event loop would).
    app.apply_section_move(s2_id, Some("Beta".to_string()));
    loop {
        match app.event_loop.next().await.expect("a completion event") {
            AppEvent::StateUpdate(su @ StateUpdate::SessionMutationApplied { .. }) => {
                app.handle_state_update(su).await;
                break;
            }
            _ => continue,
        }
    }

    let selected_idx = app
        .ui_state
        .list_state
        .selected()
        .expect("a list item should be selected after the move");
    let selected_item = &app.ui_state.list_items[selected_idx];
    assert!(
        matches!(selected_item, SessionListItem::Worktree { id, .. } if *id == s2_id),
        "the moved session should remain selected, got {selected_item:?}"
    );
    assert_eq!(
        app.ui_state.selected_session_id.map(|r| r.id),
        Some(s2_id),
        "selected_session_id should still track the moved session"
    );
}

// ---------------------------------------------------------------------------
// List-modal mouse support: geometry, row mapping, click state machine
// ---------------------------------------------------------------------------

use super::modals::{checkout_branch_areas, path_input_areas, quick_switch_areas};

#[test]
fn quick_switch_rows_area_sits_below_input_line() {
    let area = Rect::new(0, 0, 100, 50);
    let (modal, rows) = quick_switch_areas(area, 5);
    // border(2) + input(1) + 5 rows
    assert_eq!(modal.height, 8);
    assert_eq!(rows.x, modal.x + 1);
    assert_eq!(rows.width, modal.width - 2);
    assert_eq!(rows.y, modal.y + 2); // border + input line
    assert_eq!(rows.height, 5);
}

#[test]
fn quick_switch_rows_capped_at_list_max_visible() {
    let (_, rows) = quick_switch_areas(Rect::new(0, 0, 100, 50), 100);
    assert_eq!(rows.height, super::actions::LIST_MAX_VISIBLE as u16);
}

#[test]
fn quick_switch_rows_empty_when_no_matches() {
    let (_, rows) = quick_switch_areas(Rect::new(0, 0, 100, 50), 0);
    assert_eq!(rows.height, 0);
}

#[test]
fn checkout_branch_rows_area_sits_below_input_and_hint() {
    let area = Rect::new(0, 0, 100, 60);
    let (modal, rows) = checkout_branch_areas(area, 3);
    // border(2) + input(1) + hint(1) + 3 rows
    assert_eq!(modal.height, 7);
    assert_eq!(rows.y, modal.y + 3);
    assert_eq!(rows.height, 3);
}

#[test]
fn path_input_rows_area_uses_fixed_window() {
    let area = Rect::new(0, 0, 100, 50);
    let (modal, rows) = path_input_areas(area);
    assert_eq!(modal.height, 16);
    assert_eq!(rows.y, modal.y + 4); // border + 3-row prompt/input block
    assert_eq!(rows.height, super::actions::LIST_MAX_VISIBLE as u16);
}

/// App with an open section-picker palette of `n_items` rows and the rows
/// area recorded as the renderer would have left it (rows at y=12..12+n).
fn make_section_picker_app(n_items: usize) -> App {
    let mut app = make_test_app();
    let session_id = SessionId::new();
    let matches = (0..n_items)
        .map(|i| QuickSwitchItem::SectionMove {
            session_id,
            target: Some(format!("S{i}")),
            label: format!("S{i}"),
        })
        .collect();
    app.ui_state.modal = Modal::QuickSwitch {
        mode: PaletteMode::SectionPicker { session_id },
        query: super::Input::default(),
        matches,
        selected_idx: 0,
        scroll: 0,
    };
    app.ui_state.modal_list_rect = Some(Rect::new(20, 12, 40, n_items as u16));
    app
}

#[tokio::test]
async fn modal_single_click_highlights_row_without_activating() {
    let mut app = make_section_picker_app(3);
    app.handle_modal_list_click(20, 13).await; // second row
    match &app.ui_state.modal {
        Modal::QuickSwitch { selected_idx, .. } => assert_eq!(*selected_idx, 1),
        other => panic!("modal should stay open, got {other:?}"),
    }
}

#[tokio::test]
async fn modal_double_click_same_row_activates() {
    let mut app = make_section_picker_app(3);
    app.handle_modal_list_click(20, 13).await;
    app.handle_modal_list_click(25, 13).await; // same row, different column
    assert!(
        matches!(app.ui_state.modal, Modal::None),
        "double-click should activate the row and close the modal"
    );
}

#[tokio::test]
async fn modal_clicks_on_different_rows_do_not_activate() {
    let mut app = make_section_picker_app(3);
    app.handle_modal_list_click(20, 12).await;
    app.handle_modal_list_click(20, 13).await;
    assert!(matches!(app.ui_state.modal, Modal::QuickSwitch { .. }));
}

#[tokio::test]
async fn modal_keystroke_between_clicks_resets_double_click() {
    // A keystroke can refilter the list, so the second click must count as
    // a fresh first click rather than activating a possibly-shifted row.
    let mut app = make_section_picker_app(3);
    app.handle_modal_list_click(20, 13).await;
    let down = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Down,
        crossterm::event::KeyModifiers::NONE,
    );
    app.handle_modal_key(down).await;
    app.handle_modal_list_click(20, 13).await;
    assert!(
        matches!(app.ui_state.modal, Modal::QuickSwitch { .. }),
        "second click after a keystroke must not activate"
    );
}

#[tokio::test]
async fn modal_click_outside_rows_leaves_selection_alone() {
    let mut app = make_section_picker_app(3);
    app.handle_modal_list_click(20, 11).await; // input line above the rows
    match &app.ui_state.modal {
        Modal::QuickSwitch { selected_idx, .. } => assert_eq!(*selected_idx, 0),
        other => panic!("modal should stay open, got {other:?}"),
    }
}

// -- shared single-line text-input helpers (back the modal input boxes) --

fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
}

#[test]
fn input_with_caret_marks_cursor_position() {
    use crossterm::event::KeyCode;
    let mut input = Input::from("ab");
    // Cursor starts at the end → caret appended.
    assert_eq!(input_with_caret(&input), "ab▏");
    // Move left once → caret sits before the last char.
    input.handle(tui_input::InputRequest::GoToPrevChar);
    assert_eq!(input_with_caret(&input), "a▏b");
    // Sanity: KeyCode plumbing builds a Left arrow.
    assert!(matches!(key(KeyCode::Left).code, KeyCode::Left));
}

#[test]
fn edit_text_input_inserts_at_cursor_and_reports_change() {
    use crossterm::event::KeyCode;
    let mut input = Input::from("ac");
    input.handle(tui_input::InputRequest::GoToPrevChar); // cursor between a|c

    // Typing inserts at the cursor and signals the value changed.
    assert!(edit_text_input(&mut input, key(KeyCode::Char('b'))));
    assert_eq!(input.value(), "abc");

    // A cursor move changes no text → returns false (callers skip refiltering).
    assert!(!edit_text_input(&mut input, key(KeyCode::Home)));
    assert_eq!(input.value(), "abc");
    assert_eq!(input.cursor(), 0);

    // Delete-forward at the start removes the first char.
    assert!(edit_text_input(&mut input, key(KeyCode::Delete)));
    assert_eq!(input.value(), "bc");

    // A non-editing key (Enter) is left for the caller: no change, returns false.
    assert!(!edit_text_input(&mut input, key(KeyCode::Enter)));
    assert_eq!(input.value(), "bc");
}

// ---------------------------------------------------------------------------
// View-switch clearing happens in render(), not via terminal.clear()
// ---------------------------------------------------------------------------

/// Flatten a `TestBackend` buffer into one string (rows joined by spaces) so
/// tests can assert that expected text was drawn somewhere on screen.
fn buffer_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect::<Vec<_>>()
        .join("")
}

fn keybindings_settings_state(app: &App, search: Option<&str>) -> crate::tui::app::SettingsState {
    use crate::tui::app::{ProgramsState, SectionsState, SettingsState, SettingsTab};
    let rows = app.build_settings_rows(SettingsTab::Keybindings);
    SettingsState {
        tab: SettingsTab::Keybindings,
        selected_row: 1,
        editing: None,
        rows,
        sections_state: SectionsState::default(),
        programs_state: ProgramsState::default(),
        search: search.map(|q| q.into()),
    }
}

#[test]
fn render_keybindings_tab_draws_section_headers() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    app.ui_state.modal = Modal::Settings(keybindings_settings_state(&app, None));

    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let text = buffer_text(&terminal);
    // Section headers are drawn, and a representative binding under them.
    assert!(text.contains("Navigation"), "missing Navigation header");
    assert!(text.contains("Sessions"), "missing Sessions header");
    assert!(text.contains("Attach to selected session"));
    // Footer advertises the search shortcut on this tab.
    assert!(text.contains("/: search"));
}

#[test]
fn render_general_tab_draws_section_headers() {
    use crate::tui::app::{Modal, SettingsState, SettingsTab};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    let rows = app.build_settings_rows(SettingsTab::General);
    let selected_row = super::settings::first_selectable_from(&rows, 0);
    app.ui_state.modal = Modal::Settings(SettingsState {
        tab: SettingsTab::General,
        selected_row,
        editing: None,
        rows,
        sections_state: Default::default(),
        programs_state: Default::default(),
        search: None,
    });

    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let text = buffer_text(&terminal);
    // Representative section headers are drawn alongside their settings.
    assert!(
        text.contains("Sessions & Worktrees"),
        "missing Sessions header"
    );
    assert!(text.contains("Editor"), "missing Editor header");
    assert!(text.contains("Appearance"), "missing Appearance header");
    assert!(text.contains("Branch Prefix"));
}

#[test]
fn render_keybindings_search_box_filters_and_shows_prompt() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    // A search matching only "commander" should hide unrelated bindings.
    let rows = super::settings::filter_keybinding_rows(
        app.build_settings_rows(crate::tui::app::SettingsTab::Keybindings),
        "commander",
    );
    let mut state = keybindings_settings_state(&app, Some("commander"));
    state.rows = rows;
    state.selected_row = 1;
    app.ui_state.modal = Modal::Settings(state);

    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let text = buffer_text(&terminal);
    // The search prompt line is visible…
    assert!(text.contains("/commander"), "search prompt not rendered");
    // …the matching binding is shown…
    assert!(text.contains("Open commander session"));
    // …and an unrelated binding is filtered out.
    assert!(
        !text.contains("Scroll up"),
        "unrelated binding not filtered"
    );
}

#[test]
fn render_consumes_clear_right_pane_flag() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    app.ui_state.clear_right_pane = true;

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    // render() must consume the flag itself by drawing the `Clear` widget.
    // Before the fix the flag was cleared by a `terminal.clear()` call in the
    // event loop, which since ratatui 0.30 reads the cursor from stdin — a
    // blocking read that races the background input reader and crashes the
    // loop. Drawing through ratatui here performs no cursor read.
    terminal.draw(|f| app.render(f)).unwrap();

    assert!(
        !app.ui_state.clear_right_pane,
        "render() should consume clear_right_pane so the event loop never needs terminal.clear()"
    );
}

// ---------------------------------------------------------------------------
// Programs settings tab
// ---------------------------------------------------------------------------

fn program_entry(label: &str, command: &str) -> crate::config::ProgramEntry {
    crate::config::ProgramEntry {
        label: label.to_string(),
        command: command.to_string(),
    }
}

/// Borrow the Programs-tab state out of the current settings modal.
fn peek_programs(app: &App) -> &crate::tui::app::ProgramsState {
    match &app.ui_state.modal {
        Modal::Settings(s) => &s.programs_state,
        _ => panic!("expected a settings modal"),
    }
}

/// Feed one keypress into the Programs tab, keeping the modal in place.
async fn feed_programs_key(app: &mut App, code: crossterm::event::KeyCode) {
    let state = match std::mem::replace(&mut app.ui_state.modal, Modal::None) {
        Modal::Settings(s) => s,
        other => {
            app.ui_state.modal = other;
            panic!("expected a settings modal");
        }
    };
    app.handle_settings_key(key(code), state).await;
}

async fn type_programs(app: &mut App, text: &str) {
    for c in text.chars() {
        feed_programs_key(app, crossterm::event::KeyCode::Char(c)).await;
    }
}

#[tokio::test]
async fn programs_tab_new_adds_entry_immediately_and_edits_in_place() {
    use crate::tui::app::ProgramsEditing;
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // `n` adds a real, visible entry straight away (the bug fix) with a unique
    // default label + runnable command, committed to the target immediately, and
    // starts editing its label.
    feed_programs_key(&mut app, KeyCode::Char('n')).await;
    assert_eq!(
        peek_programs(&app).entries,
        vec![program_entry("New program", "claude")],
        "new entry appears in the working copy immediately"
    );
    assert_eq!(
        app.config.programs,
        vec![program_entry("New program", "claude")],
        "and is committed to the local config"
    );
    assert_eq!(peek_programs(&app).selected, 0);
    assert!(matches!(
        peek_programs(&app).editing,
        Some(ProgramsEditing::CreatingLabel { .. })
    ));

    // Type a label; Enter applies it to the live entry and advances to the
    // command step, seeded from the (new) label.
    type_programs(&mut app, "Codex").await;
    feed_programs_key(&mut app, KeyCode::Enter).await;
    assert_eq!(
        peek_programs(&app).entries[0].label,
        "Codex",
        "label applied to the live entry"
    );
    match &peek_programs(&app).editing {
        Some(ProgramsEditing::CreatingCommand { value }) => {
            assert_eq!(value.value(), "Codex", "command seeded from the label");
        }
        other => panic!("expected CreatingCommand, got {other:?}"),
    }

    // Enter finishes editing, using the seeded command.
    feed_programs_key(&mut app, KeyCode::Enter).await;
    assert_eq!(app.config.programs, vec![program_entry("Codex", "Codex")]);
    let prog = peek_programs(&app);
    assert_eq!(prog.selected, 0);
    assert!(prog.editing.is_none());
}

#[tokio::test]
async fn programs_tab_create_esc_keeps_the_added_entry() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Back out at the label step: the entry stays with its default label/command
    // and must be deleted explicitly (the requested behaviour).
    feed_programs_key(&mut app, KeyCode::Char('n')).await;
    feed_programs_key(&mut app, KeyCode::Esc).await;
    assert_eq!(
        app.config.programs,
        vec![program_entry("New program", "claude")]
    );
    assert!(peek_programs(&app).editing.is_none());

    // Back out at the command step: still kept (label applied, command default).
    feed_programs_key(&mut app, KeyCode::Char('n')).await;
    type_programs(&mut app, "Codex").await;
    feed_programs_key(&mut app, KeyCode::Enter).await; // advance to command step
    feed_programs_key(&mut app, KeyCode::Esc).await; // back out
    assert_eq!(app.config.programs.len(), 2);
    assert_eq!(app.config.programs[1], program_entry("Codex", "claude"));
    assert!(peek_programs(&app).editing.is_none());
}

#[tokio::test]
async fn programs_tab_rename_rejects_duplicate_and_empty_labels() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.config.programs = vec![
        program_entry("Claude", "claude"),
        program_entry("Codex", "codex"),
    ];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Move to the second entry and rename it to a duplicate of the first.
    feed_programs_key(&mut app, KeyCode::Char('j')).await;
    assert_eq!(peek_programs(&app).selected, 1);
    feed_programs_key(&mut app, KeyCode::Char('r')).await;
    for _ in 0.."Codex".len() {
        feed_programs_key(&mut app, KeyCode::Backspace).await;
    }
    type_programs(&mut app, "Claude").await;
    feed_programs_key(&mut app, KeyCode::Enter).await;
    assert_eq!(app.config.programs[1].label, "Codex", "duplicate rejected");

    // Renaming to empty is likewise rejected.
    feed_programs_key(&mut app, KeyCode::Char('r')).await;
    for _ in 0.."Codex".len() {
        feed_programs_key(&mut app, KeyCode::Backspace).await;
    }
    feed_programs_key(&mut app, KeyCode::Enter).await;
    assert_eq!(app.config.programs[1].label, "Codex", "empty rejected");
}

#[tokio::test]
async fn programs_tab_fields_focus_edits_command() {
    use crate::tui::app::ProgramsFocus;
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.config.programs = vec![program_entry("Claude", "claude")];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Enter the fields pane, move to the command field, edit it.
    feed_programs_key(&mut app, KeyCode::Enter).await;
    assert_eq!(peek_programs(&app).focus, ProgramsFocus::Fields);
    feed_programs_key(&mut app, KeyCode::Char('j')).await; // toggle to command field
    assert_eq!(peek_programs(&app).field_selected, 1);
    feed_programs_key(&mut app, KeyCode::Enter).await; // start editing command
    for _ in 0.."claude".len() {
        feed_programs_key(&mut app, KeyCode::Backspace).await;
    }
    type_programs(&mut app, "claude --model opus").await;
    feed_programs_key(&mut app, KeyCode::Enter).await;

    assert_eq!(app.config.programs[0].command, "claude --model opus");
    assert_eq!(app.config.programs[0].label, "Claude", "label untouched");
}

#[tokio::test]
async fn programs_tab_delete_clamps_selection_and_allows_empty() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.config.programs = vec![program_entry("Alpha", "a"), program_entry("Beta", "b")];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Select the last entry, then delete it: selection clamps back.
    feed_programs_key(&mut app, KeyCode::Char('j')).await;
    assert_eq!(peek_programs(&app).selected, 1);
    feed_programs_key(&mut app, KeyCode::Char('d')).await;
    assert_eq!(app.config.programs, vec![program_entry("Alpha", "a")]);
    assert_eq!(peek_programs(&app).selected, 0, "selection clamped");

    // Deleting the final entry is allowed — the list may be empty.
    feed_programs_key(&mut app, KeyCode::Char('d')).await;
    assert!(app.config.programs.is_empty());

    // An empty list still yields the built-in `claude` choice.
    let choices = app.config.program_choices();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].command, "claude");
}

#[tokio::test]
async fn programs_tab_reorder_changes_default() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.config.programs = vec![
        program_entry("Claude", "claude"),
        program_entry("Codex", "codex"),
    ];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);
    assert_eq!(app.config.default_session_program(), "claude");

    // `J` swaps the first entry down, making the second the new default.
    feed_programs_key(&mut app, KeyCode::Char('J')).await;
    assert_eq!(app.config.programs[0].label, "Codex");
    assert_eq!(app.config.default_session_program(), "codex");
    assert_eq!(
        peek_programs(&app).selected,
        1,
        "selection follows the move"
    );
}

#[tokio::test]
async fn programs_tab_tab_key_switches_tabs() {
    use crate::tui::app::{Modal, SettingsTab};
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Tab advances to the wrapped-around General tab.
    feed_programs_key(&mut app, KeyCode::Tab).await;
    match &app.ui_state.modal {
        Modal::Settings(s) => assert_eq!(s.tab, SettingsTab::General),
        _ => panic!("expected a settings modal"),
    }

    // BackTab from Programs lands on Sections.
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);
    feed_programs_key(&mut app, KeyCode::BackTab).await;
    match &app.ui_state.modal {
        Modal::Settings(s) => assert_eq!(s.tab, SettingsTab::Sections),
        _ => panic!("expected a settings modal"),
    }
}

#[test]
fn render_programs_tab_shows_entries_and_default_marker() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    app.config.programs = vec![
        program_entry("Claude", "claude"),
        program_entry("Codex", "codex"),
    ];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let text = buffer_text(&terminal);
    assert!(text.contains("Claude"), "missing first program label");
    assert!(text.contains("Codex"), "missing second program label");
    assert!(text.contains("(default)"), "missing default marker");
    assert!(text.contains("n: new"), "missing list footer hint");
}

#[tokio::test]
async fn render_programs_tab_shows_new_entry_while_naming_it() {
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    app.config.programs = vec![program_entry("Claude", "claude")];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // `n` adds the entry; while it is being named it must still render alongside
    // the existing entries (the bug: it used to be invisible until saved).
    feed_programs_key(&mut app, KeyCode::Char('n')).await;
    type_programs(&mut app, "Cod").await;

    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(&terminal);
    assert!(
        text.contains("Claude"),
        "existing entry still shown while naming"
    );
    assert!(
        text.contains("Cod"),
        "the new entry's live label input is shown in the list"
    );
    assert!(
        text.contains("next (command)"),
        "footer reflects the label-naming step"
    );
}

/// Like `make_test_app`, but returns the on-disk config path so tests can read
/// the persisted config back and pin that a mutation actually wrote through the
/// store (not merely the in-memory `app.config`).
fn make_test_app_with_path() -> (App, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    let state_path = tmp.path().join("state.json");
    let config = Config::default();
    let config_store = Arc::new(ConfigStore::with_path(config, config_path.clone()));
    let store = Arc::new(StateStore::with_path(AppState::new(), state_path));
    // Leak the TempDir so paths stay valid for the lifetime of the test.
    std::mem::forget(tmp);
    let app = App::new(
        config_store,
        store,
        crate::telemetry::FrontendInfo::new("test", "0.0.0"),
        crate::backend::no_remote_backends(),
    );
    (app, config_path)
}

#[tokio::test]
async fn programs_tab_reorder_persists_through_store() {
    use crossterm::event::KeyCode;

    let (mut app, path) = make_test_app_with_path();
    app.config.programs = vec![
        program_entry("Claude", "claude"),
        program_entry("Codex", "codex"),
    ];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    feed_programs_key(&mut app, KeyCode::Char('J')).await;

    // Read the config back from disk: the reorder must have been written
    // through the store (this would fail if the `J` arm dropped persist_config).
    let persisted = Config::load_from_path(&path).unwrap();
    assert_eq!(
        persisted.programs,
        vec![
            program_entry("Codex", "codex"),
            program_entry("Claude", "claude"),
        ]
    );
}

#[tokio::test]
async fn programs_tab_new_entry_persists_through_store() {
    use crossterm::event::KeyCode;

    let (mut app, path) = make_test_app_with_path();
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // `n` must write the freshly-added entry through the store immediately, so a
    // user who backs out (never reaching the final Enter) still finds it saved.
    feed_programs_key(&mut app, KeyCode::Char('n')).await;
    let persisted = Config::load_from_path(&path).unwrap();
    assert_eq!(
        persisted.programs,
        vec![program_entry("New program", "claude")]
    );
}

#[tokio::test]
async fn programs_tab_editing_field_label_rejects_duplicate() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.config.programs = vec![
        program_entry("Claude", "claude"),
        program_entry("Codex", "codex"),
    ];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Focus the fields pane on the second entry and edit its label to a dup.
    feed_programs_key(&mut app, KeyCode::Char('j')).await;
    feed_programs_key(&mut app, KeyCode::Enter).await; // into Fields (label field)
    assert_eq!(peek_programs(&app).field_selected, 0);
    feed_programs_key(&mut app, KeyCode::Enter).await; // start editing label
    for _ in 0.."Codex".len() {
        feed_programs_key(&mut app, KeyCode::Backspace).await;
    }
    type_programs(&mut app, "Claude").await;
    feed_programs_key(&mut app, KeyCode::Enter).await;

    assert_eq!(app.config.programs[1].label, "Codex", "duplicate rejected");
}

#[tokio::test]
async fn programs_tab_esc_cancels_rename_and_edit_without_change() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.config.programs = vec![program_entry("Claude", "claude")];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Rename: type a new label then Esc — nothing changes.
    feed_programs_key(&mut app, KeyCode::Char('r')).await;
    type_programs(&mut app, "XYZ").await;
    feed_programs_key(&mut app, KeyCode::Esc).await;
    assert_eq!(app.config.programs[0].label, "Claude");
    assert!(peek_programs(&app).editing.is_none());

    // Edit command field: type then Esc — nothing changes.
    feed_programs_key(&mut app, KeyCode::Enter).await; // Fields
    feed_programs_key(&mut app, KeyCode::Char('j')).await; // command field
    feed_programs_key(&mut app, KeyCode::Enter).await; // start editing
    type_programs(&mut app, "-zzz").await;
    feed_programs_key(&mut app, KeyCode::Esc).await;
    assert_eq!(app.config.programs[0].command, "claude");
    assert!(peek_programs(&app).editing.is_none());
}

#[tokio::test]
async fn programs_tab_create_label_empty_or_duplicate_keeps_default() {
    use crate::tui::app::ProgramsEditing;
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.config.programs = vec![program_entry("Claude", "claude")];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Empty label at the create step keeps the auto-generated default and
    // advances to the command step (the entry was already added on `n`).
    feed_programs_key(&mut app, KeyCode::Char('n')).await;
    feed_programs_key(&mut app, KeyCode::Enter).await;
    assert_eq!(app.config.programs.len(), 2);
    assert_eq!(app.config.programs[1].label, "New program");
    assert!(matches!(
        peek_programs(&app).editing,
        Some(ProgramsEditing::CreatingCommand { .. })
    ));
    feed_programs_key(&mut app, KeyCode::Enter).await; // finish

    // A duplicate label is rejected, so the entry keeps its unique default.
    feed_programs_key(&mut app, KeyCode::Char('n')).await;
    let default_label = peek_programs(&app).entries[2].label.clone();
    assert_ne!(default_label, "New program", "second default is distinct");
    type_programs(&mut app, "Claude").await; // duplicate of the first entry
    feed_programs_key(&mut app, KeyCode::Enter).await;
    assert_eq!(app.config.programs.len(), 3);
    assert_eq!(
        app.config.programs[2].label, default_label,
        "duplicate rejected; default kept"
    );
    assert!(matches!(
        peek_programs(&app).editing,
        Some(ProgramsEditing::CreatingCommand { .. })
    ));
}

#[tokio::test]
async fn programs_tab_reorder_up_with_k() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.config.programs = vec![
        program_entry("Claude", "claude"),
        program_entry("Codex", "codex"),
    ];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Select the second entry, then `K` moves it up to become the default.
    feed_programs_key(&mut app, KeyCode::Char('j')).await;
    feed_programs_key(&mut app, KeyCode::Char('K')).await;
    assert_eq!(app.config.programs[0].label, "Codex");
    assert_eq!(app.config.default_session_program(), "codex");
    assert_eq!(
        peek_programs(&app).selected,
        0,
        "selection follows the move"
    );
}

#[tokio::test]
async fn open_settings_on_programs_targets_local_and_loads_entries() {
    use crate::tui::app::{Modal, SettingsTab};

    let mut app = make_test_app();
    app.config.programs = vec![program_entry("Claude", "claude")];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    match &app.ui_state.modal {
        Modal::Settings(s) => {
            assert_eq!(s.tab, SettingsTab::Programs);
            assert_eq!(s.programs_state.target, crate::backend::LOCAL_BACKEND_ID);
            // Local target loads synchronously from config — no loading state.
            assert!(!s.programs_state.loading);
            assert_eq!(
                s.programs_state.entries,
                vec![program_entry("Claude", "claude")]
            );
        }
        _ => panic!("expected a settings modal on the Programs tab"),
    }
}

#[tokio::test]
async fn programs_tab_blocks_editing_while_loading() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);
    // Simulate an in-flight remote fetch.
    if let crate::tui::app::Modal::Settings(s) = &mut app.ui_state.modal {
        s.programs_state.target = crate::backend::BackendId(1);
        s.programs_state.loading = true;
    }

    // `n` (new) must be ignored while loading — no editor opens.
    feed_programs_key(&mut app, KeyCode::Char('n')).await;
    assert!(peek_programs(&app).editing.is_none());
}

#[tokio::test]
async fn server_programs_loaded_applies_for_matching_target_and_gen() {
    use crate::tui::event::StateUpdate;

    let mut app = make_test_app();
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);
    let (target, generation) = {
        let s = match &mut app.ui_state.modal {
            crate::tui::app::Modal::Settings(s) => s,
            _ => unreachable!(),
        };
        s.programs_state.target = crate::backend::BackendId(1);
        s.programs_state.loading = true;
        s.programs_state.selected = 5; // will be clamped
        (s.programs_state.target, s.programs_state.load_gen)
    };

    app.handle_state_update(StateUpdate::ServerProgramsLoaded {
        backend: target,
        generation,
        result: Ok(vec![program_entry("Remote", "claude")]),
    })
    .await;

    let prog = peek_programs(&app);
    assert!(!prog.loading);
    assert_eq!(prog.entries, vec![program_entry("Remote", "claude")]);
    assert_eq!(prog.selected, 0, "selection clamped to the new list");
    assert!(prog.load_error.is_none());
}

#[tokio::test]
async fn server_programs_loaded_ignored_for_stale_generation() {
    use crate::tui::event::StateUpdate;

    let mut app = make_test_app();
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);
    let target = crate::backend::BackendId(1);
    if let crate::tui::app::Modal::Settings(s) = &mut app.ui_state.modal {
        s.programs_state.target = target;
        s.programs_state.loading = true;
        s.programs_state.load_gen = 7;
    }

    // A response from a superseded load (wrong generation) is dropped.
    app.handle_state_update(StateUpdate::ServerProgramsLoaded {
        backend: target,
        generation: 6,
        result: Ok(vec![program_entry("Stale", "stale")]),
    })
    .await;

    let prog = peek_programs(&app);
    assert!(prog.loading, "still loading; stale response ignored");
    assert!(prog.entries.is_empty());
}

#[tokio::test]
async fn server_programs_loaded_error_sets_load_error() {
    use crate::tui::event::StateUpdate;

    let mut app = make_test_app();
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);
    let (target, generation) = {
        let s = match &mut app.ui_state.modal {
            crate::tui::app::Modal::Settings(s) => s,
            _ => unreachable!(),
        };
        s.programs_state.target = crate::backend::BackendId(1);
        s.programs_state.loading = true;
        (s.programs_state.target, s.programs_state.load_gen)
    };

    app.handle_state_update(StateUpdate::ServerProgramsLoaded {
        backend: target,
        generation,
        result: Err("connection refused".to_string()),
    })
    .await;

    let prog = peek_programs(&app);
    assert!(!prog.loading);
    assert_eq!(prog.load_error.as_deref(), Some("connection refused"));
}

#[tokio::test]
async fn commit_to_missing_backend_does_not_clobber_local_config() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    // Local config has its own programs, which must be left untouched.
    app.config.programs = vec![program_entry("Local", "local-cmd")];
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);

    // Simulate a tab that was pointed at a remote server which has since been
    // removed: a non-local target id with no backend, but a loaded (editable)
    // working copy.
    if let crate::tui::app::Modal::Settings(s) = &mut app.ui_state.modal {
        s.programs_state.target = crate::backend::BackendId(999);
        s.programs_state.entries = vec![program_entry("Remote", "remote-cmd")];
        s.programs_state.loading = false;
        s.programs_state.load_error = None;
        s.programs_state.selected = 0;
    }

    // Deleting the entry commits to the (missing) remote target.
    feed_programs_key(&mut app, KeyCode::Char('d')).await;

    // The local config must NOT have been overwritten with the remote list
    // (the old `backend_arc` fallback would have done exactly that).
    assert_eq!(
        app.config.programs,
        vec![program_entry("Local", "local-cmd")]
    );
    // And the tab surfaces that the target is gone.
    assert!(peek_programs(&app).load_error.is_some());
}

#[tokio::test]
async fn cycle_programs_target_noop_with_single_backend() {
    use crossterm::event::KeyCode;

    let mut app = make_test_app();
    app.open_settings_on_programs(crate::backend::LOCAL_BACKEND_ID);
    // `t` cycles targets; with only the local backend it stays put.
    feed_programs_key(&mut app, KeyCode::Char('t')).await;
    assert_eq!(peek_programs(&app).target, crate::backend::LOCAL_BACKEND_ID);
}

#[cfg(test)]
mod iterm2_protocol_override {
    use super::iterm2_kitty_override;
    use ratatui_image::picker::ProtocolType;

    #[test]
    fn kitty_on_iterm2_is_overridden_to_iterm2() {
        assert_eq!(
            iterm2_kitty_override(ProtocolType::Kitty, Some("iTerm.app"), None),
            Some(ProtocolType::Iterm2)
        );
    }

    #[test]
    fn kitty_on_iterm2_via_lc_terminal_is_overridden() {
        // LC_TERMINAL is iTerm2's marker when forwarded over ssh.
        assert_eq!(
            iterm2_kitty_override(ProtocolType::Kitty, Some("tmux"), Some("iTerm2")),
            Some(ProtocolType::Iterm2)
        );
    }

    #[test]
    fn kitty_on_a_real_kitty_terminal_is_kept() {
        assert_eq!(
            iterm2_kitty_override(ProtocolType::Kitty, Some("ghostty"), None),
            None
        );
    }

    #[test]
    fn non_kitty_detection_is_never_overridden() {
        // An honest iTerm2 probe (or halfblocks fallback) must pass through.
        assert_eq!(
            iterm2_kitty_override(ProtocolType::Iterm2, Some("iTerm.app"), None),
            None
        );
        assert_eq!(
            iterm2_kitty_override(ProtocolType::Halfblocks, Some("iTerm.app"), None),
            None
        );
    }

    #[test]
    fn missing_env_keeps_detection() {
        assert_eq!(iterm2_kitty_override(ProtocolType::Kitty, None, None), None);
    }
}

// ---------------------------------------------------------------------------
// Review image cache: generation guard + mouse-driven lazy fetch
// ---------------------------------------------------------------------------

/// A single modified image `FileDiff` for review-image tests.
fn modified_image_file(path: &str) -> crate::git::FileDiff {
    crate::git::FileDiff {
        old_path: path.to_string(),
        new_path: path.to_string(),
        status: crate::git::FileStatus::Modified,
        added: 0,
        removed: 0,
        hunks: Vec::new(),
        binary: Some(crate::git::BinaryInfo {
            kind: crate::git::BinaryKind::Image {
                mime: "image/png".to_string(),
            },
            old_oid: None,
            new_oid: None,
            old_size: Some(10),
            new_size: Some(20),
        }),
    }
}

#[test]
fn reset_review_images_clears_cache_and_bumps_generation() {
    let app = make_test_app();
    let gen0 = app.review_image_gen.get();
    app.review_images.borrow_mut().insert(
        ("logo.png".to_string(), crate::api::DiffSide::New),
        ImageEntry::Pending,
    );

    app.reset_review_images();

    assert!(app.review_images.borrow().is_empty(), "cache should clear");
    assert_eq!(
        app.review_image_gen.get(),
        gen0 + 1,
        "opening a review bumps the generation"
    );
}

#[tokio::test]
async fn stale_review_image_arrivals_are_dropped() {
    use crate::api::DiffSide;
    use crate::tui::event::StateUpdate;

    let mut app = make_test_app();
    // Opening a review bumps the generation and clears the cache.
    app.reset_review_images();
    let current = app.review_image_gen.get();

    // A late arrival from a *previous* review (stale generation) must be
    // dropped — otherwise it could repopulate the cleared cache and show the
    // wrong image for a same-named path in the now-open review.
    app.handle_state_update(StateUpdate::ReviewImageLoaded {
        generation: current.wrapping_sub(1),
        path: "logo.png".to_string(),
        side: DiffSide::New,
        image: Err("from a closed review".to_string()),
    })
    .await;
    assert!(
        app.review_images.borrow().is_empty(),
        "stale-generation arrival must be dropped, not cached"
    );

    // An arrival for the currently-open review is cached.
    app.handle_state_update(StateUpdate::ReviewImageLoaded {
        generation: current,
        path: "logo.png".to_string(),
        side: DiffSide::New,
        image: Err("decode failed".to_string()),
    })
    .await;
    assert!(
        app.review_images
            .borrow()
            .contains_key(&("logo.png".to_string(), DiffSide::New)),
        "current-generation arrival must be cached"
    );
}

#[tokio::test]
async fn mouse_file_click_kicks_off_image_fetch() {
    use crate::api::DiffSide;

    let mut app = make_test_app();
    let state = DiffReviewState::new(
        SessionId::new(),
        "t".to_string(),
        "base".to_string(),
        crate::git::ParsedDiff {
            files: vec![modified_image_file("logo.png")],
        },
        Vec::new(),
    );
    app.ui_state.modal = Modal::ReviewDiff(Box::new(state));
    app.ui_state.review_file_list_rect = Some(Rect {
        x: 0,
        y: 0,
        width: 20,
        height: 20,
    });

    // Left-click the (single) image file row in the tree. Before the fix this
    // changed the selection but never started the lazy fetch, leaving the image
    // stuck on "Loading image…" forever. Now it must enqueue a fetch — an entry
    // appears in the image cache (the session has no worktree on disk here, so
    // the fetch resolves to `Failed`, but the key being present proves the fetch
    // was kicked off).
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 5,
        row: 0,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    app.handle_input(crate::tui::event::InputEvent::Mouse(click))
        .await;

    assert!(
        app.review_images
            .borrow()
            .contains_key(&("logo.png".to_string(), DiffSide::New)),
        "clicking an image file in the tree should kick off its image fetch"
    );
}

// --- ProgramPicker (new-session program selection) ---

fn picker(commands: &[&str], selected: usize) -> ProgramPicker {
    ProgramPicker {
        choices: commands
            .iter()
            .map(|c| crate::config::ProgramEntry {
                label: c.to_string(),
                command: c.to_string(),
            })
            .collect(),
        selected,
    }
}

#[test]
fn program_picker_selected_command_reads_highlight() {
    let p = picker(&["claude", "codex"], 1);
    assert_eq!(p.selected_command().as_deref(), Some("codex"));
}

#[test]
fn program_picker_navigation_saturates_at_ends() {
    let mut p = picker(&["claude", "codex"], 0);
    // Up at the top stays put.
    p.select_up();
    assert_eq!(p.selected, 0);
    // Down advances, then saturates at the last entry.
    p.select_down();
    assert_eq!(p.selected, 1);
    p.select_down();
    assert_eq!(p.selected, 1);
}

// --- ServerPicker (new-session server selection) ---

#[test]
fn server_picker_selected_backend_and_default() {
    let choices = vec![
        (LOCAL_BACKEND_ID, "local".to_string()),
        (BackendId(1), "buildbox".to_string()),
    ];
    // Defaults to the entry for the requested backend, and `committed` matches so
    // an immediate confirm is a no-op.
    let p = ServerPicker::new(choices.clone(), BackendId(1));
    assert_eq!(p.selected, 1);
    assert_eq!(p.committed, 1);
    assert_eq!(p.selected_backend(), Some(BackendId(1)));
    // An unknown default falls back to the first entry.
    let p = ServerPicker::new(choices, BackendId(99));
    assert_eq!(p.selected_backend(), Some(LOCAL_BACKEND_ID));
}

// --- SectionPicker (new-session section selection) ---

#[test]
fn section_picker_catch_all_maps_to_none() {
    // Row 0 is always the catch-all → no override.
    let p = SectionPicker::new(vec!["Open PRs".to_string()], None);
    assert_eq!(p.choices[0], crate::session::IN_PROGRESS);
    assert_eq!(p.selected, 0);
    assert_eq!(p.selected_section(), None);
}

#[test]
fn section_picker_selects_configured_row() {
    let p = SectionPicker::new(vec!["Open PRs".to_string(), "Merged".to_string()], None);
    // Highlight the second configured section (index 2, after the catch-all).
    let mut p = p;
    p.select_down();
    p.select_down();
    assert_eq!(p.selected, 2);
    assert_eq!(p.selected_section().as_deref(), Some("Merged"));
}

#[test]
fn section_picker_pre_selects_the_default_row() {
    // A default naming a configured section pre-selects that row…
    let p = SectionPicker::new(
        vec!["Open PRs".to_string(), "Merged".to_string()],
        Some("Merged"),
    );
    assert_eq!(p.selected, 2);
    assert_eq!(p.selected_section().as_deref(), Some("Merged"));
    // …while an unknown default falls back to the catch-all (row 0 / None).
    let p = SectionPicker::new(vec!["Open PRs".to_string()], Some("Gone"));
    assert_eq!(p.selected, 0);
    assert_eq!(p.selected_section(), None);
}

#[test]
fn section_picker_default_prefers_a_configured_section_over_the_catch_all() {
    // A section configured with the reserved catch-all spelling must still be
    // selectable: matching a default starts from row 1, so it resolves to the
    // configured row (index 1) rather than the catch-all (index 0).
    let p = SectionPicker::new(
        vec![crate::session::IN_PROGRESS.to_string()],
        Some(crate::session::IN_PROGRESS),
    );
    assert_eq!(p.selected, 1);
    assert_eq!(
        p.selected_section().as_deref(),
        Some(crate::session::IN_PROGRESS)
    );
}

// --- InputFocus (Tab cycling in the input modal) ---

/// All four optional fields present (the new-session dialog with >1 backend and
/// configured sections).
fn all_fields() -> crate::tui::app::FieldsPresent {
    crate::tui::app::FieldsPresent {
        server: true,
        project: true,
        program: true,
        section: true,
    }
}

/// Only the project + program fields (single backend, no configured sections) —
/// the classic layout before server/section were added.
fn project_program_only() -> crate::tui::app::FieldsPresent {
    crate::tui::app::FieldsPresent {
        server: false,
        project: true,
        program: true,
        section: false,
    }
}

#[test]
fn input_focus_cycles_all_present_fields() {
    // Name → Server → Project → Program → Section → Name with every field present.
    let f = all_fields();
    assert_eq!(InputFocus::Name.next(f), InputFocus::Server);
    assert_eq!(InputFocus::Server.next(f), InputFocus::Project);
    assert_eq!(InputFocus::Project.next(f), InputFocus::Program);
    assert_eq!(InputFocus::Program.next(f), InputFocus::Section);
    assert_eq!(InputFocus::Section.next(f), InputFocus::Name);
}

#[test]
fn input_focus_skips_absent_fields() {
    // No server/section: Name → Project → Program → Name.
    let f = project_program_only();
    assert_eq!(InputFocus::Name.next(f), InputFocus::Project);
    assert_eq!(InputFocus::Project.next(f), InputFocus::Program);
    assert_eq!(InputFocus::Program.next(f), InputFocus::Name);
    // No project picker: Name → Program → Name.
    let no_project = crate::tui::app::FieldsPresent {
        server: false,
        project: false,
        program: true,
        section: false,
    };
    assert_eq!(InputFocus::Name.next(no_project), InputFocus::Program);
    assert_eq!(InputFocus::Program.next(no_project), InputFocus::Name);
    // Nothing optional present: Tab stays on the name field.
    let none = crate::tui::app::FieldsPresent {
        server: false,
        project: false,
        program: false,
        section: false,
    };
    assert_eq!(InputFocus::Name.next(none), InputFocus::Name);
}

#[test]
fn input_focus_prev_cycles_backward() {
    // Shift+Tab reverses the full ring.
    let f = all_fields();
    assert_eq!(InputFocus::Name.prev(f), InputFocus::Section);
    assert_eq!(InputFocus::Section.prev(f), InputFocus::Program);
    assert_eq!(InputFocus::Program.prev(f), InputFocus::Project);
    assert_eq!(InputFocus::Project.prev(f), InputFocus::Server);
    assert_eq!(InputFocus::Server.prev(f), InputFocus::Name);
    // Absent fields are skipped, same as forward cycling.
    let f = project_program_only();
    assert_eq!(InputFocus::Name.prev(f), InputFocus::Program);
    assert_eq!(InputFocus::Program.prev(f), InputFocus::Project);
    assert_eq!(InputFocus::Project.prev(f), InputFocus::Name);
}

// --- ProjectPicker (new-session project selection) ---

fn project_choices(names: &[&str]) -> Vec<ProjectChoice> {
    names
        .iter()
        .map(|n| ProjectChoice {
            id: ProjectId::new(),
            name: n.to_string(),
            repo_path: std::path::PathBuf::from(format!("/repos/{n}")),
        })
        .collect()
}

#[test]
fn project_picker_new_preselects_default() {
    let choices = project_choices(&["alpha", "beta", "gamma"]);
    let default = choices[2].id;
    let p = ProjectPicker::new(choices.clone(), default);
    assert_eq!(p.selected_id(), Some(default));
    assert_eq!(p.selected_choice().map(|c| c.name.as_str()), Some("gamma"));
}

#[test]
fn project_picker_navigation_saturates_over_filtered() {
    let choices = project_choices(&["alpha", "beta"]);
    let first = choices[0].id;
    let mut p = ProjectPicker::new(choices, first);
    p.select_up();
    assert_eq!(p.selected, 0);
    p.select_down();
    assert_eq!(p.selected, 1);
    p.select_down();
    assert_eq!(p.selected, 1);
}

#[test]
fn project_picker_apply_filter_narrows_and_reanchors() {
    let choices = project_choices(&["commander", "kokoro", "commons"]);
    let default = choices[0].id; // "commander"
    let mut p = ProjectPicker::new(choices, default);

    // Filter to entries fuzzy-matching "com" — "commander" and "commons".
    p.filter = "com".to_string();
    p.apply_filter();
    assert_eq!(p.filtered.len(), 2);
    let names: Vec<&str> = p
        .filtered
        .iter()
        .map(|&i| p.choices[i].name.as_str())
        .collect();
    assert!(names.contains(&"commander"));
    assert!(names.contains(&"commons"));
    assert!(!names.contains(&"kokoro"));
    // The previously-selected "commander" survives the filter, so the
    // highlight re-anchors onto it rather than jumping to the top.
    assert_eq!(p.selected_id(), Some(default));

    // Clearing the filter restores all choices in name order.
    p.filter.clear();
    p.apply_filter();
    assert_eq!(p.filtered, vec![0, 1, 2]);
}

#[test]
fn project_picker_apply_filter_clamps_when_selection_filtered_out() {
    let choices = project_choices(&["alpha", "beta"]);
    let beta = choices[1].id;
    let mut p = ProjectPicker::new(choices, beta);
    assert_eq!(p.selected, 1);
    // A filter that excludes the current selection resets to the top match.
    p.filter = "alpha".to_string();
    p.apply_filter();
    assert_eq!(p.selected, 0);
    assert_eq!(p.selected_choice().map(|c| c.name.as_str()), Some("alpha"));
}

#[test]
fn project_picker_no_match_has_no_selection() {
    // When the filter matches nothing there's no project to submit under — the
    // Enter handler keys off `selected_id()` being None to keep the dialog open.
    let choices = project_choices(&["alpha", "beta"]);
    let first = choices[0].id;
    let mut p = ProjectPicker::new(choices, first);
    p.filter = "zzzznomatch".to_string();
    p.apply_filter();
    assert!(p.filtered.is_empty());
    assert_eq!(p.selected_id(), None);
}

#[test]
fn project_picker_scroll_keeps_selection_visible() {
    // More projects than fit on screen: scrolling down keeps the highlight
    // inside the visible window rather than off the bottom.
    let names: Vec<String> = (0..12).map(|i| format!("proj{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let choices = project_choices(&refs);
    let first = choices[0].id;
    let mut p = ProjectPicker::new(choices, first);
    assert_eq!(p.scroll, 0);
    for _ in 0..11 {
        p.select_down();
        assert!(
            p.scroll <= p.selected && p.selected < p.scroll + 6,
            "selected {} must stay within window [{}, {})",
            p.selected,
            p.scroll,
            p.scroll + 6
        );
    }
    assert_eq!(p.selected, 11);
    // Scrolling back to the top brings the window with it.
    for _ in 0..11 {
        p.select_up();
    }
    assert_eq!(p.selected, 0);
    assert_eq!(p.scroll, 0);
}

#[tokio::test]
async fn backtab_toggles_review_focus_like_tab() {
    let mut app = make_test_app();
    let state = DiffReviewState::new(
        SessionId::new(),
        "t".to_string(),
        "base".to_string(),
        crate::git::ParsedDiff {
            files: vec![modified_image_file("logo.png")],
        },
        Vec::new(),
    );
    assert_eq!(state.focus, ReviewFocus::FileList);
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::BackTab,
        crossterm::event::KeyModifiers::NONE,
    );
    app.handle_review_key(key, Box::new(state)).await;
    match &app.ui_state.modal {
        Modal::ReviewDiff(s) => {
            assert_eq!(s.focus, ReviewFocus::Body, "BackTab should flip focus")
        }
        other => panic!("expected review modal to stay open, got {other:?}"),
    }
}

// ===========================================================================
// Main-view render characterization (Phase C0 safety net)
//
// These byte-identical buffer snapshots pin the CURRENT rendered output of the
// session-list tree across all three `ViewMode`s and a couple of interaction
// states. The Phase C refactor moves the tree/render data source from the
// local `AppState` onto DTO snapshots behind a backend trait; re-running these
// against the refactor proves the pixels did not move for local users.
//
// Normalization (documented so goldens stay stable):
//   * Each buffer row is flattened to its cell symbols, OSC 8 hyperlink escape
//     sequences (injected around PR badges) are stripped, and trailing
//     whitespace is trimmed. Styling/colour is intentionally NOT captured —
//     `TestBackend` symbols carry glyphs only, which keeps goldens independent
//     of the auto-detected terminal theme.
//   * All session timestamps are fixed, and `tick_count` is pinned to 0 so the
//     braille spinner (Working / Creating rows) resolves to a stable frame.
// ===========================================================================

/// Strip OSC 8 hyperlink escape sequences (`ESC ] ... BEL`) that
/// `inject_pr_hyperlinks` wraps around PR-badge glyphs, leaving the visible
/// text (e.g. `PR #42`) behind.
fn strip_osc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip until the terminating BEL.
            for c2 in chars.by_ref() {
                if c2 == '\u{07}' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Flatten a `TestBackend` buffer into one newline-joined string, one line per
/// row, OSC escapes stripped and trailing whitespace trimmed. See the module
/// comment above for why styling is dropped.
fn buffer_lines(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = buffer.area;
    let mut out = String::new();
    for y in 0..area.height {
        let mut row = String::new();
        for x in 0..area.width {
            row.push_str(buffer[(x, y)].symbol());
        }
        out.push_str(strip_osc(&row).trim_end());
        out.push('\n');
    }
    out
}

/// A fixed instant plus `offset` seconds, so seeded ordering is deterministic
/// and any rendered timestamp is stable.
fn fixed_time(offset: i64) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    chrono::Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap() + chrono::Duration::seconds(offset)
}

/// Seed a small but feature-rich scenario into `app` and refresh the list.
///
/// Covers three projects, a stacked parent/child chain, unread plus Working,
/// WaitingForInput and Idle agent states, a Creating and a Stopped session,
/// open/merged/draft PR chips, and section placement (including one manual
/// `section_override`). All timestamps are fixed so ordering is deterministic.
async fn seed_render_scenario(app: &mut App) {
    use crate::git::PrState;
    use crate::session::{Project, SectionConfig, SessionStatus, WorktreeSession};
    use std::path::PathBuf;

    app.config.sections = vec![
        SectionConfig {
            name: "Review".to_string(),
            ..Default::default()
        },
        SectionConfig {
            name: "Done".to_string(),
            ..Default::default()
        },
    ];

    let alpha = Project::new("alpha", PathBuf::from("/tmp/alpha"), "main");
    let bravo = Project::new("bravo", PathBuf::from("/tmp/bravo"), "main");
    let charlie = Project::new("charlie", PathBuf::from("/tmp/charlie"), "develop");
    let (alpha_id, bravo_id, charlie_id) = (alpha.id, bravo.id, charlie.id);

    // alpha: working / unread / stacked base+child / stopped
    let mut working =
        WorktreeSession::new(alpha_id, "fix-login", "fix-login", PathBuf::new(), "claude");
    working.created_at = fixed_time(50);
    working.status = SessionStatus::Running;

    let mut unread = WorktreeSession::new(
        alpha_id,
        "flaky-test",
        "flaky-test",
        PathBuf::new(),
        "claude",
    );
    unread.created_at = fixed_time(40);
    unread.unread = true;

    let mut base = WorktreeSession::new(
        alpha_id,
        "refactor",
        "refactor-br",
        PathBuf::new(),
        "claude",
    );
    base.created_at = fixed_time(30);

    let mut child = WorktreeSession::new(
        alpha_id,
        "refactor-2",
        "refactor-2-br",
        PathBuf::new(),
        "claude",
    );
    child.created_at = fixed_time(35);
    child.pr_base_branch = Some("refactor-br".to_string());

    let mut stopped =
        WorktreeSession::new(alpha_id, "old-thing", "old-thing", PathBuf::new(), "claude");
    stopped.created_at = fixed_time(10);
    stopped.status = SessionStatus::Stopped;

    // bravo: creating / PR open / PR merged / PR draft
    let mut creating = WorktreeSession::new(
        bravo_id,
        "spinning-up",
        "spinning-up",
        PathBuf::new(),
        "claude",
    );
    creating.created_at = fixed_time(45);
    creating.status = SessionStatus::Creating;

    let mut pr_open =
        WorktreeSession::new(bravo_id, "review-me", "review-me", PathBuf::new(), "claude");
    pr_open.created_at = fixed_time(35);
    pr_open.pr_number = Some(42);
    pr_open.pr_url = Some("https://example.com/pr/42".to_string());
    pr_open.pr_state = Some(PrState::Open);
    pr_open.current_section = Some("Review".to_string());
    pr_open.section_override = Some("Review".to_string());

    let mut pr_merged =
        WorktreeSession::new(bravo_id, "shipped", "shipped", PathBuf::new(), "claude");
    pr_merged.created_at = fixed_time(25);
    pr_merged.pr_number = Some(7);
    pr_merged.pr_url = Some("https://example.com/pr/7".to_string());
    pr_merged.pr_state = Some(PrState::Merged);
    pr_merged.pr_merged = true;
    pr_merged.current_section = Some("Done".to_string());

    let mut pr_draft = WorktreeSession::new(bravo_id, "wip-pr", "wip-pr", PathBuf::new(), "claude");
    pr_draft.created_at = fixed_time(15);
    pr_draft.pr_number = Some(99);
    pr_draft.pr_url = Some("https://example.com/pr/99".to_string());
    pr_draft.pr_state = Some(PrState::Open);
    pr_draft.pr_draft = true;

    // charlie: waiting-for-input
    let mut waiting = WorktreeSession::new(
        charlie_id,
        "need-input",
        "need-input",
        PathBuf::new(),
        "claude",
    );
    waiting.created_at = fixed_time(20);

    let working_id = working.id;
    let base_id = base.id;
    let pr_open_id = pr_open.id;

    app.service
        .store()
        .mutate(move |state| {
            state.add_project(alpha);
            state.add_project(bravo);
            state.add_project(charlie);
            for s in [
                working, unread, base, child, stopped, creating, pr_open, pr_merged, pr_draft,
                waiting,
            ] {
                state.add_session(s);
            }
        })
        .await
        .unwrap();

    // Agent states: Working / Idle / WaitingForInput; others left unset.
    app.ui_state
        .agent_states
        .insert(working_id, AgentState::Working);
    app.ui_state.agent_states.insert(base_id, AgentState::Idle);
    app.ui_state
        .agent_states
        .insert(pr_open_id, AgentState::WaitingForInput);

    // Pin the spinner frame so Working/Creating rows are stable.
    app.ui_state.tick_count = 0;
    app.sync_local_view_from_store_for_test().await;
    app.refresh_list_items().await;
}

#[tokio::test]
async fn render_project_grouped_view_matches_snapshot() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    seed_render_scenario(&mut app).await;
    app.ui_state.view_mode = ViewMode::ProjectGrouped;
    app.refresh_list_items().await;
    app.ui_state.list_state.select(Some(0));
    app.update_selection();

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let expected = r#"
  Sessions [Project]:               ┌ Shell · Info ───────────────────────────────────────────────────────────────────┐
  alpha [main] (5)                  │                                                                                 │
      1 ⠋ fix-login                 │                                                                                 │
      2 ◆ flaky-test                │                                                                                 │
      3 ● refactor [refactor-br]    │                                                                                 │
         4 ● refactor-2 [refactor-2-│                                                                                 │
      5 ○ old-thing                 │                                                                                 │
  bravo [main] (4)                  │                                                                                 │
      6 ⠋ spinning-up               │                                                                                 │
      7 ? review-me  PR #42         │                                                                                 │
      8 ● shipped  PR #7            │                                                                                 │
      9 ● wip-pr  PR #99            │                                                                                 │
  charlie [develop] (1)             │                                                                                 │
     10 ● need-input                │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    └─────────────────────────────────────────────────────────────────────────────────┘

 Sessions: 10 │ [n]ew session │ s[t]acked │ [N]ew project                                                        ? help
"#;
    pretty_assertions::assert_eq!(buffer_lines(&terminal), expected);
}

#[tokio::test]
async fn render_section_grouped_view_matches_snapshot() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    seed_render_scenario(&mut app).await;
    app.ui_state.view_mode = ViewMode::SectionGrouped;
    app.refresh_list_items().await;
    app.ui_state.list_state.select(Some(0));
    app.update_selection();

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let expected = r#"
  Sessions [Sections]:              ┌ Preview · Info · Shell ─────────────────────────────────────────────────────────┐
  ▾ In Progress (8)                 │                                                                                 │
    alpha [main] (5)                │                                                                                 │
      1 ⠋ fix-login                 │                                                                                 │
      2 ◆ flaky-test                │                                                                                 │
      3 ● refactor [refactor-br]    │                                                                                 │
      4 ● refactor-2 [refactor-2-br]│                                                                                 │
      5 ○ old-thing                 │                                                                                 │
    bravo [main] (2)                │                                                                                 │
      6 ⠋ spinning-up               │                                                                                 │
      7 ● wip-pr  PR #99            │                                                                                 │
    charlie [develop] (1)           │                                                                                 │
      8 ● need-input                │                                                                                 │
   ────────────────────             │                                                                                 │
  ▾ Review (1)                      │                                                                                 │
    bravo [main] (1)                │                                                                                 │
      9 ? review-me  PR #42         │                                                                                 │
   ────────────────────             │                                                                                 │
  ▾ Done (1)                        │                                                                                 │
    bravo [main] (1)                │                                                                                 │
     10 ● shipped  PR #7            │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    └─────────────────────────────────────────────────────────────────────────────────┘

 Sessions: 10 │ [n]ew session │ s[t]acked │ [N]ew project                                                        ? help
"#;
    pretty_assertions::assert_eq!(buffer_lines(&terminal), expected);
}

#[tokio::test]
async fn render_section_stacks_view_matches_snapshot() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    seed_render_scenario(&mut app).await;
    app.ui_state.view_mode = ViewMode::SectionStacks;
    app.refresh_list_items().await;
    app.ui_state.list_state.select(Some(0));
    app.update_selection();

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let expected = r#"
  Sessions [Section Stacks]:        ┌ Preview · Info · Shell ─────────────────────────────────────────────────────────┐
  ▾ In Progress (8)                 │                                                                                 │
    alpha [main] (5)                │                                                                                 │
      1 ⠋ fix-login                 │                                                                                 │
      2 ◆ flaky-test                │                                                                                 │
      3 ● refactor [refactor-br]    │                                                                                 │
         4 ● refactor-2 [refactor-2-│                                                                                 │
      5 ○ old-thing                 │                                                                                 │
    bravo [main] (2)                │                                                                                 │
      6 ⠋ spinning-up               │                                                                                 │
      7 ● wip-pr  PR #99            │                                                                                 │
    charlie [develop] (1)           │                                                                                 │
      8 ● need-input                │                                                                                 │
   ────────────────────             │                                                                                 │
  ▾ Review (1)                      │                                                                                 │
    bravo [main] (1)                │                                                                                 │
      9 ? review-me  PR #42         │                                                                                 │
   ────────────────────             │                                                                                 │
  ▾ Done (1)                        │                                                                                 │
    bravo [main] (1)                │                                                                                 │
     10 ● shipped  PR #7            │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    │                                                                                 │
                                    └─────────────────────────────────────────────────────────────────────────────────┘

 Sessions: 10 │ [n]ew session │ s[t]acked │ [N]ew project                                                        ? help
"#;
    pretty_assertions::assert_eq!(buffer_lines(&terminal), expected);
}

#[tokio::test]
async fn render_navigation_moves_selection_and_swaps_status_actions() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    seed_render_scenario(&mut app).await;
    app.ui_state.view_mode = ViewMode::ProjectGrouped;
    app.refresh_list_items().await;

    // Start on the first row (the `alpha` project header) and move the
    // selection down two rows onto the second worktree (`flaky-test`).
    app.ui_state.list_state.select(Some(0));
    app.update_selection();
    app.ui_state.list_state.next();
    app.ui_state.list_state.next();
    app.update_selection();

    // Selection landed on list index 2, which is the unread `flaky-test`
    // worktree; `selected_session_id` tracks it and no project is selected.
    assert_eq!(app.ui_state.list_state.selected(), Some(2));
    let selected = &app.ui_state.list_items[2];
    let (selected_id, selected_project) = match selected {
        SessionListItem::Worktree {
            id,
            project_id,
            title,
            unread,
            ..
        } => {
            assert_eq!(title, "flaky-test");
            assert!(unread, "flaky-test is seeded unread");
            (*id, *project_id)
        }
        other => panic!("expected the flaky-test worktree at index 2, got {other:?}"),
    };
    assert_eq!(
        app.ui_state.selected_session_id.map(|r| r.id),
        Some(selected_id)
    );
    // Characterization: selecting a worktree also stamps `selected_project_id`
    // with the session's parent project (it is not cleared to None).
    assert_eq!(
        app.ui_state.selected_project_id.map(|(_, p)| p),
        Some(selected_project)
    );

    // With a session selected, the status bar surfaces the session-scoped
    // action buttons (delete / review / edit) alongside the always-present
    // new-session and new-project actions.
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let lines = buffer_lines(&terminal);
    let status = lines.lines().last().unwrap();
    assert_eq!(
        status,
        " Sessions: 10 │ [n]ew session │ s[t]acked │ [d]elete │ [r]eview │ edit [.] │ [N]ew project                       ? help"
    );
}

// --- actions.rs decision predicates (characterization) ----------------------

#[tokio::test]
async fn selected_session_is_creating_tracks_selected_worktree_status() {
    let mut app = make_test_app();
    seed_render_scenario(&mut app).await;
    app.ui_state.view_mode = ViewMode::ProjectGrouped;
    app.refresh_list_items().await;

    // Point the selection at the `spinning-up` (Creating) worktree.
    let creating_id = app.ui_state.list_items.iter().find_map(|i| match i {
        SessionListItem::Worktree {
            id, title, status, ..
        } if title == "spinning-up" && *status == SessionStatus::Creating => Some(*id),
        _ => None,
    });
    app.ui_state.selected_session_id = creating_id.map(crate::backend::SessionRef::local);
    assert!(
        app.selected_session_is_creating(),
        "a selected Creating session should be reported as creating"
    );

    // A Running session is not creating.
    let running_id = app.ui_state.list_items.iter().find_map(|i| match i {
        SessionListItem::Worktree { id, title, .. } if title == "fix-login" => Some(*id),
        _ => None,
    });
    app.ui_state.selected_session_id = running_id.map(crate::backend::SessionRef::local);
    assert!(!app.selected_session_is_creating());

    // No selection at all is not creating.
    app.ui_state.selected_session_id = None;
    assert!(!app.selected_session_is_creating());
}

#[tokio::test]
async fn selected_item_is_section_header_detects_header_rows() {
    let mut app = make_test_app();
    seed_render_scenario(&mut app).await;
    app.ui_state.view_mode = ViewMode::SectionGrouped;
    app.refresh_list_items().await;

    // Index 0 in a section view is the "In Progress" section header.
    assert!(matches!(
        app.ui_state.list_items[0],
        SessionListItem::SectionHeader { .. }
    ));
    app.ui_state.list_state.select(Some(0));
    assert!(app.selected_item_is_section_header());

    // Select the first worktree row instead — not a header.
    let worktree_idx = app
        .ui_state
        .list_items
        .iter()
        .position(|i| matches!(i, SessionListItem::Worktree { .. }))
        .unwrap();
    app.ui_state.list_state.select(Some(worktree_idx));
    assert!(!app.selected_item_is_section_header());
}

// ---------------------------------------------------------------------------
// Phase E: multi-backend tree, connection state, hot-reload reconcile
// ---------------------------------------------------------------------------

use super::reconcile_remote_servers;
use crate::api::WorkspaceSnapshot;
use crate::backend::{
    BackendId, ConnectionState, RemoteBackendFactory, SessionRef, empty_snapshot, mock::MockBackend,
};

/// A snapshot carrying one running session under one project, for exercising a
/// remote backend's tree contents / command gating.
fn snapshot_with_one_session() -> (WorkspaceSnapshot, SessionId, ProjectId) {
    use crate::session::{Project, SessionStatus, WorktreeSession};
    let mut state = crate::config::AppState::default();
    let project = Project::new("remote-proj", std::path::PathBuf::from("/tmp/rp"), "main");
    let pid = project.id;
    let mut sess = WorktreeSession::new(
        pid,
        "remote-sess",
        "remote-br",
        std::path::PathBuf::new(),
        "claude",
    );
    sess.status = SessionStatus::Running;
    let sid = sess.id;
    let mut project = project;
    project.add_worktree(sid);
    state.projects.insert(pid, project);
    state.sessions.insert(sid, sess);
    (crate::api::workspace_snapshot_from_state(&state), sid, pid)
}

/// Build an `App` with the local backend plus one mock remote per `(name,
/// snapshot)`, wired through the real `App::new` factory path.
fn build_app_with_mock_remotes(servers: Vec<(&str, WorkspaceSnapshot)>) -> App {
    let tmp = tempfile::TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    let state_path = tmp.path().join("state.json");
    let mut config = Config::default();
    config.telemetry.enabled = false;
    let mut snapshots: std::collections::HashMap<String, WorkspaceSnapshot> = Default::default();
    for (name, snap) in servers {
        config
            .remote_servers
            .push(crate::config::RemoteServerConfig {
                name: name.to_string(),
                url: format!("http://{name}:7878"),
                token: None,
            });
        snapshots.insert(name.to_string(), snap);
    }
    let config_store = Arc::new(ConfigStore::with_path(config, config_path));
    let store = Arc::new(StateStore::with_path(AppState::new(), state_path));
    std::mem::forget(tmp);
    let factory: RemoteBackendFactory = Arc::new(move |cfg: &crate::config::RemoteServerConfig| {
        let snap = snapshots
            .get(&cfg.name)
            .cloned()
            .unwrap_or_else(empty_snapshot);
        Ok(Arc::new(MockBackend::new(cfg.name.clone(), snap)) as Arc<dyn CommanderBackend>)
    });
    App::new(
        config_store,
        store,
        crate::telemetry::FrontendInfo::new("test", "0.0.0"),
        factory,
    )
}

#[tokio::test]
async fn single_local_backend_suppresses_server_header() {
    // The C0 invariant: with only the local backend, no ServerHeader is emitted.
    let mut app = make_test_app();
    seed_render_scenario(&mut app).await;
    app.ui_state.view_mode = ViewMode::ProjectGrouped;
    app.refresh_list_items().await;
    assert!(
        !app.ui_state.list_items.iter().any(|i| i.is_server_header()),
        "a lone local backend must not render a server header"
    );
}

#[tokio::test]
async fn multi_backend_tree_emits_server_headers_in_config_order_local_first() {
    let mut app = build_app_with_mock_remotes(vec![
        ("buildbox", empty_snapshot()),
        ("ci", empty_snapshot()),
    ]);
    app.bootstrap_backend_views().await;
    app.refresh_list_items().await;

    let headers: Vec<(BackendId, String)> = app
        .ui_state
        .list_items
        .iter()
        .filter_map(|i| match i {
            SessionListItem::ServerHeader { backend, name, .. } => Some((*backend, name.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        headers,
        vec![
            (BackendId(0), "local".to_string()),
            (BackendId(1), "buildbox".to_string()),
            (BackendId(2), "ci".to_string()),
        ],
        "server headers should be local-first, then config order"
    );
}

/// The `version_warning` on the buildbox server header, or `None` if there's no
/// such header.
fn buildbox_version_warning(app: &App) -> Option<crate::backend::VersionMismatch> {
    app.ui_state.list_items.iter().find_map(|i| match i {
        SessionListItem::ServerHeader {
            name,
            version_warning,
            ..
        } if name == "buildbox" => version_warning.clone(),
        _ => None,
    })
}

fn agent_states_box() -> Box<crate::api::AgentStatesSnapshot> {
    Box::new(crate::api::AgentStatesSnapshot {
        states: Default::default(),
        commander_running: false,
    })
}

#[tokio::test]
async fn older_server_snapshot_annotates_header_but_placeholder_does_not() {
    // A remote whose server build is behind this client (major.minor) flags a
    // warning on its header. Before its first real snapshot lands the header
    // carries the connecting placeholder (== client version), so no false alarm.
    let mut old_snap = empty_snapshot();
    old_snap.server.version = "0.1.0".to_string(); // certainly behind the client
    let mut app = build_app_with_mock_remotes(vec![("buildbox", old_snap.clone())]);
    app.bootstrap_backend_views().await;
    app.refresh_list_items().await;
    assert_eq!(
        buildbox_version_warning(&app),
        None,
        "the connecting placeholder reports the client version, so no warning yet"
    );

    // Land the older snapshot (first real snapshot arrives via BackendChanged).
    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(1).0,
        snapshot: Box::new(old_snap),
        states: agent_states_box(),
    })
    .await;
    assert_eq!(
        buildbox_version_warning(&app),
        Some(crate::backend::VersionMismatch {
            server: "0.1.0".to_string(),
            client: crate::VERSION.to_string(),
        }),
        "an older remote server must carry a version warning on its header"
    );
}

#[tokio::test]
async fn stale_server_toast_fires_once_and_never_for_local() {
    let mut old_snap = empty_snapshot();
    old_snap.server.version = "0.1.0".to_string();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", old_snap.clone())]);
    app.bootstrap_backend_views().await;

    // First fold of the older snapshot: the one-time toast fires.
    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(1).0,
        snapshot: Box::new(old_snap.clone()),
        states: agent_states_box(),
    })
    .await;
    let toast = app.ui_state.status_message.as_ref().map(|(m, _)| m.clone());
    assert!(
        toast
            .as_deref()
            .is_some_and(|m| m.contains("older than this client")),
        "first fold of a stale server must toast, got {toast:?}"
    );
    assert!(app.ui_state.version_warned.contains(&BackendId(1).0));

    // A second fold must NOT re-fire the toast.
    app.ui_state.status_message = None;
    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(1).0,
        snapshot: Box::new(old_snap),
        states: agent_states_box(),
    })
    .await;
    assert_eq!(
        app.ui_state.status_message, None,
        "a subsequent fold of the same stale server must not re-toast"
    );

    // The local backend reports the client's own version, so it never toasts.
    assert!(
        !app.ui_state.version_warned.contains(&BackendId(0).0),
        "the local backend must never flag a version mismatch"
    );
}

#[tokio::test]
async fn version_toast_does_not_clobber_a_live_status_message() {
    let mut old_snap = empty_snapshot();
    old_snap.server.version = "0.1.0".to_string();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", old_snap.clone())]);
    app.bootstrap_backend_views().await;

    // A live message (e.g. "Created session …") occupies the single slot.
    app.ui_state.status_message = Some((
        "busy".to_string(),
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    ));
    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(1).0,
        snapshot: Box::new(old_snap),
        states: agent_states_box(),
    })
    .await;
    assert_eq!(
        app.ui_state
            .status_message
            .as_ref()
            .map(|(m, _)| m.as_str()),
        Some("busy"),
        "the version toast must not overwrite a live message"
    );
    assert!(
        !app.ui_state.version_warned.contains(&BackendId(1).0),
        "a deferred toast must not mark the server warned, so it can retry"
    );

    // Once the slot frees, the next refresh delivers the deferred toast.
    app.ui_state.status_message = None;
    app.refresh_list_items().await;
    assert!(
        app.ui_state
            .status_message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("older than this client")),
        "the deferred toast must fire once the slot is free"
    );
    assert!(app.ui_state.version_warned.contains(&BackendId(1).0));
}

#[tokio::test]
async fn two_stale_servers_each_get_their_own_toast() {
    let mut old_snap = empty_snapshot();
    old_snap.server.version = "0.1.0".to_string();
    let mut app = build_app_with_mock_remotes(vec![
        ("buildbox", old_snap.clone()),
        ("ci", old_snap.clone()),
    ]);
    app.bootstrap_backend_views().await;

    // Fold buildbox (id 1): its toast fires; ci is still on its placeholder.
    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(1).0,
        snapshot: Box::new(old_snap.clone()),
        states: agent_states_box(),
    })
    .await;
    assert!(
        app.ui_state
            .status_message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("buildbox")),
        "first stale server toasts, got {:?}",
        app.ui_state.status_message
    );
    assert!(app.ui_state.version_warned.contains(&BackendId(1).0));
    assert!(!app.ui_state.version_warned.contains(&BackendId(2).0));

    // Free the slot, then fold ci (id 2): it gets its own toast.
    app.ui_state.status_message = None;
    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(2).0,
        snapshot: Box::new(old_snap),
        states: agent_states_box(),
    })
    .await;
    assert!(
        app.ui_state
            .status_message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("ci")),
        "second stale server toasts too, got {:?}",
        app.ui_state.status_message
    );
    assert!(app.ui_state.version_warned.contains(&BackendId(2).0));
}

#[tokio::test]
async fn multi_backend_list_keys_are_unique() {
    let (remote_snap, _sid, _pid) = snapshot_with_one_session();
    let (local_snap, _s, _p) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    // Give the local backend some content too.
    app.backends[0].view.snapshot = local_snap;
    app.backends[0].view.connection = ConnectionState::Connected;
    app.bootstrap_backend_views().await;
    app.refresh_list_items().await;

    let keys: Vec<String> = app.ui_state.list_items.iter().map(|i| i.key()).collect();
    let unique: std::collections::HashSet<&String> = keys.iter().collect();
    assert_eq!(
        keys.len(),
        unique.len(),
        "list item keys must be unique: {keys:?}"
    );
}

#[tokio::test]
async fn factory_failure_yields_degraded_placeholder() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = Config::default();
    config.telemetry.enabled = false;
    config
        .remote_servers
        .push(crate::config::RemoteServerConfig {
            name: "broken".to_string(),
            url: "http://broken:7878".to_string(),
            token: None,
        });
    let config_store = Arc::new(ConfigStore::with_path(
        config,
        tmp.path().join("config.toml"),
    ));
    let store = Arc::new(StateStore::with_path(
        AppState::new(),
        tmp.path().join("state.json"),
    ));
    std::mem::forget(tmp);
    let factory: RemoteBackendFactory = Arc::new(|_cfg| {
        Err(crate::backend::BackendError::InvalidRequest(
            "bad url".to_string(),
        ))
    });
    let app = App::new(
        config_store,
        store,
        crate::telemetry::FrontendInfo::new("test", "0.0.0"),
        factory,
    );
    // The broken server still occupies a handle, seeded Degraded with the reason.
    let handle = app
        .backend(BackendId(1))
        .expect("placeholder handle present");
    match &handle.view.connection {
        ConnectionState::Degraded { reason } => assert!(reason.contains("bad url"), "{reason}"),
        other => panic!("expected Degraded placeholder, got {other:?}"),
    }
    assert_eq!(handle.backend.descriptor().name, "broken");
}

#[test]
fn is_command_available_false_when_selected_backend_degraded() {
    // A session is selected but its owning backend is disconnected: session
    // actions must be gated off.
    let mut ui = AppUiState {
        selected_session_id: Some(SessionRef::new(BackendId(1), SessionId::new())),
        selected_project_id: Some((BackendId(1), ProjectId::new())),
        selected_backend_connected: false,
        ..AppUiState::default()
    };
    assert!(!ui.is_command_available(BindableAction::DeleteSession));
    assert!(!ui.is_command_available(BindableAction::RestartSession));
    // Flip to connected and the same action becomes available.
    ui.selected_backend_connected = true;
    assert!(ui.is_command_available(BindableAction::DeleteSession));
}

#[tokio::test]
async fn degraded_server_header_renders_greyed_name_and_reason() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = build_app_with_mock_remotes(vec![("buildbox", empty_snapshot())]);
    app.bootstrap_backend_views().await;
    // Drive the remote to Degraded and fold it into the view as the watcher would.
    app.handle_state_update(crate::tui::event::StateUpdate::BackendConnection {
        backend_id: 1,
        state: ConnectionState::Degraded {
            reason: "connection refused".to_string(),
        },
    })
    .await;

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_lines(&terminal);
    assert!(text.contains("buildbox"), "header should name the server");
    assert!(
        text.contains("connection refused"),
        "degraded header should show the reason: {text}"
    );
}

#[tokio::test]
async fn pull_blocked_badges_union_remote_backend_snapshot() {
    // A remote snapshot carries a blocked project pull. Folding it must surface
    // the badge — even though the block is on a remote, not the local backend.
    // Against a builder that reads only `local_view()`, the remote's blocked
    // project never lands in `project_pull_blocked`: red.
    use crate::api::{PullBlockReason, PullStatus};
    let (mut remote_snap, _sid, pid) = snapshot_with_one_session();
    remote_snap.project_pull.insert(
        pid,
        PullStatus::Blocked {
            reason: PullBlockReason::Dirty,
        },
    );
    let mut app = build_app_with_mock_remotes(vec![("buildbox", empty_snapshot())]);
    app.bootstrap_backend_views().await;
    assert!(
        !app.ui_state.project_pull_blocked.contains_key(&pid),
        "no badge before the remote's blocked-pull snapshot has landed"
    );

    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(1).0,
        snapshot: Box::new(remote_snap),
        states: Box::new(crate::api::AgentStatesSnapshot {
            states: Default::default(),
            commander_running: false,
        }),
    })
    .await;

    assert!(
        app.ui_state.project_pull_blocked.contains_key(&pid),
        "folding a remote snapshot with a blocked pull must surface its badge"
    );
}

#[tokio::test]
async fn local_connection_degrades_from_tmux_ok_false_and_stays_degraded() {
    // A local snapshot fetched while tmux is down reports `server.tmux_ok=false`.
    // Folding it must degrade the local header — and a subsequent fold (tmux
    // still down) must NOT flip it back to Connected. Against HEAD the fold set
    // `connection = Connected` unconditionally, so the first fold already
    // un-gated local commands: red.
    let mut app = make_test_app();
    let degraded_snap = empty_snapshot();
    assert!(
        !degraded_snap.server.tmux_ok,
        "fixture precondition: empty_snapshot reports tmux down"
    );
    let states = || {
        Box::new(crate::api::AgentStatesSnapshot {
            states: Default::default(),
            commander_running: false,
        })
    };

    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(0).0,
        snapshot: Box::new(degraded_snap),
        states: states(),
    })
    .await;
    assert!(
        matches!(
            app.local_view().connection,
            ConnectionState::Degraded { .. }
        ),
        "tmux_ok=false must degrade the local header, got {:?}",
        app.local_view().connection
    );

    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(0).0,
        snapshot: Box::new(empty_snapshot()),
        states: states(),
    })
    .await;
    assert!(
        matches!(
            app.local_view().connection,
            ConnectionState::Degraded { .. }
        ),
        "a later fold with tmux still down must not un-degrade the local header, got {:?}",
        app.local_view().connection
    );
}

#[tokio::test]
async fn remote_connection_stays_watch_owned_across_snapshot_fold() {
    // A remote's health is owned by its connection-watch task. Once the watch
    // reports Degraded, a snapshot fold (e.g. a slow in-flight fetch completing
    // after the poller gave up) must NOT resurrect the header to Connected.
    // Against HEAD the fold set `connection = Connected` for every backend: red.
    let (remote_snap, _sid, _pid) = snapshot_with_one_session();
    assert!(
        remote_snap.server.tmux_ok,
        "fixture precondition: the remote snapshot reports tmux up"
    );
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap.clone())]);
    app.bootstrap_backend_views().await;

    app.handle_state_update(StateUpdate::BackendConnection {
        backend_id: BackendId(1).0,
        state: ConnectionState::Degraded {
            reason: "connection refused".to_string(),
        },
    })
    .await;

    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(1).0,
        snapshot: Box::new(remote_snap),
        states: Box::new(crate::api::AgentStatesSnapshot {
            states: Default::default(),
            commander_running: false,
        }),
    })
    .await;

    match &app.backend(BackendId(1)).unwrap().view.connection {
        ConnectionState::Degraded { reason } => assert_eq!(reason, "connection refused"),
        other => panic!("remote connection must stay watch-owned Degraded, got {other:?}"),
    }
}

#[tokio::test]
async fn selection_falls_back_to_local_when_backend_removed() {
    let (remote_snap, sid, pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_list_items().await;
    // Select the remote session.
    app.ui_state.selected_session_id = Some(SessionRef::new(BackendId(1), sid));
    app.ui_state.selected_project_id = Some((BackendId(1), pid));
    app.ui_state.selected_backend_connected = true;

    // Hot-reload removes the server.
    let old = app.config.remote_servers.clone();
    app.apply_remote_servers_reload(&old, &[]);

    assert!(
        app.backend(BackendId(1)).is_none(),
        "removed backend's handle should be gone"
    );
    assert_eq!(
        app.ui_state.selected_session_id, None,
        "selection on a removed backend should fall back to local (cleared)"
    );
    assert!(app.ui_state.selected_backend_connected);
}

#[tokio::test]
async fn hot_reload_adds_new_backend_handle() {
    let mut app = build_app_with_mock_remotes(vec![("buildbox", empty_snapshot())]);
    app.bootstrap_backend_views().await;
    let old = app.config.remote_servers.clone();
    let new = vec![
        crate::config::RemoteServerConfig {
            name: "buildbox".to_string(),
            url: "http://buildbox:7878".to_string(),
            token: None,
        },
        crate::config::RemoteServerConfig {
            name: "ci".to_string(),
            url: "http://ci:7878".to_string(),
            token: None,
        },
    ];
    app.config.remote_servers = new.clone();
    app.apply_remote_servers_reload(&old, &new);

    // buildbox kept its id; ci got a fresh one.
    assert_eq!(
        app.backend(BackendId(1)).unwrap().backend.descriptor().name,
        "buildbox"
    );
    let names: Vec<String> = app
        .backends
        .iter()
        .map(|h| h.backend.descriptor().name)
        .collect();
    assert_eq!(names, vec!["local", "buildbox", "ci"]);
}

#[test]
fn reconcile_remote_servers_detects_add_remove_change() {
    let cfg = |name: &str, url: &str| crate::config::RemoteServerConfig {
        name: name.to_string(),
        url: url.to_string(),
        token: None,
    };
    let old = vec![cfg("a", "http://a:1"), cfg("b", "http://b:1")];
    // a unchanged, b's url changed, c added, (b removed+added via change).
    let new = vec![
        cfg("a", "http://a:1"),
        cfg("b", "http://b:2"),
        cfg("c", "http://c:1"),
    ];
    let recon = reconcile_remote_servers(&old, &new);
    assert_eq!(recon.removed, vec!["b".to_string()]);
    let added: Vec<String> = recon.added.iter().map(|s| s.name.clone()).collect();
    assert_eq!(added, vec!["b".to_string(), "c".to_string()]);
}

#[tokio::test]
async fn attach_target_backend_routes_to_session_owner() {
    let (remote_snap, sid, _pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;

    // A session target routes to the ref's backend; the switcher gate then
    // reflects that backend's capabilities (a remote mock has no switcher).
    let remote_target = AttachTarget::Session {
        session: SessionRef::new(BackendId(1), sid),
        kind: crate::backend::AttachKind::Agent,
    };
    assert_eq!(app.attach_target_backend(&remote_target), BackendId(1));
    assert!(
        !app.backend(BackendId(1))
            .unwrap()
            .backend
            .capabilities()
            .switcher_popup,
        "remote backend has no in-session switcher"
    );

    // A name-only target (commander / project shell) is local.
    let local_target = AttachTarget::LocalName("cc-commander".to_string());
    assert_eq!(app.attach_target_backend(&local_target), LOCAL_BACKEND_ID);
}

#[test]
fn reconcile_remote_servers_reorder_is_noop() {
    let cfg = |name: &str| crate::config::RemoteServerConfig {
        name: name.to_string(),
        url: format!("http://{name}:1"),
        token: None,
    };
    let old = vec![cfg("a"), cfg("b")];
    let new = vec![cfg("b"), cfg("a")];
    let recon = reconcile_remote_servers(&old, &new);
    assert!(recon.added.is_empty());
    assert!(recon.removed.is_empty());
}

// ---------------------------------------------------------------------------
// Add/remove remote server palette flows (Phase G)
// ---------------------------------------------------------------------------

fn server_cfg(name: &str, url: &str) -> crate::config::RemoteServerConfig {
    crate::config::RemoteServerConfig {
        name: name.to_string(),
        url: url.to_string(),
        token: None,
    }
}

#[tokio::test]
async fn add_remote_server_flow_chains_name_url_token() {
    let mut app = make_test_app();
    app.handle_add_remote_server();
    assert!(matches!(
        &app.ui_state.modal,
        Modal::Input {
            on_submit: InputAction::AddRemoteServerName,
            mask: false,
            ..
        }
    ));

    // Name → URL step.
    app.handle_input_submit(
        InputAction::AddRemoteServerName,
        "buildbox".into(),
        None,
        None,
    )
    .await;
    assert!(matches!(
        &app.ui_state.modal,
        Modal::Input {
            on_submit: InputAction::AddRemoteServerUrl { .. },
            mask: false,
            ..
        }
    ));

    // Invalid URL is rejected and the URL step re-opens with the entry kept.
    app.handle_input_submit(
        InputAction::AddRemoteServerUrl {
            name: "buildbox".into(),
        },
        "not a url".into(),
        None,
        None,
    )
    .await;
    match &app.ui_state.modal {
        Modal::Input {
            on_submit: InputAction::AddRemoteServerUrl { name },
            value,
            ..
        } => {
            assert_eq!(name, "buildbox");
            assert_eq!(value.value(), "not a url");
        }
        other => panic!("expected URL re-prompt, got {other:?}"),
    }
    assert!(app.ui_state.status_message.is_some());

    // Valid URL → masked token step.
    app.handle_input_submit(
        InputAction::AddRemoteServerUrl {
            name: "buildbox".into(),
        },
        "http://buildbox:7878".into(),
        None,
        None,
    )
    .await;
    assert!(matches!(
        &app.ui_state.modal,
        Modal::Input {
            on_submit: InputAction::AddRemoteServerToken { .. },
            mask: true,
            ..
        }
    ));

    // Token submission kicks off the probe (Loading modal).
    app.handle_input_submit(
        InputAction::AddRemoteServerToken {
            name: "buildbox".into(),
            url: "http://buildbox:7878".into(),
        },
        "sekrit-token".into(),
        None,
        None,
    )
    .await;
    assert!(matches!(&app.ui_state.modal, Modal::Loading { .. }));
}

#[tokio::test]
async fn add_remote_server_duplicate_name_reprompts() {
    let mut app = make_test_app();
    app.config.remote_servers = vec![server_cfg("buildbox", "http://b:7878")];
    app.handle_input_submit(
        InputAction::AddRemoteServerName,
        "buildbox".into(),
        None,
        None,
    )
    .await;
    assert!(matches!(
        &app.ui_state.modal,
        Modal::Input {
            on_submit: InputAction::AddRemoteServerName,
            ..
        }
    ));
    assert!(app.ui_state.status_message.is_some());
}

#[tokio::test]
async fn probe_success_persists_server_and_wires_backend() {
    let mut app = make_test_app();
    app.ui_state.modal = Modal::Loading {
        title: String::new(),
        message: String::new(),
        hint: None,
    };
    let backends_before = app.backends.len();
    app.handle_state_update(StateUpdate::RemoteServerProbed {
        nonce: app.probe_nonce,
        server: server_cfg("buildbox", "http://buildbox:7878"),
        result: Ok(true),
    })
    .await;
    assert!(matches!(app.ui_state.modal, Modal::None));
    // Persisted to config (both the live cache and the store's copy)…
    assert_eq!(app.config.remote_servers.len(), 1);
    assert_eq!(app.service.read_config().remote_servers.len(), 1);
    // …and a live handle exists (a degraded placeholder here, since the test
    // factory refuses construction — the shape the tree renders either way).
    assert_eq!(app.backends.len(), backends_before + 1);
}

#[tokio::test]
async fn probe_failure_offers_save_anyway_which_persists() {
    let mut app = make_test_app();
    app.ui_state.modal = Modal::Loading {
        title: String::new(),
        message: String::new(),
        hint: None,
    };
    app.handle_state_update(StateUpdate::RemoteServerProbed {
        nonce: app.probe_nonce,
        server: server_cfg("buildbox", "http://buildbox:7878"),
        result: Err("connection refused".into()),
    })
    .await;
    let confirm = match &app.ui_state.modal {
        Modal::Confirm {
            message,
            on_confirm: ConfirmAction::AddRemoteServerAnyway { server },
            ..
        } => {
            assert!(message.contains("connection refused"));
            server.clone()
        }
        other => panic!("expected save-anyway confirm, got {other:?}"),
    };
    app.handle_confirm(ConfirmAction::AddRemoteServerAnyway { server: confirm })
        .await;
    assert_eq!(app.config.remote_servers.len(), 1);
}

#[tokio::test]
async fn probe_result_ignored_when_flow_dismissed() {
    let mut app = make_test_app();
    // No Loading modal up — the user cancelled; a late probe result must not
    // write config or open modals.
    app.handle_state_update(StateUpdate::RemoteServerProbed {
        nonce: app.probe_nonce,
        server: server_cfg("buildbox", "http://buildbox:7878"),
        result: Ok(true),
    })
    .await;
    assert!(matches!(app.ui_state.modal, Modal::None));
    assert!(app.config.remote_servers.is_empty());
}

#[tokio::test]
async fn remove_remote_server_empty_config_reports_nothing_to_do() {
    let mut app = make_test_app();
    app.handle_remove_remote_server();
    assert!(matches!(app.ui_state.modal, Modal::None));
    let (msg, _) = app.ui_state.status_message.clone().unwrap();
    assert!(msg.contains("No remote servers"));
}

#[tokio::test]
async fn remove_remote_server_picker_confirm_removes_from_config() {
    let mut app = make_test_app();
    // Seed via the same write path the add flow uses so the store copy and
    // live cache agree.
    app.add_remote_server_to_config(server_cfg("buildbox", "http://b:7878"))
        .unwrap();
    assert_eq!(app.backends.len(), 2);

    app.handle_remove_remote_server();
    match &app.ui_state.modal {
        Modal::QuickSwitch { mode, matches, .. } => {
            assert!(matches!(mode, PaletteMode::RemoteServerPicker));
            assert_eq!(matches.len(), 1);
        }
        other => panic!("expected picker, got {other:?}"),
    }

    app.handle_confirm(ConfirmAction::RemoveRemoteServer {
        name: "buildbox".into(),
    })
    .await;
    assert!(app.config.remote_servers.is_empty());
    assert!(app.service.read_config().remote_servers.is_empty());
    assert_eq!(app.backends.len(), 1, "backend handle dropped");
}

#[test]
fn remote_server_picker_items_filter_by_name_and_url() {
    let mut app = make_test_app();
    app.config.remote_servers = vec![
        server_cfg("buildbox", "http://tail:7878"),
        server_cfg("laptop", "http://lap:7878"),
    ];
    let all = app.gather_remote_server_picker_items("");
    assert_eq!(all.len(), 2);
    let by_name = app.gather_remote_server_picker_items("build");
    assert_eq!(by_name.len(), 1);
    let by_url = app.gather_remote_server_picker_items("lap:7878");
    assert_eq!(by_url.len(), 1);
    match &by_url[0] {
        QuickSwitchItem::RemoteServerRemove { name, label } => {
            assert_eq!(name, "laptop");
            assert!(label.contains("http://lap:7878"));
        }
        other => panic!("unexpected item {other:?}"),
    }
}

#[test]
fn masked_input_modal_renders_bullets_not_the_token() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    app.ui_state.modal = Modal::Input {
        title: "Add Remote Server".to_string(),
        prompt: "Bearer token:".to_string(),
        value: "sekrit-token".into(),
        on_submit: InputAction::AddRemoteServerToken {
            name: "b".into(),
            url: "http://b:7878".into(),
        },
        existing_branches: None,
        project_picker: None,
        program_picker: None,
        server_picker: None,
        section_picker: None,
        focus: InputFocus::Name,
        expanded: false,
        mask: true,
    };
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(&terminal);
    assert!(!text.contains("sekrit-token"), "token leaked to screen");
    assert!(text.contains(&"•".repeat("sekrit-token".len())));
}

// ---------------------------------------------------------------------------
// Review fixes: bootstrap non-blocking, cascade routing, deterministic maps
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bootstrap_skips_remote_backends_so_a_dead_server_cannot_block_startup() {
    let mut app = build_app_with_mock_remotes(vec![("deadbox", empty_snapshot())]);
    // A downed server: every query fails. If bootstrap awaited it, the view
    // would flip to Degraded here (and a real backend would block for its
    // full connect timeout before first draw).
    app.backend(BackendId(1))
        .unwrap()
        .backend
        .as_any()
        .downcast_ref::<MockBackend>()
        .unwrap()
        .set_failing(true);

    app.bootstrap_backend_views().await;

    // Local bootstrapped; the remote was never queried — still Connecting,
    // waiting on its poller, exactly what the tree renders at first draw.
    assert!(matches!(
        app.backend(BackendId(0)).unwrap().view.connection,
        ConnectionState::Connected
    ));
    assert!(matches!(
        app.backend(BackendId(1)).unwrap().view.connection,
        ConnectionState::Connecting
    ));
}

#[tokio::test]
async fn cascade_resume_targets_the_paused_backend_not_local() {
    let (mut snap, sid, _pid) = snapshot_with_one_session();
    snap.cascade_paused = Some(sid);
    let mut app = build_app_with_mock_remotes(vec![("buildbox", snap)]);
    app.bootstrap_backend_views().await;
    // Also fetch the remote view (bootstrap skips remotes by design).
    app.refresh_backend_view(BackendId(1)).await;

    let (backend_id, paused_sid) = app
        .paused_cascade_backend()
        .expect("remote paused cascade must be found");
    assert_eq!(
        backend_id,
        BackendId(1),
        "resume must route to the paused backend"
    );
    assert_eq!(paused_sid, sid);
}

#[tokio::test]
async fn cascade_resume_prefers_the_selections_backend_when_multiple_paused() {
    let (mut remote_snap, remote_sid, _pid) = snapshot_with_one_session();
    remote_snap.cascade_paused = Some(remote_sid);
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;

    // Local also paused; the user's selection sits on local.
    let local_sid = SessionId::new();
    app.backend_mut_for_test(BackendId(0))
        .view
        .snapshot
        .cascade_paused = Some(local_sid);
    app.ui_state.selected_session_id = Some(SessionRef::local(local_sid));

    let (backend_id, paused_sid) = app.paused_cascade_backend().unwrap();
    assert_eq!(backend_id, BackendId(0));
    assert_eq!(paused_sid, local_sid);
}

#[tokio::test]
async fn ai_summary_routes_to_owning_backend_not_local() {
    // A remote-backed session's AI summary must query the backend that owns it
    // (which serves branch-diff over the wire), not the local backend — where
    // the session id doesn't exist and the query would fail with a local error.
    let (remote_snap, remote_sid, _pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    // Fill the remote view so `backend_of_session` finds the session there.
    app.refresh_backend_view(BackendId(1)).await;
    app.config.ai_summary_enabled = true;

    app.spawn_ai_summary_if_needed(remote_sid);

    // The spawned task queries the remote `MockBackend::branch_diff`, which
    // returns `Unavailable { reason: "unimplemented in mock" }` — a signature
    // only the mock produces. Had the fetch gone to the local backend, the error
    // would be a local one (the session isn't in the local store).
    let ev = app.event_loop.next().await.expect("summary event");
    match ev {
        AppEvent::StateUpdate(StateUpdate::AiSummaryReady {
            session_id, result, ..
        }) => {
            assert_eq!(session_id, remote_sid);
            let err = result.expect_err("mock branch_diff errs, so no summary text");
            assert!(
                err.contains("unimplemented in mock"),
                "summary must query the remote backend, got: {err}"
            );
        }
        other => panic!("expected AiSummaryReady, got {other:?}"),
    }
}

#[test]
fn open_in_editor_hidden_for_remote_backed_selection() {
    // A session on a backend that can't drive the operator's local editor must
    // not offer OpenInEditor in the palette.
    let mut ui = AppUiState {
        selected_session_id: Some(SessionRef::new(BackendId(1), SessionId::new())),
        selected_project_id: Some((BackendId(1), ProjectId::new())),
        selected_backend_connected: true,
        selected_backend_capabilities: crate::backend::BackendCapabilities {
            open_editor: false,
            ..crate::backend::BackendCapabilities::LOCAL
        },
        ..AppUiState::default()
    };
    assert!(!ui.is_command_available(BindableAction::OpenInEditor));
    // A backend that can drive the local editor keeps it available.
    ui.selected_backend_capabilities = crate::backend::BackendCapabilities::LOCAL;
    assert!(ui.is_command_available(BindableAction::OpenInEditor));
}

#[tokio::test]
async fn open_in_editor_toasts_for_remote_session_instead_of_launching() {
    let (remote_snap, remote_sid, remote_pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;
    app.ui_state.selected_session_id = Some(SessionRef::new(BackendId(1), remote_sid));
    app.ui_state.selected_project_id = Some((BackendId(1), remote_pid));

    app.handle_open_in_editor().await;

    assert!(
        app.ui_state.editor_command.is_none(),
        "must not queue a local editor launch for a remote session"
    );
    assert!(
        !app.ui_state.should_quit,
        "must not tear down the TUI to launch an editor"
    );
    let (msg, _) = app
        .ui_state
        .status_message
        .clone()
        .expect("a toast explaining the editor is unavailable");
    assert!(msg.contains("not available for remote"), "toast: {msg}");
}

#[tokio::test]
async fn select_shell_toasts_for_remote_project_instead_of_local_lookup() {
    let (remote_snap, _sid, remote_pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;
    // A remote project row is selected (no session).
    app.ui_state.selected_project_id = Some((BackendId(1), remote_pid));
    app.ui_state.selected_session_id = None;

    app.handle_select_shell().await;

    assert!(
        app.ui_state.attach_request.is_none(),
        "must not queue an attach for a remote project shell"
    );
    assert!(
        !matches!(app.ui_state.modal, Modal::Error { .. }),
        "must not surface the confusing local-lookup error modal"
    );
    let (msg, _) = app
        .ui_state
        .status_message
        .clone()
        .expect("a toast explaining the shell is unavailable");
    assert!(msg.contains("not available for remote"), "toast: {msg}");
}

#[tokio::test]
async fn stale_probe_result_ignored_while_unrelated_loading_modal_up() {
    // The exact scenario the probe-nonce check exists for: a STALE probe result
    // arriving while an unrelated Loading modal is up must NOT write config.
    let mut app = make_test_app();
    app.ui_state.modal = Modal::Loading {
        title: "Something else".to_string(),
        message: String::new(),
        hint: None,
    };
    app.handle_state_update(StateUpdate::RemoteServerProbed {
        nonce: app.probe_nonce.wrapping_sub(1), // stale — from a prior/aborted flow
        server: server_cfg("buildbox", "http://buildbox:7878"),
        result: Ok(true),
    })
    .await;
    assert!(
        app.config.remote_servers.is_empty(),
        "a stale probe result must not persist a server"
    );
    assert!(
        matches!(&app.ui_state.modal, Modal::Loading { title, .. } if title == "Something else"),
        "the unrelated Loading modal must be left intact"
    );
}

#[tokio::test]
async fn checkout_branch_lists_remote_project_branches_via_backend() {
    // Opening the Checkout modal on a remote project must route through the
    // owning backend's `list_branches` — not the local-only gix path, which
    // would fail "Project not found" for a project the local backend doesn't
    // know about.
    let (remote_snap, _sid, remote_pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;

    app.backend(BackendId(1))
        .unwrap()
        .backend
        .as_any()
        .downcast_ref::<MockBackend>()
        .unwrap()
        .set_branches(vec![
            crate::api::BranchInfo {
                name: "main".to_string(),
                is_remote: false,
            },
            crate::api::BranchInfo {
                name: "origin/feature-x".to_string(),
                is_remote: true,
            },
        ]);

    app.ui_state.selected_project_id = Some((BackendId(1), remote_pid));
    app.handle_checkout_branch().await;

    // The modal opens immediately with an empty, spinning list; the initial
    // listing and fetch-refresh arrive on background tasks. Drive the events the
    // spawned task posts until the list is populated.
    for _ in 0..10 {
        if matches!(&app.ui_state.modal, Modal::CheckoutBranch { all_branches, .. } if !all_branches.is_empty())
        {
            break;
        }
        match app.event_loop.next().await.expect("a checkout event") {
            AppEvent::StateUpdate(su @ StateUpdate::CheckoutBranchesLoaded { .. })
            | AppEvent::StateUpdate(su @ StateUpdate::CheckoutFetchComplete { .. }) => {
                app.handle_state_update(su).await;
            }
            _ => continue,
        }
    }

    match &app.ui_state.modal {
        Modal::CheckoutBranch { all_branches, .. } => {
            let names: Vec<&str> = all_branches.iter().map(|b| b.local_name.as_str()).collect();
            assert!(names.contains(&"main"), "local branch listed: {names:?}");
            assert!(
                names.contains(&"feature-x"),
                "remote-only branch listed: {names:?}"
            );
        }
        other => panic!("expected CheckoutBranch modal, got {other:?}"),
    }
}

#[tokio::test]
async fn checkout_branch_submits_against_remote_backend() {
    // Pressing Enter in the Checkout modal on a remote project must spawn the
    // create against the *owning* backend, resolving the project's repo path
    // from that backend's snapshot — not the local view (which would 404 with
    // "Project not found").
    let (remote_snap, _sid, remote_pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;

    // Drive the create through the same submission entry point the Enter key
    // uses, so a regression in project resolution is caught end-to-end.
    app.start_checkout_session(remote_pid, "feature-x".to_string())
        .await;

    assert!(
        !matches!(&app.ui_state.modal, Modal::Error { .. }),
        "remote checkout must not raise a 'Project not found' error modal"
    );

    let mock = app
        .backend(BackendId(1))
        .unwrap()
        .backend
        .as_any()
        .downcast_ref::<MockBackend>()
        .unwrap();
    let mut created = None;
    for _ in 0..50 {
        if let Some(opts) = mock.created_sessions().into_iter().next() {
            created = Some(opts);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let created = created.expect("create must be spawned against the remote backend");
    assert_eq!(
        created.project_path,
        std::path::PathBuf::from("/tmp/rp"),
        "the remote project's repo_path must be used"
    );
    assert_eq!(
        created.base_branch.as_deref(),
        Some("feature-x"),
        "the checked-out branch must be the base branch"
    );
}

#[tokio::test]
async fn delete_merged_pr_sessions_sweeps_remote_backends() {
    // A merged-PR session living on a remote backend must be swept by the bulk
    // "Delete merged-PR sessions" command — candidates come from every backend
    // view, and the delete routes to the owning backend.
    let (mut remote_snap, remote_sid, _pid) = snapshot_with_one_session();
    remote_snap.sessions[0].pr_merged = true;
    remote_snap.sessions[0].pr_state = crate::git::PrState::Merged;
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;

    app.handle_delete_merged_pr_sessions().await;
    match &app.ui_state.modal {
        Modal::Confirm {
            on_confirm: ConfirmAction::DeleteMergedPrSessions { session_ids },
            ..
        } => assert_eq!(
            session_ids,
            &vec![remote_sid],
            "the remote merged-PR session must be a delete candidate"
        ),
        other => panic!("expected merged-PR confirm, got {other:?}"),
    }

    app.handle_confirm(ConfirmAction::DeleteMergedPrSessions {
        session_ids: vec![remote_sid],
    })
    .await;

    // The delete is spawned; poll the mock's recorded deletes.
    let mock = app
        .backend(BackendId(1))
        .unwrap()
        .backend
        .as_any()
        .downcast_ref::<MockBackend>()
        .unwrap();
    let mut deleted = false;
    for _ in 0..50 {
        if mock.deleted_sessions().contains(&remote_sid) {
            deleted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        deleted,
        "the remote backend must have been asked to delete the merged-PR session"
    );
}

#[tokio::test]
async fn refresh_pr_status_fans_out_to_connected_backends_only() {
    let mut app = build_app_with_mock_remotes(vec![
        ("connected", empty_snapshot()),
        ("degraded", empty_snapshot()),
    ]);
    app.bootstrap_backend_views().await;
    app.backend_mut_for_test(BackendId(1)).view.connection = ConnectionState::Connected;
    app.backend_mut_for_test(BackendId(2)).view.connection = ConnectionState::Degraded {
        reason: "down".to_string(),
    };

    app.refresh_pr_status_all();

    let count = |id| {
        app.backend(id)
            .unwrap()
            .backend
            .as_any()
            .downcast_ref::<MockBackend>()
            .unwrap()
            .pr_refresh_count()
    };
    // The fan-out is spawned off the event loop; poll for the connected remote's
    // refresh to land.
    let mut got = false;
    for _ in 0..50 {
        if count(BackendId(1)) == 1 {
            got = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(got, "connected remote gets the refresh");
    assert_eq!(count(BackendId(2)), 0, "degraded remote is skipped");
}

#[tokio::test]
async fn palette_includes_remote_backend_sessions() {
    let (remote_snap, remote_sid, _pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;

    let matches = app.gather_quick_switch_matches("").await;
    assert!(
        matches.iter().any(|m| m.session_id == remote_sid),
        "the quick-switch palette must include remote-backend sessions"
    );
}

/// Build a snapshot holding several sessions with the given `(title,
/// last_attached_at)` pairs, all under one project.
fn snapshot_with_attach_times(
    sessions: &[(&str, Option<chrono::DateTime<chrono::Utc>>)],
) -> (WorkspaceSnapshot, Vec<SessionId>) {
    use crate::session::{Project, SessionStatus, WorktreeSession};
    let mut state = crate::config::AppState::default();
    let mut project = Project::new("proj", std::path::PathBuf::from("/tmp/p"), "main");
    let pid = project.id;
    let mut ids = Vec::new();
    for (title, attached) in sessions {
        let mut s = WorktreeSession::new(pid, *title, *title, std::path::PathBuf::new(), "claude");
        s.status = SessionStatus::Running;
        s.last_attached_at = *attached;
        let id = s.id;
        project.add_worktree(id);
        state.sessions.insert(id, s);
        ids.push(id);
    }
    state.projects.insert(pid, project);
    (crate::api::workspace_snapshot_from_state(&state), ids)
}

/// Both palette build paths (initial `gather_quick_switch_matches` and the
/// per-keystroke `refilter_quick_switch`) must order an empty query by
/// most-recent attach, newest first, with never-attached sessions last.
#[tokio::test]
async fn quick_switch_empty_query_orders_by_recency() {
    use chrono::Duration;

    let now = chrono::Utc::now();
    let (snap, ids) = snapshot_with_attach_times(&[
        ("alpha", Some(now - Duration::minutes(5))),
        ("bravo", Some(now - Duration::minutes(1))),
        ("charlie", Some(now - Duration::minutes(10))),
        ("delta-never", None),
    ]);
    let (alpha, bravo, charlie, never) = (ids[0], ids[1], ids[2], ids[3]);

    let mut app = build_app_with_mock_remotes(vec![("box", snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;

    // Path 1: initial open.
    let matches = app.gather_quick_switch_matches("").await;
    let ids: Vec<SessionId> = matches.iter().map(|m| m.session_id).collect();
    assert_eq!(
        ids,
        vec![bravo, alpha, charlie, never],
        "gather_quick_switch_matches must rank empty query by recency, never-attached last"
    );

    // Path 2: per-keystroke refilter, which builds from list_items.
    app.refresh_list_items().await;
    app.open_quick_switch_with_mode(PaletteMode::Unified).await;
    app.refilter_quick_switch();
    let Modal::QuickSwitch { matches, .. } = &app.ui_state.modal else {
        panic!("expected quick-switch modal");
    };
    let ids: Vec<SessionId> = matches
        .iter()
        .filter_map(|m| match m {
            QuickSwitchItem::Session(s) => Some(s.session_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        vec![bravo, alpha, charlie, never],
        "refilter_quick_switch must rank empty query by recency, never-attached last"
    );
}

#[tokio::test]
async fn session_id_by_tmux_name_resolves_against_remote_view() {
    let (remote_snap, remote_sid, _pid) = snapshot_with_one_session();
    let tmux_name = remote_snap.sessions[0].tmux_session_name.clone();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;

    // Given the attached (remote) backend, the name resolves against its view —
    // Alt-r inside a remote attach opens the right session's review.
    assert_eq!(
        app.session_id_by_tmux_name(BackendId(1), &tmux_name),
        Some(remote_sid),
    );
}

#[tokio::test]
async fn new_session_disables_local_branch_hint_for_remote_project() {
    let (remote_snap, _sid, remote_pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;
    app.ui_state.selected_project_id = Some((BackendId(1), remote_pid));

    app.handle_new_session().await;

    match &app.ui_state.modal {
        Modal::Input {
            existing_branches,
            project_picker,
            ..
        } => {
            assert!(
                existing_branches.is_none(),
                "no local branch hint for a remote project"
            );
            assert!(
                !project_picker
                    .as_ref()
                    .expect("new-session dialog has a project picker")
                    .branch_hint_enabled,
                "a remote project's picker must not run the local gix hint on navigation"
            );
        }
        other => panic!("expected New Session Input modal, got {other:?}"),
    }
}

#[tokio::test]
async fn new_session_shows_server_field_and_switch_rebuilds_pickers() {
    let (remote_snap, _sid, remote_pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;
    // Open the dialog on the remote project.
    app.ui_state.selected_project_id = Some((BackendId(1), remote_pid));
    app.handle_new_session().await;

    // Two backends → the server field is present and defaults to the current
    // (remote) backend; the remote project's picker disables the local hint.
    match &app.ui_state.modal {
        Modal::Input {
            server_picker: Some(sp),
            project_picker: Some(pp),
            section_picker,
            ..
        } => {
            assert_eq!(sp.selected_backend(), Some(BackendId(1)));
            assert!(!pp.branch_hint_enabled, "remote project picker");
            assert!(section_picker.is_some());
        }
        other => panic!("expected New Session modal with a server picker, got {other:?}"),
    }

    // Point the server picker at the local backend and apply the change.
    if let Modal::Input {
        server_picker: Some(sp),
        ..
    } = &mut app.ui_state.modal
    {
        sp.selected = sp
            .choices
            .iter()
            .position(|(id, _)| *id == LOCAL_BACKEND_ID)
            .expect("local backend is in the picker");
    }
    app.on_new_session_server_changed().await;

    // The project picker is rebuilt for the local backend (hint re-enabled) and
    // focus stays on the server field.
    match &app.ui_state.modal {
        Modal::Input {
            project_picker: Some(pp),
            section_picker: Some(_),
            focus,
            ..
        } => {
            assert!(
                pp.branch_hint_enabled,
                "a local project picker re-enables the gix branch hint"
            );
            assert_eq!(
                *focus,
                InputFocus::Server,
                "focus stays on the server field"
            );
        }
        other => panic!("expected a rebuilt New Session modal, got {other:?}"),
    }
}

#[tokio::test]
async fn server_switch_rekeys_pending_action_to_new_backend_project() {
    // Regression (M1/M2): switching the Server field must re-key `on_submit`'s
    // project to the newly selected backend's project and clear the old backend's
    // section — otherwise the async remote swap-in (keyed on `on_submit`) can
    // never correlate to the dialog, and a stale in-flight response would stomp
    // it. Two remotes so both backends have projects with distinct ids.
    let (snap1, _s1, pid1) = snapshot_with_one_session();
    let (snap2, _s2, pid2) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("r1", snap1), ("r2", snap2)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;
    app.refresh_backend_view(BackendId(2)).await;
    app.ui_state.selected_project_id = Some((BackendId(1), pid1));
    app.handle_new_session().await;

    // Switch the Server field to r2 and apply.
    if let Modal::Input {
        server_picker: Some(sp),
        ..
    } = &mut app.ui_state.modal
    {
        sp.selected = sp
            .choices
            .iter()
            .position(|(id, _)| *id == BackendId(2))
            .expect("r2 is in the picker");
    }
    app.on_new_session_server_changed().await;

    match &app.ui_state.modal {
        Modal::Input {
            on_submit:
                InputAction::CreateSession {
                    project_id,
                    section,
                },
            project_picker: Some(pp),
            ..
        } => {
            assert_eq!(*project_id, pid2, "pending action re-keyed to r2's project");
            assert!(
                section.is_none(),
                "old backend's section is cleared on switch"
            );
            assert_eq!(
                pp.selected_id(),
                Some(pid2),
                "project picker holds r2's project"
            );
        }
        other => panic!("expected a re-keyed CreateSession modal, got {other:?}"),
    }
}

/// Build a New Session `Modal::Input` for the swap-in handler tests: a
/// `CreateSession` action targeting `project_id` with `section`, a program picker
/// (local fallback), and an as-yet-unfilled section picker (catch-all only, as a
/// remote backend's dialog opens before `create_options` returns).
fn open_create_session_modal(project_id: ProjectId, section: Option<String>) -> Modal {
    Modal::Input {
        title: "New Session".to_string(),
        prompt: "Enter session name:".to_string(),
        value: super::Input::default(),
        on_submit: InputAction::CreateSession {
            project_id,
            section,
        },
        existing_branches: None,
        project_picker: None,
        program_picker: Some(ProgramPicker {
            choices: vec![crate::config::ProgramEntry {
                label: "bash".to_string(),
                command: "bash".to_string(),
            }],
            selected: 0,
        }),
        server_picker: None,
        section_picker: Some(SectionPicker::new(Vec::new(), None)),
        focus: InputFocus::Name,
        expanded: false,
        mask: false,
    }
}

#[tokio::test]
async fn remote_options_swap_in_applies_when_correlated_and_preserves_cursor_section() {
    // Regression (M1 + M3): a correlated `NewSessionProgramsLoaded` fills in the
    // remote's programs and sections, and the section picker keeps the section
    // baked into the pending action (the cursor-derived default) rather than
    // resetting to the catch-all.
    let mut app = make_test_app();
    let pid = ProjectId::new();
    app.ui_state.modal = open_create_session_modal(pid, Some("Open PRs".to_string()));

    app.handle_state_update(StateUpdate::NewSessionProgramsLoaded {
        project_id: pid,
        picker: Some(ProgramPicker {
            choices: vec![crate::config::ProgramEntry {
                label: "claude".to_string(),
                command: "claude".to_string(),
            }],
            selected: 0,
        }),
        sections: vec!["Open PRs".to_string(), "Merged".to_string()],
    })
    .await;

    match &app.ui_state.modal {
        Modal::Input {
            program_picker: Some(prog),
            section_picker: Some(sec),
            ..
        } => {
            assert_eq!(
                prog.selected_command().as_deref(),
                Some("claude"),
                "remote program list swapped in"
            );
            assert!(sec.choices.len() > 1, "remote sections swapped in");
            assert_eq!(
                sec.selected_section().as_deref(),
                Some("Open PRs"),
                "cursor-derived section survives the swap-in"
            );
        }
        other => panic!("expected an updated New Session modal, got {other:?}"),
    }
}

#[tokio::test]
async fn remote_options_swap_in_is_dropped_for_a_different_project() {
    // Regression (M2): a swap-in whose project doesn't match the pending action
    // (e.g. a stale response after the user switched the Server field) must be
    // ignored, not written into a dialog now targeting a different backend.
    let mut app = make_test_app();
    let pid = ProjectId::new();
    let other = ProjectId::new();
    app.ui_state.modal = open_create_session_modal(pid, None);

    app.handle_state_update(StateUpdate::NewSessionProgramsLoaded {
        project_id: other,
        picker: Some(ProgramPicker {
            choices: vec![crate::config::ProgramEntry {
                label: "claude".to_string(),
                command: "claude".to_string(),
            }],
            selected: 0,
        }),
        sections: vec!["Open PRs".to_string()],
    })
    .await;

    match &app.ui_state.modal {
        Modal::Input {
            program_picker: Some(prog),
            section_picker: Some(sec),
            ..
        } => {
            assert_eq!(
                prog.selected_command().as_deref(),
                Some("bash"),
                "uncorrelated response must not replace the program picker"
            );
            assert_eq!(
                sec.choices.len(),
                1,
                "uncorrelated response must not add sections"
            );
        }
        other => panic!("expected the unchanged New Session modal, got {other:?}"),
    }
}

#[tokio::test]
async fn restore_selection_resolves_remembered_remote_backend() {
    // Exercises `last_selected_backend`: the remembered name resolves to the
    // owning backend and the row is restored. (Session ids are globally unique,
    // so this is behaviour-preserving vs. raw-id matching — the field makes the
    // resolution explicit and keeps the read side symmetric with the write.)
    let (remote_snap, remote_sid, _pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;
    app.refresh_list_items().await;

    app.tui_prefs
        .set_selection(Some(remote_sid), None, Some("buildbox".to_string()))
        .await;

    app.restore_selection().await;

    let idx = app
        .ui_state
        .list_state
        .selected()
        .expect("a row is selected");
    match &app.ui_state.list_items[idx] {
        SessionListItem::Worktree { id, .. } => assert_eq!(*id, remote_sid),
        other => panic!("expected the remote session row, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Review-view write paths route to the owning backend, not the local service
// ---------------------------------------------------------------------------

/// The `MockBackend` behind backend `id` in `app`, for call-recording asserts.
fn remote_mock(app: &App, id: BackendId) -> &MockBackend {
    app.backend(id)
        .unwrap()
        .backend
        .as_any()
        .downcast_ref::<MockBackend>()
        .unwrap()
}

/// An `App` with one mock remote whose single session's view is populated, plus
/// the session id, for driving the review view against a remote-owned session.
async fn app_with_remote_session() -> (App, SessionId) {
    let (remote_snap, remote_sid, _pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    // Populate the remote view so `backend_of_session` resolves to it.
    app.refresh_backend_view(BackendId(1)).await;
    (app, remote_sid)
}

/// An `App` whose single session's backend advertises the `open_editor`
/// capability and whose config pins a deterministic terminal (non-GUI) editor,
/// for driving the review view's open-in-editor path. Returns the app, the
/// session id, and the session's worktree path.
async fn app_with_editor_capable_session() -> (App, SessionId, std::path::PathBuf) {
    use crate::session::{Project, SessionStatus, WorktreeSession};
    let mut state = crate::config::AppState::default();
    let mut project = Project::new("proj", std::path::PathBuf::from("/tmp/rp"), "main");
    let pid = project.id;
    let worktree = std::path::PathBuf::from("/tmp/wt/session-a");
    let mut sess = WorktreeSession::new(pid, "sess", "br", worktree.clone(), "claude");
    sess.status = SessionStatus::Running;
    let sid = sess.id;
    project.add_worktree(sid);
    state.projects.insert(pid, project);
    state.sessions.insert(sid, sess);
    let snap = crate::api::workspace_snapshot_from_state(&state);

    let mut app = build_app_with_mock_remotes(vec![("buildbox", snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;
    remote_mock(&app, BackendId(1)).set_open_editor(true);
    app.config.editor = Some("vi".to_string());
    app.config.editor_gui = Some(false);
    (app, sid, worktree)
}

/// A two-file text diff review state for `sid` (first selectable line is a.rs's
/// context line, so `build_draft((0, 0), …)` yields a New-side comment).
fn review_state_for(sid: SessionId) -> Box<DiffReviewState> {
    let diff = crate::git::parse_unified_diff(
        "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,3 @@
 fn main() {
+    let y = 3;
 }
",
    );
    Box::new(DiffReviewState::new(
        sid,
        "t".to_string(),
        "main".to_string(),
        diff,
        Vec::new(),
    ))
}

#[tokio::test]
async fn review_create_comment_routes_to_owning_backend() {
    let (mut app, remote_sid) = app_with_remote_session().await;
    let mut state = review_state_for(remote_sid);
    // Open the comment box over the first selectable line with some text.
    state.comment = Some(super::review::CommentDraft {
        input: Input::from("nit: rename"),
        range: (0, 0),
    });
    let enter = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    );

    app.handle_review_key(enter, state).await;

    assert_eq!(
        remote_mock(&app, BackendId(1)).created_comment_sessions(),
        vec![remote_sid],
        "create_comment must route to the backend that owns the session"
    );
}

#[tokio::test]
async fn review_apply_comments_routes_to_owning_backend() {
    let (mut app, remote_sid) = app_with_remote_session().await;
    let state = review_state_for(remote_sid);
    let apply = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('a'),
        crossterm::event::KeyModifiers::NONE,
    );

    app.handle_review_key(apply, state).await;

    assert_eq!(
        remote_mock(&app, BackendId(1)).applied_comment_sessions(),
        vec![remote_sid],
        "apply_comments must route to the backend that owns the session"
    );
}

#[tokio::test]
async fn review_toggle_file_reviewed_routes_to_owning_backend() {
    let (mut app, remote_sid) = app_with_remote_session().await;
    let state = review_state_for(remote_sid);
    let mark = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('m'),
        crossterm::event::KeyModifiers::NONE,
    );

    app.handle_review_key(mark, state).await;

    assert_eq!(
        remote_mock(&app, BackendId(1)).toggled_reviewed_files(),
        vec![(remote_sid, "a.rs".to_string())],
        "toggle_file_reviewed must route to the owning backend, by display path"
    );
}

#[tokio::test]
async fn review_open_in_editor_key_routes_to_owning_backend() {
    // Pressing the OpenInEditor binding (`.` by default) inside the review view
    // must be intercepted and routed through the shared editor path, honouring
    // the owning backend's capability gate. The mock backend can't drive the
    // local editor, so it toasts rather than launching — proving the key is now
    // wired up in the review view (before the fix it was a no-op).
    let (mut app, remote_sid) = app_with_remote_session().await;
    let state = review_state_for(remote_sid);
    let dot = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('.'),
        crossterm::event::KeyModifiers::NONE,
    );

    app.handle_review_key(dot, state).await;

    assert!(
        app.ui_state.editor_command.is_none(),
        "must not queue a local editor launch for a remote-owned review"
    );
    assert!(
        !app.ui_state.should_quit,
        "must not tear down the TUI to launch an editor"
    );
    let (msg, _) = app
        .ui_state
        .status_message
        .clone()
        .expect("a toast explaining the editor is unavailable");
    assert!(
        msg.contains("not available for remote"),
        "unexpected toast: {msg}"
    );
    // The review view stays open after the key is handled.
    assert!(
        matches!(app.ui_state.modal, Modal::ReviewDiff(_)),
        "review modal must remain open"
    );
}

#[tokio::test]
async fn review_open_in_editor_key_launches_terminal_editor_for_local_session() {
    // A backend that can drive the local editor + a terminal (non-GUI) editor:
    // pressing `.` in the review view queues the editor on that session's own
    // worktree and tears the TUI down to run it, leaving the review restored so
    // it reopens when the editor exits.
    let (mut app, sid, worktree) = app_with_editor_capable_session().await;

    let review = review_state_for(sid);
    let dot = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('.'),
        crossterm::event::KeyModifiers::NONE,
    );

    app.handle_review_key(dot, review).await;

    assert_eq!(
        app.ui_state.editor_command,
        Some(("vi".to_string(), worktree)),
        "must queue the terminal editor on the review session's worktree"
    );
    assert!(
        app.ui_state.should_quit,
        "a terminal editor tears the TUI down to run foreground"
    );
    assert!(
        matches!(app.ui_state.modal, Modal::ReviewDiff(_)),
        "review modal must be restored so it reopens after the editor exits"
    );
}

#[test]
fn review_footer_surfaces_live_status_message() {
    // The review view is a full-screen takeover that never draws the normal
    // status bar, so a status message must appear in its footer instead —
    // otherwise apply/refresh results and editor errors would be invisible.
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    let sid = SessionId::new();
    app.ui_state.modal = Modal::ReviewDiff(review_state_for(sid));
    app.ui_state.status_message = Some((
        "Editor unavailable here".to_string(),
        Instant::now() + Duration::from_secs(3),
    ));

    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    assert!(
        buffer_text(&terminal).contains("Editor unavailable here"),
        "review footer must render the live status message"
    );
}

#[test]
fn review_footer_truncates_a_long_status_message_instead_of_blanking() {
    // A status message wider than the footer must be truncated to fit, not
    // dropped — dropping would blank the footer for the toast's lifetime, the
    // exact invisibility the footer toast exists to prevent (and long messages
    // are usually errors, which matter most).
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = make_test_app();
    app.ui_state.modal = Modal::ReviewDiff(review_state_for(SessionId::new()));
    app.ui_state.status_message = Some((
        "Failed to launch '/usr/local/bin/my-editor': No such file or directory (os error 2)"
            .to_string(),
        Instant::now() + Duration::from_secs(3),
    ));

    // Narrow terminal: the full message can't fit on the footer row.
    let mut terminal = Terminal::new(TestBackend::new(40, 20)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Failed to launch") && text.contains('…'),
        "a long status message must be truncated with an ellipsis, not dropped: {text:?}"
    );
}

#[tokio::test]
async fn review_open_in_editor_key_ignored_in_visual_mode() {
    // In visual (line-select) mode the editor shortcut is inert — it must not
    // tear the TUI down mid-selection, matching the footer, which only offers
    // "edit" outside comment/visual sub-modes.
    let (mut app, sid, _worktree) = app_with_editor_capable_session().await;

    // Enter visual mode in the body, then press the editor key.
    let mut review = review_state_for(sid);
    review.focus = super::review::ReviewFocus::Body;
    review.visual_anchor = Some(review.cursor);
    let dot = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('.'),
        crossterm::event::KeyModifiers::NONE,
    );

    app.handle_review_key(dot, review).await;

    assert!(
        app.ui_state.editor_command.is_none() && !app.ui_state.should_quit,
        "editor shortcut must be inert during a visual selection"
    );
}

#[tokio::test]
async fn review_reload_comments_routes_to_owning_backend() {
    let (mut app, remote_sid) = app_with_remote_session().await;
    let mut state = review_state_for(remote_sid);

    app.reload_review_comments(&mut state).await;

    assert_eq!(
        remote_mock(&app, BackendId(1)).listed_comment_sessions(),
        vec![remote_sid],
        "reloading comments must list from the owning backend"
    );
}

#[tokio::test]
async fn review_image_fetch_routes_to_owning_backend() {
    use crate::api::DiffSide;
    let (mut app, remote_sid) = app_with_remote_session().await;
    // A review state whose only file is a modified binary image.
    let state = DiffReviewState::new(
        remote_sid,
        "t".to_string(),
        "main".to_string(),
        crate::git::ParsedDiff {
            files: vec![modified_image_file("logo.png")],
        },
        Vec::new(),
    );

    app.ensure_review_image(&state).await;

    // The fetch runs in a spawned task that records the call, then emits a
    // `ReviewImageLoaded` event — await events until it lands, then assert the
    // remote mock (not the local backend) served the blob.
    for _ in 0..10 {
        match app.event_loop.next().await {
            Some(AppEvent::StateUpdate(StateUpdate::ReviewImageLoaded { .. })) => break,
            _ => continue,
        }
    }
    assert_eq!(
        remote_mock(&app, BackendId(1)).fetched_diff_blobs(),
        vec![(remote_sid, DiffSide::New, "logo.png".to_string())],
        "fetch_diff_blob must route to the backend that owns the session"
    );
}

#[test]
fn is_loopback_url_flags_loopback_hosts() {
    use super::is_loopback_url;
    // Loopback: the heuristic that warns about a self-referential remote server.
    assert!(is_loopback_url("http://localhost:7878"));
    assert!(is_loopback_url("http://LocalHost:7878"));
    assert!(is_loopback_url("http://127.0.0.1:7878"));
    assert!(is_loopback_url("http://127.1.2.3:7878")); // any 127.x.x.x
    assert!(is_loopback_url("http://[::1]:7878"));
    assert!(is_loopback_url("http://user@localhost:7878")); // userinfo stripped
    assert!(is_loopback_url("http://localhost")); // no port
    // Non-loopback: a real remote host.
    assert!(!is_loopback_url("https://buildbox:7878"));
    assert!(!is_loopback_url("http://192.168.1.10:7878"));
    assert!(!is_loopback_url("http://example.com"));
}

#[tokio::test]
async fn select_does_not_block_event_loop_on_mark_read() {
    // Enter-to-attach is the hottest action; its `mark_read` is a remote POST
    // with a client ceiling. It must be spawned fire-and-forget so the handler
    // returns immediately (the attach itself stamps MRU server-side).
    let (mut app, remote_sid) = app_with_remote_session().await;
    let sref = crate::backend::SessionRef::new(BackendId(1), remote_sid);
    app.ui_state.selected_session_id = Some(sref);

    // Hold mark_read open: were it awaited inline, handle_select would never
    // return and the timeout below would fire.
    let gate = remote_mock(&app, BackendId(1)).block_mark_read();

    tokio::time::timeout(std::time::Duration::from_millis(500), app.handle_select())
        .await
        .expect("handle_select must not block on the remote mark_read POST");

    // The attach is still requested synchronously.
    assert!(app.ui_state.should_quit, "attach must be requested");
    assert!(
        app.ui_state.attach_request.is_some(),
        "attach target must be set"
    );

    // Releasing the gate lets the spawned mark_read complete and record.
    gate.notify_one();
    let mut recorded = false;
    for _ in 0..50 {
        if remote_mock(&app, BackendId(1))
            .read_marked_sessions()
            .contains(&remote_sid)
        {
            recorded = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        recorded,
        "mark_read must still be delivered to the owning backend"
    );
}

/// Dispatch a `BackendChanged` fold for backend `id` carrying `new_states`,
/// after seeding that backend's cached (old) states with `old_states`.
async fn fold_backend_states(
    app: &mut App,
    id: BackendId,
    old_states: BTreeMap<SessionId, AgentState>,
    new_states: BTreeMap<SessionId, AgentState>,
) {
    app.backend_mut_for_test(id).view.agent_states.states = old_states;
    let snapshot = app.view_for(id).snapshot.clone();
    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: id.0,
        snapshot: Box::new(snapshot),
        states: Box::new(crate::api::AgentStatesSnapshot {
            states: new_states,
            commander_running: false,
        }),
    })
    .await;
}

#[tokio::test]
async fn remote_review_auto_refreshes_on_working_to_idle() {
    // With a remote session's review open, a per-backend Working→Idle transition
    // must trigger the same in-place review refresh the local path gives.
    let (mut app, remote_sid) = app_with_remote_session().await;
    app.ui_state.modal = Modal::ReviewDiff(review_state_for(remote_sid));

    fold_backend_states(
        &mut app,
        BackendId(1),
        BTreeMap::from([(remote_sid, AgentState::Working)]),
        BTreeMap::from([(remote_sid, AgentState::Idle)]),
    )
    .await;

    let mut refreshed = false;
    for _ in 0..50 {
        if remote_mock(&app, BackendId(1))
            .review_refreshed_sessions()
            .contains(&remote_sid)
        {
            refreshed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        refreshed,
        "a remote session's review must auto-refresh on Working→Idle"
    );
}

#[tokio::test]
async fn remote_review_no_refresh_without_transition() {
    // Idle→Idle (no Working→Idle edge) must NOT trigger a review refresh.
    let (mut app, remote_sid) = app_with_remote_session().await;
    app.ui_state.modal = Modal::ReviewDiff(review_state_for(remote_sid));

    fold_backend_states(
        &mut app,
        BackendId(1),
        BTreeMap::from([(remote_sid, AgentState::Idle)]),
        BTreeMap::from([(remote_sid, AgentState::Idle)]),
    )
    .await;

    // Give any (erroneously) spawned refresh a chance to land before asserting.
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        tokio::task::yield_now().await;
    }
    assert!(
        remote_mock(&app, BackendId(1))
            .review_refreshed_sessions()
            .is_empty(),
        "no transition means no review refresh"
    );
}

#[tokio::test]
async fn restart_confirm_spawns_against_owning_backend_and_toasts() {
    // Confirming a restart must spawn the restart on the OWNING backend (not the
    // local one) off the event loop, then toast on the `RestartFinished` event.
    let (app, remote_sid) = app_with_remote_session().await;
    let mut app = app;

    app.handle_confirm(super::ConfirmAction::RestartSession {
        session_id: remote_sid,
    })
    .await;

    // Drive the completion event the spawned restart posts.
    loop {
        match app.event_loop.next().await.expect("a restart event") {
            AppEvent::StateUpdate(su @ StateUpdate::RestartFinished { .. }) => {
                app.handle_state_update(su).await;
                break;
            }
            _ => continue,
        }
    }

    assert_eq!(
        remote_mock(&app, BackendId(1)).restarted_sessions(),
        vec![remote_sid],
        "restart must route to the backend that owns the session"
    );
    let (msg, _) = app
        .ui_state
        .status_message
        .clone()
        .expect("a status toast after restart");
    assert!(msg.contains("restarted"), "toast: {msg}");
}

#[tokio::test]
async fn change_program_confirm_spawns_against_owning_backend_and_routes_program() {
    // Confirming a program change must spawn `change_program` on the OWNING
    // backend (not the local one) with the chosen command, off the event loop.
    let (app, remote_sid) = app_with_remote_session().await;
    let mut app = app;

    app.handle_confirm(super::ConfirmAction::ChangeProgram {
        session_id: remote_sid,
        program: "codex".to_string(),
    })
    .await;

    // Drive the completion event the spawned change posts (reuses RestartFinished).
    loop {
        match app.event_loop.next().await.expect("a change-program event") {
            AppEvent::StateUpdate(su @ StateUpdate::RestartFinished { .. }) => {
                app.handle_state_update(su).await;
                break;
            }
            _ => continue,
        }
    }

    assert_eq!(
        remote_mock(&app, BackendId(1)).program_changes(),
        vec![(remote_sid, "codex".to_string())],
        "change_program must route to the owning backend with the chosen program"
    );
}

#[tokio::test]
async fn program_picker_items_flag_current_and_filter() {
    // `gather_program_picker_items` flags the row matching the session's current
    // program and filters rows by a label/command substring.
    let (app, sid) = app_with_remote_session().await;
    let mut app = app;
    app.ui_state.program_picker_current = "codex".to_string();
    app.ui_state.program_picker_choices = vec![
        crate::config::ProgramEntry {
            label: "Claude".to_string(),
            command: "claude".to_string(),
        },
        crate::config::ProgramEntry {
            label: "Codex".to_string(),
            command: "codex".to_string(),
        },
        crate::config::ProgramEntry {
            label: "OpenCode".to_string(),
            command: "opencode".to_string(),
        },
    ];

    let all = app.gather_program_picker_items(sid, "");
    assert_eq!(all.len(), 3, "no filter lists every choice");
    let codex = all
        .iter()
        .find_map(|i| match i {
            QuickSwitchItem::ProgramChange { program, label, .. } if program == "codex" => {
                Some(label.clone())
            }
            _ => None,
        })
        .expect("a codex row");
    assert!(
        codex.contains("current"),
        "current program is flagged: {codex}"
    );

    let filtered = app.gather_program_picker_items(sid, "open");
    assert_eq!(filtered.len(), 1, "substring filter narrows the list");
    match &filtered[0] {
        QuickSwitchItem::ProgramChange { program, .. } => assert_eq!(program, "opencode"),
        other => panic!("expected a ProgramChange row, got {other:?}"),
    }
}

#[tokio::test]
async fn program_choices_loaded_replaces_only_for_matching_open_palette() {
    // The remote program-list load must replace the palette's fallback choices
    // only when the change-program palette is still open for the SAME session.
    let (app, sid) = app_with_remote_session().await;
    let mut app = app;
    app.ui_state.program_picker_choices = vec![crate::config::ProgramEntry {
        label: "Claude".to_string(),
        command: "claude".to_string(),
    }];
    app.ui_state.modal = Modal::QuickSwitch {
        mode: super::PaletteMode::ProgramPicker { session_id: sid },
        query: super::Input::default(),
        matches: Vec::new(),
        selected_idx: 0,
        scroll: 0,
    };

    // A load for a DIFFERENT session is dropped.
    app.handle_state_update(StateUpdate::ProgramChoicesLoaded {
        session_id: SessionId::new(),
        choices: vec![crate::config::ProgramEntry {
            label: "Codex".to_string(),
            command: "codex".to_string(),
        }],
    })
    .await;
    assert_eq!(
        app.ui_state.program_picker_choices.len(),
        1,
        "a load for another session must not touch these choices"
    );

    // A load for the open palette's session replaces the choices and rebuilds rows.
    app.handle_state_update(StateUpdate::ProgramChoicesLoaded {
        session_id: sid,
        choices: vec![
            crate::config::ProgramEntry {
                label: "Codex".to_string(),
                command: "codex".to_string(),
            },
            crate::config::ProgramEntry {
                label: "OpenCode".to_string(),
                command: "opencode".to_string(),
            },
        ],
    })
    .await;
    assert_eq!(app.ui_state.program_picker_choices.len(), 2);
    match &app.ui_state.modal {
        Modal::QuickSwitch { matches, .. } => {
            assert_eq!(matches.len(), 2, "rows rebuilt from the loaded choices")
        }
        other => panic!("expected the palette to stay open, got {other:?}"),
    }
}

#[tokio::test]
async fn selecting_program_row_opens_change_program_confirm() {
    // Selecting a program row in the change-program palette must open a confirm
    // modal carrying the target session and chosen program (not apply directly).
    let (app, remote_sid) = app_with_remote_session().await;
    let mut app = app;

    app.ui_state.modal = Modal::QuickSwitch {
        mode: super::PaletteMode::ProgramPicker {
            session_id: remote_sid,
        },
        query: super::Input::default(),
        matches: vec![QuickSwitchItem::ProgramChange {
            session_id: remote_sid,
            program: "opencode".to_string(),
            label: "opencode".to_string(),
        }],
        selected_idx: 0,
        scroll: 0,
    };

    app.activate_quick_switch_selection().await;

    match &app.ui_state.modal {
        Modal::Confirm {
            on_confirm:
                super::ConfirmAction::ChangeProgram {
                    session_id,
                    program,
                },
            ..
        } => {
            assert_eq!(*session_id, remote_sid);
            assert_eq!(program, "opencode");
        }
        other => panic!("expected ChangeProgram confirm modal, got {other:?}"),
    }
}

#[tokio::test]
async fn remote_session_created_selects_row_and_reconciles_owning_backend() {
    // A SessionCreated event for a remote-owned session must refresh + reconcile
    // that backend (not the local one) BEFORE selecting, so the new row is
    // present in the tree and lands selected — the "half-lands" bug otherwise
    // no-ops the reconcile and selects nothing.
    let (remote_snap, _sid, _pid) = snapshot_with_one_session();
    let mut app = build_app_with_mock_remotes(vec![("buildbox", remote_snap)]);
    app.bootstrap_backend_views().await;
    app.refresh_backend_view(BackendId(1)).await;

    // The mock's create_session appends a new session (fresh id) to its snapshot
    // and returns that id — mirroring a real backend committing the row.
    let new_id = remote_mock(&app, BackendId(1))
        .create_session(crate::api::CreateSessionOpts {
            project_path: std::path::PathBuf::from("/tmp/rp"),
            title: "new".to_string(),
            program: None,
            initial_prompt: None,
            model: None,
            effort: None,
            mode: None,
            base_branch: None,
            section: None,
            stack_parent: None,
        })
        .await
        .unwrap();

    app.handle_state_update(StateUpdate::SessionCreated {
        session_id: new_id,
        backend_id: BackendId(1).0,
    })
    .await;

    assert_eq!(
        remote_mock(&app, BackendId(1)).reconciled_sessions(),
        vec![new_id],
        "section reconcile must route to the backend that owns the new session"
    );
    assert_eq!(
        app.ui_state.selected_session_id.map(|r| r.id),
        Some(new_id),
        "the newly created remote session should land selected in the tree"
    );
}

#[tokio::test]
async fn open_review_shows_loading_immediately_and_routes_fetch_error() {
    // handle_open_review must NOT block the event loop on the (remote) fetch: it
    // shows the loading spinner and hands the fetch to a spawned task. The mock's
    // open_review is unavailable, so the task posts ReviewOpenFailed{Some}.
    let (mut app, remote_sid) = app_with_remote_session().await;
    app.ui_state.selected_session_id = Some(SessionRef::new(BackendId(1), remote_sid));

    app.handle_open_review().await;
    assert!(
        matches!(app.ui_state.modal, Modal::Loading { .. }),
        "the spinner must be up immediately, before the fetch completes"
    );

    loop {
        match app.event_loop.next().await.expect("a review-open event") {
            AppEvent::StateUpdate(su @ StateUpdate::ReviewOpenFailed { .. }) => {
                app.handle_state_update(su).await;
                break;
            }
            _ => continue,
        }
    }
    assert!(
        matches!(app.ui_state.modal, Modal::Error { .. }),
        "a failed fetch must surface an error modal, not silently return"
    );
}

#[tokio::test]
async fn review_open_failed_none_reports_no_changes() {
    // A no-changes fetch (error: None) closes the spinner and toasts, rather
    // than opening an empty review.
    let (mut app, _remote_sid) = app_with_remote_session().await;
    app.ui_state.modal = Modal::Loading {
        title: "Preparing review".to_string(),
        message: "Loading changes…".to_string(),
        hint: None,
    };
    app.handle_state_update(StateUpdate::ReviewOpenFailed { error: None })
        .await;
    assert!(matches!(app.ui_state.modal, Modal::None));
    let (msg, _) = app.ui_state.status_message.clone().expect("a status toast");
    assert!(msg.contains("No changes"), "toast: {msg}");
}

#[tokio::test]
async fn bulk_merged_pr_delete_runs_sequentially_in_one_task() {
    // The merged-PR bulk delete must run as ONE sequential task (sessions can
    // share a git repo, and concurrent worktree removals race). Assert every
    // session is deleted via its owning backend, in order, from a single call.
    let backend: Arc<dyn CommanderBackend> = Arc::new(MockBackend::new("b", empty_snapshot()));
    let ids: Vec<SessionId> = (0..3).map(|_| SessionId::new()).collect();
    let deletes: Vec<(Arc<dyn CommanderBackend>, SessionId)> =
        ids.iter().map(|id| (backend.clone(), *id)).collect();
    let (tx, _rx) = tokio::sync::mpsc::channel(16);

    super::actions::delete_sessions_in_sequence(deletes, tx).await;

    let mock = backend.as_any().downcast_ref::<MockBackend>().unwrap();
    assert_eq!(
        mock.deleted_sessions(),
        ids,
        "all sessions must be deleted, in batch order, on the one task"
    );
}

#[test]
fn attach_transport_error_maps_to_toast() {
    // A mid-attach transport error must surface a toast, not vanish.
    let toast = attach_end_toast(&crate::tmux::AttachResult::Error("ws dropped".to_string()));
    assert_eq!(toast.as_deref(), Some("Attach failed: ws dropped"));
}

#[test]
fn attach_clean_detach_has_no_toast() {
    // A clean detach (or a session end handled by its own arm) needs no toast.
    assert_eq!(attach_end_toast(&crate::tmux::AttachResult::Detached), None);
    assert_eq!(
        attach_end_toast(&crate::tmux::AttachResult::SessionEnded),
        None
    );
}

#[test]
fn tmux_startup_proceeds_when_tmux_present_regardless_of_remotes() {
    assert_eq!(tmux_startup_decision(None, false), TmuxStartup::Proceed);
    assert_eq!(tmux_startup_decision(None, true), TmuxStartup::Proceed);
}

#[test]
fn tmux_startup_degrades_local_when_tmux_down_but_remotes_configured() {
    // A remote-only operator must not be locked out by a missing local tmux.
    assert_eq!(
        tmux_startup_decision(Some("tmux not found".to_string()), true),
        TmuxStartup::DegradeLocal("tmux not found".to_string()),
    );
}

#[test]
fn tmux_startup_aborts_when_tmux_down_and_no_remotes() {
    // With nothing else to drive, a missing tmux is still a hard error.
    assert_eq!(
        tmux_startup_decision(Some("tmux not found".to_string()), false),
        TmuxStartup::Abort("tmux not found".to_string()),
    );
}

#[tokio::test]
async fn pending_comment_markers_union_every_backend_view() {
    // A remote backend's first snapshot arrives via `BackendChanged` (the poller
    // delivers it — bootstrap skips remotes). Folding it in must re-derive the
    // session-list `*` markers so a remote session's pending comment lights up
    // in production, without any per-backend network query.
    let (mut remote_snap, remote_sid, _pid) = snapshot_with_one_session();
    remote_snap.pending_comment_sessions = vec![remote_sid];
    // The mock's view starts empty; the pending id only arrives with the
    // BackendChanged snapshot below, so this exercises the real production path.
    let mut app = build_app_with_mock_remotes(vec![("buildbox", empty_snapshot())]);
    app.bootstrap_backend_views().await;
    assert!(
        !app.ui_state.sessions_with_comments.contains(&remote_sid),
        "no marker before the remote's snapshot has landed"
    );

    app.handle_state_update(StateUpdate::BackendChanged {
        backend_id: BackendId(1).0,
        snapshot: Box::new(remote_snap),
        states: Box::new(crate::api::AgentStatesSnapshot {
            states: Default::default(),
            commander_running: false,
        }),
    })
    .await;

    assert!(
        app.ui_state.sessions_with_comments.contains(&remote_sid),
        "folding a backend snapshot must re-derive pending-comment markers"
    );
}
