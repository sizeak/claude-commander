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
        crate::backend::no_remote_backends(),
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
    // `apply_section_move` refreshes the cached view itself, so the rebuilt
    // tree reflects the move without a manual sync here.
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
        app.ui_state.selected_session_id.map(|r| r.id),
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
    assert!(text.contains("Default Program"));
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

// --- InputFocus (Tab cycling in the input modal) ---

#[test]
fn input_focus_cycles_all_present_fields() {
    // Name → Project → Program → Name when both pickers are present.
    assert_eq!(InputFocus::Name.next(true, true), InputFocus::Project);
    assert_eq!(InputFocus::Project.next(true, true), InputFocus::Program);
    assert_eq!(InputFocus::Program.next(true, true), InputFocus::Name);
}

#[test]
fn input_focus_skips_absent_fields() {
    // No project picker: Name → Program → Name.
    assert_eq!(InputFocus::Name.next(false, true), InputFocus::Program);
    assert_eq!(InputFocus::Program.next(false, true), InputFocus::Name);
    // No program picker: Name → Project → Name.
    assert_eq!(InputFocus::Name.next(true, false), InputFocus::Project);
    assert_eq!(InputFocus::Project.next(true, false), InputFocus::Name);
    // Neither picker: Tab stays on the name field.
    assert_eq!(InputFocus::Name.next(false, false), InputFocus::Name);
}

#[test]
fn input_focus_prev_cycles_backward() {
    // Shift+Tab reverses: Name → Program → Project → Name with both present.
    assert_eq!(InputFocus::Name.prev(true, true), InputFocus::Program);
    assert_eq!(InputFocus::Program.prev(true, true), InputFocus::Project);
    assert_eq!(InputFocus::Project.prev(true, true), InputFocus::Name);
    // Absent fields are skipped, same as forward cycling.
    assert_eq!(InputFocus::Name.prev(false, true), InputFocus::Program);
    assert_eq!(InputFocus::Name.prev(true, false), InputFocus::Project);
    assert_eq!(InputFocus::Name.prev(false, false), InputFocus::Name);
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
    app.handle_input_submit(InputAction::AddRemoteServerName, "buildbox".into(), None)
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
    )
    .await;
    assert!(matches!(&app.ui_state.modal, Modal::Loading { .. }));
}

#[tokio::test]
async fn add_remote_server_duplicate_name_reprompts() {
    let mut app = make_test_app();
    app.config.remote_servers = vec![server_cfg("buildbox", "http://b:7878")];
    app.handle_input_submit(InputAction::AddRemoteServerName, "buildbox".into(), None)
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
