//! Snapshot tests for TUI widget rendering
//!
//! Uses ratatui's TestBackend + insta for visual regression testing.
//! Run `cargo insta review` to accept/update snapshots.

use std::path::PathBuf;
use std::time::Instant;

use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::git::DiffInfo;
use crate::session::{ProjectId, SessionId, SessionListItem, SessionStatus};
use crate::tui::theme::Theme;
use crate::tui::widgets::{DiffView, Preview, TreeList};

/// Fixed theme for reproducible snapshots (no terminal detection)
fn test_theme() -> Theme {
    Theme::basic()
}

/// Helper to center a rect (mirrors app.rs centered_rect)
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// ── Session list ───────────────────────────────────────────────────

#[test]
fn test_session_list_empty() {
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let items: Vec<SessionListItem> = vec![];
            let tree_list = TreeList::new(&items, &theme)
                .highlight_style(theme.selection().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(
                tree_list,
                frame.area(),
                &mut ratatui::widgets::ListState::default(),
            );
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_session_list_single_project() {
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let items = vec![SessionListItem::Project {
                id: ProjectId::new(),
                name: "my-project".to_string(),
                repo_path: PathBuf::from("/home/user/projects/my-project"),
                main_branch: "main".to_string(),
                worktree_count: 0,
            }];
            let tree_list = TreeList::new(&items, &theme)
                .highlight_style(theme.selection().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(
                tree_list,
                frame.area(),
                &mut ratatui::widgets::ListState::default(),
            );
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_session_list_with_sessions() {
    let backend = TestBackend::new(70, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();
    let project_id = ProjectId::new();

    terminal
        .draw(|frame| {
            let items = vec![
                SessionListItem::Project {
                    id: project_id,
                    name: "claude-commander".to_string(),
                    repo_path: PathBuf::from("/home/user/projects/cc"),
                    main_branch: "main".to_string(),
                    worktree_count: 3,
                },
                SessionListItem::Worktree {
                    id: SessionId::new(),
                    project_id,
                    title: "Add auth feature".to_string(),
                    branch: "feature-auth".to_string(),
                    status: SessionStatus::Running,
                    program: "claude".to_string(),
                    pr_number: None,
                    pr_url: None,
                    pr_merged: false,
                },
                SessionListItem::Worktree {
                    id: SessionId::new(),
                    project_id,
                    title: "Fix login bug".to_string(),
                    branch: "fix-login".to_string(),
                    status: SessionStatus::Paused,
                    program: "claude".to_string(),
                    pr_number: None,
                    pr_url: None,
                    pr_merged: false,
                },
                SessionListItem::Worktree {
                    id: SessionId::new(),
                    project_id,
                    title: "Refactor DB".to_string(),
                    branch: "refactor-db".to_string(),
                    status: SessionStatus::Stopped,
                    program: "claude".to_string(),
                    pr_number: None,
                    pr_url: None,
                    pr_merged: false,
                },
            ];
            let tree_list = TreeList::new(&items, &theme)
                .highlight_style(theme.selection().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(
                tree_list,
                frame.area(),
                &mut ratatui::widgets::ListState::default(),
            );
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_session_list_with_pr_badges() {
    let backend = TestBackend::new(120, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();
    let project_id = ProjectId::new();

    terminal
        .draw(|frame| {
            let items = vec![
                SessionListItem::Project {
                    id: project_id,
                    name: "my-app".to_string(),
                    repo_path: PathBuf::from("/home/user/my-app"),
                    main_branch: "main".to_string(),
                    worktree_count: 2,
                },
                SessionListItem::Worktree {
                    id: SessionId::new(),
                    project_id,
                    title: "Add feature".to_string(),
                    branch: "feat-x".to_string(),
                    status: SessionStatus::Running,
                    program: "claude".to_string(),
                    pr_number: Some(42),
                    pr_url: Some("https://github.com/org/repo/pull/42".to_string()),
                    pr_merged: false,
                },
                SessionListItem::Worktree {
                    id: SessionId::new(),
                    project_id,
                    title: "Old PR".to_string(),
                    branch: "old-pr".to_string(),
                    status: SessionStatus::Stopped,
                    program: "claude".to_string(),
                    pr_number: Some(10),
                    pr_url: Some("https://github.com/org/repo/pull/10".to_string()),
                    pr_merged: true,
                },
            ];
            let tree_list = TreeList::new(&items, &theme)
                .highlight_style(theme.selection().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(
                tree_list,
                frame.area(),
                &mut ratatui::widgets::ListState::default(),
            );
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_session_list_mixed_programs() {
    let backend = TestBackend::new(120, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();
    let project_id = ProjectId::new();

    terminal
        .draw(|frame| {
            let items = vec![
                SessionListItem::Project {
                    id: project_id,
                    name: "multi-agent".to_string(),
                    repo_path: PathBuf::from("/home/user/multi"),
                    main_branch: "main".to_string(),
                    worktree_count: 2,
                },
                SessionListItem::Worktree {
                    id: SessionId::new(),
                    project_id,
                    title: "Claude task".to_string(),
                    branch: "claude-task".to_string(),
                    status: SessionStatus::Running,
                    program: "claude".to_string(),
                    pr_number: None,
                    pr_url: None,
                    pr_merged: false,
                },
                SessionListItem::Worktree {
                    id: SessionId::new(),
                    project_id,
                    title: "Aider task".to_string(),
                    branch: "aider-task".to_string(),
                    status: SessionStatus::Running,
                    program: "aider".to_string(),
                    pr_number: None,
                    pr_url: None,
                    pr_merged: false,
                },
            ];
            let tree_list = TreeList::new(&items, &theme)
                .highlight_style(theme.selection().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(
                tree_list,
                frame.area(),
                &mut ratatui::widgets::ListState::default(),
            );
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

// ── Preview widget ─────────────────────────────────────────────────

#[test]
fn test_preview_empty() {
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| {
            let preview = Preview::new("")
                .block(
                    Block::default()
                        .title(" [Preview] | Diff | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(preview, frame.area());
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_preview_with_content() {
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    let content = "$ claude --resume\n\nClaude is thinking...\n\n> I'll help you fix the auth bug.\n> Let me look at the code first.\n\nReading src/auth.rs...";

    terminal
        .draw(|frame| {
            let preview = Preview::new(content)
                .block(
                    Block::default()
                        .title(" [Preview] | Diff | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(preview, frame.area());
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_preview_scrolled() {
    let backend = TestBackend::new(60, 8);
    let mut terminal = Terminal::new(backend).unwrap();

    let content = (0..50)
        .map(|i| format!("Line {}: some content here", i))
        .collect::<Vec<_>>()
        .join("\n");

    terminal
        .draw(|frame| {
            let preview = Preview::new(&content)
                .block(
                    Block::default()
                        .title(" [Preview] | Diff | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(20);
            frame.render_widget(preview, frame.area());
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

// ── Diff view ──────────────────────────────────────────────────────

#[test]
fn test_diff_view_empty() {
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let info = DiffInfo::empty();
            let diff_view = DiffView::new(&info, &theme)
                .block(
                    Block::default()
                        .title(" Preview | [Diff] | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(diff_view, frame.area());
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_diff_view_with_changes() {
    let backend = TestBackend::new(70, 16);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    let diff = "\
diff --git a/src/auth.rs b/src/auth.rs
index abc123..def456 100644
--- a/src/auth.rs
+++ b/src/auth.rs
@@ -10,7 +10,9 @@ fn authenticate(user: &str) -> Result<Token> {
     let credentials = load_credentials(user)?;
-    let token = validate(credentials);
+    let token = validate(credentials)?;
+    info!(\"User {} authenticated\", user);
+    update_last_login(user)?;
     Ok(token)
 }";

    terminal
        .draw(|frame| {
            let info = DiffInfo {
                diff: diff.to_string(),
                files_changed: 1,
                lines_added: 3,
                lines_removed: 1,
                line_count: diff.lines().count(),
                computed_at: Instant::now(),
                base_commit: "abc123".to_string(),
            };
            let diff_view = DiffView::new(&info, &theme)
                .block(
                    Block::default()
                        .title(" Preview | [Diff] | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(diff_view, frame.area());
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

// ── Modals ─────────────────────────────────────────────────────────

#[test]
fn test_modal_input() {
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let area = frame.area();
            let modal_area = centered_rect(60, 20, area);
            frame.render_widget(Clear, modal_area);

            let block = Block::default()
                .title(" New Session ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.modal_warning));

            let inner = block.inner(modal_area);
            frame.render_widget(block, modal_area);

            let text = "Enter session name:\n\n> my-feature_";
            let paragraph = Paragraph::new(text);
            frame.render_widget(paragraph, inner);
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_modal_confirm() {
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let area = frame.area();
            let modal_area = centered_rect(50, 15, area);
            frame.render_widget(Clear, modal_area);

            let block = Block::default()
                .title(" Delete Session ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.modal_error));

            let inner = block.inner(modal_area);
            frame.render_widget(block, modal_area);

            let text = "Delete session 'fix-login'?\n\n[Enter] Confirm  [Esc] Cancel";
            let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
            frame.render_widget(paragraph, inner);
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_modal_error() {
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let area = frame.area();
            let modal_area = centered_rect(60, 20, area);
            frame.render_widget(Clear, modal_area);

            let block = Block::default()
                .title(" Error ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.modal_error));

            let inner = block.inner(modal_area);
            frame.render_widget(block, modal_area);

            let text =
                "Failed to create session: git worktree add failed\n\nPress any key to close.";
            let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
            frame.render_widget(paragraph, inner);
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_modal_help() {
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let area = frame.area();
            let modal_area = centered_rect(70, 80, area);
            frame.render_widget(Clear, modal_area);

            let block = Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.modal_info));

            let inner = block.inner(modal_area);
            frame.render_widget(block, modal_area);

            let content_area = inner.inner(Margin {
                horizontal: 2,
                vertical: 1,
            });

            let help_lines = vec![
                Line::from("Navigation:"),
                Line::from("  j/k, Up/Down    Navigate session list"),
                Line::from("  Enter           Attach to selected session"),
                Line::from("  Tab/Shift+Tab   Toggle preview/diff/shell view"),
                Line::from(""),
                Line::from("Session Management:"),
                Line::from("  n               New worktree session"),
                Line::from("  N               New project (add git repo)"),
                Line::from("  p               Pause session"),
                Line::from("  r               Resume session"),
                Line::from("  d               Delete/kill session"),
                Line::from(""),
                Line::from("Press any key to close this help."),
            ];

            let paragraph = Paragraph::new(help_lines);
            frame.render_widget(paragraph, content_area);
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_modal_loading() {
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let area = frame.area();
            let modal_area = centered_rect(60, 20, area);
            frame.render_widget(Clear, modal_area);

            let block = Block::default()
                .title(" New Session ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.modal_info));

            let inner = block.inner(modal_area);
            frame.render_widget(block, modal_area);

            let text = "⠋ Creating \"my-feature\"...";
            let paragraph = Paragraph::new(text);
            frame.render_widget(paragraph, inner);
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

// ── Status bar ─────────────────────────────────────────────────────

#[test]
fn test_status_bar_default() {
    let backend = TestBackend::new(120, 1);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let text = "Sessions: 3 | Press ? for help | n: new session | N: add project";
            let paragraph = Paragraph::new(text).style(theme.status_bar());
            frame.render_widget(paragraph, frame.area());
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_status_bar_with_message() {
    let backend = TestBackend::new(120, 1);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let text = "Created session abc12345";
            let paragraph = Paragraph::new(text).style(theme.status_bar());
            frame.render_widget(paragraph, frame.area());
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

// ── Pane content replacement (no clear needed) ─────────────────────

#[test]
fn test_preview_content_replacement_no_clear() {
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    // Render content A (simulating session 1 selected)
    terminal
        .draw(|frame| {
            let preview = Preview::new("Session 1 output\nLine 2\nLine 3\nLine 4\nLine 5")
                .block(
                    Block::default()
                        .title(" [Preview] | Diff | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(preview, frame.area());
        })
        .unwrap();

    // Render content B WITHOUT clearing (simulating scrolling to session 2)
    terminal
        .draw(|frame| {
            let preview = Preview::new("Session 2 output\nDifferent content")
                .block(
                    Block::default()
                        .title(" [Preview] | Diff | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(preview, frame.area());
        })
        .unwrap();

    // Snapshot should show clean content B with no artifacts from A
    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_preview_to_diff_view_switch_no_clear() {
    let backend = TestBackend::new(70, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    // Render Preview pane first
    terminal
        .draw(|frame| {
            let preview = Preview::new("Preview content here\nLine 2\nLine 3")
                .block(
                    Block::default()
                        .title(" [Preview] | Diff | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(preview, frame.area());
        })
        .unwrap();

    // Switch to Diff view WITHOUT clearing
    terminal
        .draw(|frame| {
            let diff = "\
diff --git a/file.rs b/file.rs
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,3 @@
 context
-old line
+new line";
            let info = DiffInfo {
                diff: diff.to_string(),
                files_changed: 1,
                lines_added: 1,
                lines_removed: 1,
                line_count: diff.lines().count(),
                computed_at: Instant::now(),
                base_commit: "abc".to_string(),
            };
            let diff_view = DiffView::new(&info, &theme)
                .block(
                    Block::default()
                        .title(" Preview | [Diff] | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(diff_view, frame.area());
        })
        .unwrap();

    // Snapshot should show clean Diff view with no Preview artifacts
    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_preview_to_shell_view_switch_no_clear() {
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();

    // Render Preview pane first
    terminal
        .draw(|frame| {
            let preview = Preview::new("Preview content\nWith multiple lines\nOf output")
                .block(
                    Block::default()
                        .title(" [Preview] | Diff | Shell ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(preview, frame.area());
        })
        .unwrap();

    // Switch to Shell view WITHOUT clearing
    terminal
        .draw(|frame| {
            let shell = Preview::new("$ ls -la\ntotal 42\ndrwxr-xr-x 5 user user 4096")
                .block(
                    Block::default()
                        .title(" Preview | Diff | [Shell] ")
                        .borders(Borders::ALL),
                )
                .scroll(0);
            frame.render_widget(shell, frame.area());
        })
        .unwrap();

    // Snapshot should show clean Shell view with no Preview artifacts
    insta::assert_snapshot!(terminal.backend());
}

// ── Quick-switch modal ─────────────────────────────────────────────

#[test]
fn test_quick_switch_empty_query() {
    let backend = TestBackend::new(80, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let area = frame.area();
            let modal_width = area.width * 60 / 100;
            let modal_area = Rect {
                x: area.x + (area.width - modal_width) / 2,
                y: area.y + area.height / 5,
                width: modal_width,
                height: 3, // border + input + border
            };
            frame.render_widget(Clear, modal_area);

            let block = Block::default()
                .title(" Quick Switch ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.modal_info));
            let inner = block.inner(modal_area);
            frame.render_widget(block, modal_area);

            let input_line = Line::from("> _");
            frame.render_widget(Paragraph::new(input_line), inner);
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_quick_switch_with_matches() {
    let backend = TestBackend::new(80, 15);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();

    terminal
        .draw(|frame| {
            let area = frame.area();
            let modal_width = area.width * 60 / 100;
            let matches = vec![
                (
                    "●",
                    theme.status_running,
                    "Add auth",
                    "feature-auth",
                    "my-app",
                    true,
                ),
                (
                    "◐",
                    theme.status_paused,
                    "Fix login",
                    "fix-login",
                    "my-app",
                    false,
                ),
                (
                    "○",
                    theme.status_stopped,
                    "Old task",
                    "old-branch",
                    "other",
                    false,
                ),
            ];
            let modal_area = Rect {
                x: area.x + (area.width - modal_width) / 2,
                y: area.y + area.height / 5,
                width: modal_width,
                height: 3 + matches.len() as u16,
            };
            frame.render_widget(Clear, modal_area);

            let block = Block::default()
                .title(" Quick Switch ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.modal_info));
            let inner = block.inner(modal_area);
            frame.render_widget(block, modal_area);

            // Input line
            let input_area = Rect { height: 1, ..inner };
            frame.render_widget(Paragraph::new(Line::from("> auth_")), input_area);

            // Match lines
            for (i, (icon, color, title, branch, project, selected)) in matches.iter().enumerate() {
                let row = inner.y + 1 + i as u16;
                let line = Line::from(vec![
                    Span::styled(format!(" {} ", icon), Style::default().fg(*color)),
                    Span::styled(
                        title.to_string(),
                        if *selected {
                            theme.selection()
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(
                        format!(" [{}]", branch),
                        Style::default().fg(theme.text_accent),
                    ),
                    Span::styled(
                        format!(" ({})", project),
                        Style::default().fg(theme.text_secondary),
                    ),
                ]);
                let line_area = Rect {
                    y: row,
                    height: 1,
                    ..inner
                };
                frame.render_widget(Paragraph::new(line), line_area);
            }
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn test_session_list_creating_status() {
    let backend = TestBackend::new(70, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = test_theme();
    let project_id = ProjectId::new();

    terminal
        .draw(|frame| {
            let items = vec![
                SessionListItem::Project {
                    id: project_id,
                    name: "my-project".to_string(),
                    repo_path: PathBuf::from("/home/user/my-project"),
                    main_branch: "main".to_string(),
                    worktree_count: 2,
                },
                SessionListItem::Worktree {
                    id: SessionId::new(),
                    project_id,
                    title: "Existing session".to_string(),
                    branch: "feature-existing".to_string(),
                    status: SessionStatus::Running,
                    program: "claude".to_string(),
                    pr_number: None,
                    pr_url: None,
                    pr_merged: false,
                },
                SessionListItem::Worktree {
                    id: SessionId::new(),
                    project_id,
                    title: "New session".to_string(),
                    branch: "feature-new".to_string(),
                    status: SessionStatus::Creating,
                    program: "claude".to_string(),
                    pr_number: None,
                    pr_url: None,
                    pr_merged: false,
                },
            ];
            // tick=0 → spinner frame 0 → "⠋"
            let tree_list = TreeList::new(&items, &theme)
                .tick(0)
                .highlight_style(theme.selection().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(
                tree_list,
                frame.area(),
                &mut ratatui::widgets::ListState::default(),
            );
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}
