use super::actions::{adjust_list_scroll, delete_confirm_message};
use super::modals::centered_rect;
use super::render::commander_chip_label;
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

// --- agent-state poll tick decisions ----------------------------------------

#[test]
fn poll_skips_when_idle_and_commander_unchanged() {
    // No sessions, commander not running, was not running → nothing to do.
    assert!(poll_tick_can_skip(true, false, false));
}

#[test]
fn poll_never_skips_while_commander_is_running() {
    // A running commander always has agent state worth forwarding, so the gate
    // must not skip even if the running flag is unchanged — the pure contract
    // matches the docstring regardless of what the call site can reach today.
    assert!(!poll_tick_can_skip(true, true, true));
    assert!(!poll_tick_can_skip(false, true, true));
}

#[test]
fn poll_does_not_skip_when_commander_flips() {
    // Commander just stopped (true → false) with no other sessions: must NOT
    // skip, so the trailing-edge "turn off" update is emitted.
    assert!(!poll_tick_can_skip(true, false, true));
    // Commander just started (false → true).
    assert!(!poll_tick_can_skip(true, true, false));
}

#[test]
fn poll_sends_on_fresh_states_or_commander_flip() {
    // Fresh states → always send.
    assert!(poll_tick_should_send(false, false, false));
    // No states but commander flipped on → send (chip turns on).
    assert!(poll_tick_should_send(true, true, false));
    // No states but commander flipped off → send (chip turns off).
    assert!(poll_tick_should_send(true, false, true));
    // No states, commander unchanged → nothing to send.
    assert!(!poll_tick_should_send(true, true, true));
    assert!(!poll_tick_should_send(true, false, false));
}

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
        lfs_pulling: false,
        stacked_child: false,
    }
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
        selected_session_id: session,
        selected_project_id: project,
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
fn test_view_mode_default_is_project_grouped() {
    let s = AppUiState::default();
    assert_eq!(s.view_mode, ViewMode::ProjectGrouped);
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
    let tmp = tempfile::TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    let state_path = tmp.path().join("state.json");
    let config = Config::default();
    let config_store = Arc::new(ConfigStore::with_path(config, config_path));
    let store = Arc::new(StateStore::with_path(AppState::new(), state_path));
    // Leak the TempDir so paths stay valid for the lifetime of the test.
    std::mem::forget(tmp);
    App::new(
        config_store,
        store,
        crate::telemetry::FrontendInfo::new("test", "0.0.0"),
    )
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
        app.ui_state.attach_command.is_none(),
        "disabled commander must not queue an attach command"
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

    app.refresh_list_items().await;

    // Move session two into "Beta" — it was in the "In Progress" catch-all.
    app.apply_section_move(s2_id, Some("Beta".to_string()))
        .await;

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
        app.ui_state.selected_session_id,
        Some(s2_id),
        "selected_session_id should still track the moved session"
    );
}

// ---------------------------------------------------------------------------
// List-modal mouse support: geometry, row mapping, click state machine
// ---------------------------------------------------------------------------

use super::modals::{
    checkout_branch_areas, modal_list_index_at, path_input_areas, quick_switch_areas,
};

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

#[test]
fn modal_list_index_at_maps_rows_within_bounds() {
    let rows = Rect::new(20, 12, 58, 5);
    assert_eq!(modal_list_index_at(20, 12, rows, 0, 10), Some(0));
    assert_eq!(modal_list_index_at(77, 16, rows, 0, 10), Some(4));
    // Left/right of the rows area, the input line above, below the window.
    assert_eq!(modal_list_index_at(19, 12, rows, 0, 10), None);
    assert_eq!(modal_list_index_at(78, 12, rows, 0, 10), None);
    assert_eq!(modal_list_index_at(20, 11, rows, 0, 10), None);
    assert_eq!(modal_list_index_at(20, 17, rows, 0, 10), None);
}

#[test]
fn modal_list_index_at_applies_scroll_offset() {
    let rows = Rect::new(20, 12, 58, 5);
    assert_eq!(modal_list_index_at(20, 13, rows, 3, 10), Some(4));
}

#[test]
fn modal_list_index_at_rejects_rows_past_end_of_list() {
    let rows = Rect::new(20, 12, 58, 5);
    assert_eq!(modal_list_index_at(20, 14, rows, 0, 2), None);
    assert_eq!(modal_list_index_at(20, 12, rows, 0, 0), None);
}

#[test]
fn wheel_step_moves_one_row_and_clamps_at_ends() {
    use super::actions::wheel_step;
    assert_eq!(wheel_step(0, false, 5), 0);
    assert_eq!(wheel_step(2, false, 5), 1);
    assert_eq!(wheel_step(2, true, 5), 3);
    assert_eq!(wheel_step(4, true, 5), 4);
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
    use crate::tui::app::{SectionsState, SettingsState, SettingsTab};
    let rows = app.build_settings_rows(SettingsTab::Keybindings);
    SettingsState {
        tab: SettingsTab::Keybindings,
        selected_row: 1,
        editing: None,
        rows,
        sections_state: SectionsState::default(),
        search: search.map(|q| q.into()),
    }
}

#[test]
fn render_keybindings_tab_draws_section_headers() {
    use crate::tui::app::Modal;
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
fn render_keybindings_search_box_filters_and_shows_prompt() {
    use crate::tui::app::Modal;
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
        focus_program: true,
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
