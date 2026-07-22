//! Standalone session picker for use inside a tmux popup.
//!
//! Invoked as `claude-commander pick-session --out <path>` from within
//! an attached session (Ctrl+Space while attached). Reads the persisted
//! `AppState`, lets the user fuzzy-filter sessions, and writes the
//! chosen session's `tmux_session_name` to `<path>`. The outer TUI
//! reads that file when the popup closes and re-attaches to the chosen
//! session.

use std::io::{self, Stdout};
use std::path::Path;
use std::time::Instant;

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use chrono::{DateTime, Utc};

use crate::config::AppState;
use crate::error::{Result, TuiError};
use crate::session::{SessionStatus, display_branch};
use crate::tui::list_nav::{DOUBLE_CLICK_WINDOW, list_index_at, wheel_step};

struct Match {
    tmux_name: String,
    title: String,
    branch: String,
    project_name: String,
    status: SessionStatus,
    score: i64,
    last_attached_at: Option<DateTime<Utc>>,
}

fn gather_matches(state: &AppState, query: &str, current: Option<&str>) -> Vec<Match> {
    let mut scored: Vec<Match> = Vec::new();
    for session in state.sessions.values() {
        if session.status == SessionStatus::Creating {
            continue;
        }
        // Alt+Tab semantics: never list the session the user is already
        // attached to — switching to it is a no-op.
        if let Some(c) = current
            && session.matches_tmux_name(c)
        {
            continue;
        }
        let Some(score) = session.fuzzy_score(query) else {
            continue;
        };
        let project_name = state
            .get_project(&session.project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        scored.push(Match {
            tmux_name: session.tmux_session_name.clone(),
            title: session.title.clone(),
            branch: session.branch.clone(),
            project_name,
            status: session.status,
            score,
            last_attached_at: session.last_attached_at,
        });
    }
    if query.is_empty() {
        // Most-recently attached first; sessions never attached fall to
        // the bottom, ordered by title for stable navigation.
        scored.sort_by(|a, b| {
            b.last_attached_at
                .cmp(&a.last_attached_at)
                .then_with(|| a.title.cmp(&b.title))
        });
    } else {
        scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
    }
    scored
}

/// Run the picker. Writes the chosen tmux session name to `out_path`
/// on Enter; leaves it untouched on Esc / Ctrl+C / no matches.
///
/// `current` names the session the user is attached to right now; that
/// session is excluded from the list so the top row is always the
/// previously-viewed session (Alt+Tab semantics).
pub fn run_session_picker(out_path: &Path, current: Option<&str>) -> Result<()> {
    // Propagate state-load failures (before entering the alternate screen)
    // rather than defaulting to an empty state, which would render a
    // misleading "no sessions" picker over a corrupt state file.
    let state = AppState::load()?;

    enable_raw_mode().map_err(|e| TuiError::InitFailed(e.to_string()))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .map_err(|e| TuiError::InitFailed(e.to_string()))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| TuiError::InitFailed(e.to_string()))?;

    let result = run_loop(&mut terminal, &state, current);

    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    let _ = terminal.show_cursor();

    if let Some(tmux_name) = result? {
        std::fs::write(out_path, tmux_name)
            .map_err(|e| TuiError::InitFailed(format!("write out file: {}", e)))?;
    }
    Ok(())
}

/// Mutable picker UI state threaded through [`handle_event`]. `scroll` is
/// updated by `draw` (to keep the highlight visible) and read back when
/// mapping a click position to a list row.
struct PickerUi {
    query: String,
    selected_idx: usize,
    scroll: usize,
    /// Last clicked row and when, for double-click detection. Cleared on
    /// any keystroke or non-row click, since those invalidate the pending
    /// first click (same convention as the in-tree list modals).
    last_click: Option<(usize, Instant)>,
}

/// What the event loop should do after an input event.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Continue,
    Cancel,
    /// Return the match at this index as the pick.
    Pick(usize),
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &AppState,
    current: Option<&str>,
) -> Result<Option<String>> {
    let mut ui = PickerUi {
        query: String::new(),
        selected_idx: 0,
        scroll: 0,
        last_click: None,
    };

    loop {
        let matches = gather_matches(state, &ui.query, current);
        if ui.selected_idx >= matches.len() {
            ui.selected_idx = matches.len().saturating_sub(1);
        }

        terminal
            .draw(|f| draw(f, &ui.query, &matches, ui.selected_idx, &mut ui.scroll))
            .map_err(|e| TuiError::InitFailed(e.to_string()))?;

        let area = terminal
            .size()
            .map(|s| Rect::new(0, 0, s.width, s.height))
            .map_err(|e| TuiError::InitFailed(e.to_string()))?;
        let ev = event::read().map_err(|e| TuiError::InitFailed(e.to_string()))?;
        match handle_event(&mut ui, &ev, matches.len(), area, Instant::now()) {
            Verdict::Continue => {}
            Verdict::Cancel => return Ok(None),
            Verdict::Pick(idx) => {
                if let Some(m) = matches.get(idx) {
                    return Ok(Some(m.tmux_name.clone()));
                }
            }
        }
    }
}

/// Advance the picker state by one input event. Pure with respect to the
/// terminal so key and mouse behaviour can be unit-tested: `area` is the
/// full popup area (as last drawn), `now` the event's arrival time.
fn handle_event(
    ui: &mut PickerUi,
    ev: &Event,
    n_matches: usize,
    area: Rect,
    now: Instant,
) -> Verdict {
    match ev {
        Event::Key(key) => {
            if key.kind != KeyEventKind::Press {
                return Verdict::Continue;
            }
            ui.last_click = None;
            match (key.code, key.modifiers) {
                (KeyCode::Esc, _) => Verdict::Cancel,
                (KeyCode::Char('c' | 'g'), KeyModifiers::CONTROL) => Verdict::Cancel,
                (KeyCode::Enter, _) if n_matches > 0 => Verdict::Pick(ui.selected_idx),
                (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                    ui.selected_idx = ui.selected_idx.saturating_sub(1);
                    Verdict::Continue
                }
                (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL)
                    if ui.selected_idx + 1 < n_matches =>
                {
                    ui.selected_idx += 1;
                    Verdict::Continue
                }
                (KeyCode::Backspace, _) => {
                    ui.query.pop();
                    ui.selected_idx = 0;
                    ui.scroll = 0;
                    Verdict::Continue
                }
                (KeyCode::Char(c), m)
                    if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
                {
                    ui.query.push(c);
                    ui.selected_idx = 0;
                    ui.scroll = 0;
                    Verdict::Continue
                }
                _ => Verdict::Continue,
            }
        }
        Event::Mouse(mouse) => match mouse.kind {
            // Wheel moves the highlight, clamping at the ends; `draw`
            // adjusts `scroll` to keep it visible (same semantics as the
            // in-tree quick-switch palette).
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if n_matches > 0 => {
                let down = mouse.kind == MouseEventKind::ScrollDown;
                ui.selected_idx = wheel_step(ui.selected_idx, down, n_matches);
                Verdict::Continue
            }
            // A click highlights the row under the cursor; a second click
            // on the same row within DOUBLE_CLICK_WINDOW picks it, exactly
            // as Enter would.
            MouseEventKind::Down(MouseButton::Left) => {
                let idx = list_area(area).and_then(|rows| {
                    list_index_at(mouse.column, mouse.row, rows, ui.scroll, n_matches)
                });
                let Some(idx) = idx else {
                    // The query line or an empty row: any pending
                    // first-click is stale.
                    ui.last_click = None;
                    return Verdict::Continue;
                };
                ui.selected_idx = idx;
                let is_double = matches!(
                    ui.last_click,
                    Some((prev_idx, prev_at))
                        if prev_idx == idx && now.duration_since(prev_at) <= DOUBLE_CLICK_WINDOW
                );
                if is_double {
                    // Consume the click pair so a third click doesn't re-fire.
                    ui.last_click = None;
                    Verdict::Pick(idx)
                } else {
                    ui.last_click = Some((idx, now));
                    Verdict::Continue
                }
            }
            _ => Verdict::Continue,
        },
        _ => Verdict::Continue,
    }
}

/// The rows-only list area beneath the one-line query input, or `None`
/// when the popup is too short to show any rows. Shared by `draw` and the
/// click mapping in [`handle_event`] so they can't disagree on geometry.
fn list_area(area: Rect) -> Option<Rect> {
    (area.height > 1).then(|| Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height - 1,
    })
}

fn draw(
    f: &mut Frame<'_>,
    query: &str,
    matches: &[Match],
    selected_idx: usize,
    scroll: &mut usize,
) {
    let area = f.area();
    if area.height == 0 {
        return;
    }

    let input_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Line::from(format!("❯ {}_", query))),
        input_area,
    );

    let Some(list_area) = list_area(area) else {
        return;
    };
    let visible = list_area.height as usize;
    if visible == 0 {
        return;
    }

    if selected_idx < *scroll {
        *scroll = selected_idx;
    }
    if selected_idx >= *scroll + visible {
        *scroll = selected_idx + 1 - visible;
    }

    for (i, m) in matches.iter().skip(*scroll).take(visible).enumerate() {
        let abs_idx = *scroll + i;
        let is_selected = abs_idx == selected_idx;
        let row = Rect {
            x: list_area.x,
            y: list_area.y + i as u16,
            width: list_area.width,
            height: 1,
        };

        let (icon, color) = match m.status {
            SessionStatus::Creating | SessionStatus::Merging | SessionStatus::Pushing => {
                ("⠋", Color::Yellow)
            }
            SessionStatus::Running => ("●", Color::Green),
            SessionStatus::Stopped => ("○", Color::DarkGray),
            SessionStatus::CascadePaused => ("⏸", Color::Yellow),
        };

        let title_style = if is_selected {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        } else {
            Style::default()
        };

        let mut spans = vec![
            Span::styled(format!(" {} ", icon), Style::default().fg(color)),
            Span::styled(m.title.clone(), title_style),
        ];
        if let Some(shown_branch) = display_branch(&m.title, &m.branch) {
            spans.push(Span::styled(
                format!(" [{}]", shown_branch),
                Style::default().fg(Color::Cyan),
            ));
        }
        spans.push(Span::styled(
            format!(" ({})", m.project_name),
            Style::default().fg(Color::DarkGray),
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Project, ProjectId, SessionId, WorktreeSession};
    use chrono::Duration;
    use std::path::PathBuf;

    fn add_session(state: &mut AppState, project_id: ProjectId, title: &str) -> SessionId {
        let s = WorktreeSession::new(project_id, title, title, PathBuf::from("/tmp/wt"), "claude");
        let id = s.id;
        state.add_session(s);
        id
    }

    fn fixture() -> (AppState, ProjectId) {
        let mut state = AppState::new();
        let project = Project::new("p", PathBuf::from("/tmp/p"), "main");
        let project_id = project.id;
        state.add_project(project);
        (state, project_id)
    }

    #[test]
    fn empty_query_orders_by_last_attached_desc() {
        let (mut state, project_id) = fixture();
        let a = add_session(&mut state, project_id, "alpha");
        let b = add_session(&mut state, project_id, "bravo");
        let c = add_session(&mut state, project_id, "charlie");

        let now = chrono::Utc::now();
        state.sessions.get_mut(&a).unwrap().last_attached_at = Some(now - Duration::minutes(5));
        state.sessions.get_mut(&b).unwrap().last_attached_at = Some(now - Duration::minutes(1));
        state.sessions.get_mut(&c).unwrap().last_attached_at = Some(now - Duration::minutes(10));

        let matches = gather_matches(&state, "", None);
        let titles: Vec<&str> = matches.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, vec!["bravo", "alpha", "charlie"]);
    }

    #[test]
    fn never_attached_sessions_sort_to_bottom_by_title() {
        let (mut state, project_id) = fixture();
        let a = add_session(&mut state, project_id, "zulu");
        let _b = add_session(&mut state, project_id, "alpha");
        let _c = add_session(&mut state, project_id, "bravo");

        // Only zulu has been attached; alpha + bravo are unattached and
        // should fall below zulu, ordered by title.
        state.sessions.get_mut(&a).unwrap().last_attached_at = Some(chrono::Utc::now());

        let matches = gather_matches(&state, "", None);
        let titles: Vec<&str> = matches.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, vec!["zulu", "alpha", "bravo"]);
    }

    #[test]
    fn current_session_is_excluded() {
        let (mut state, project_id) = fixture();
        let _a = add_session(&mut state, project_id, "alpha");
        let b = add_session(&mut state, project_id, "bravo");
        let current_tmux = state.sessions.get(&b).unwrap().tmux_session_name.clone();

        let matches = gather_matches(&state, "", Some(&current_tmux));
        let titles: Vec<&str> = matches.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, vec!["alpha"]);
    }

    #[test]
    fn current_session_excluded_via_shell_pair_name() {
        let (mut state, project_id) = fixture();
        let _a = add_session(&mut state, project_id, "alpha");
        let b = add_session(&mut state, project_id, "bravo");
        // bravo has a paired shell session — Ctrl+\ in attach mode toggles
        // the user between them. Either name should hide bravo from the list.
        state.sessions.get_mut(&b).unwrap().shell_tmux_session_name = Some("cc-bravo-sh".into());

        let matches = gather_matches(&state, "", Some("cc-bravo-sh"));
        let titles: Vec<&str> = matches.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, vec!["alpha"]);
    }

    // -- Mouse interaction (parity with the in-tree quick-switch palette) --

    use crossterm::event::{KeyEvent, MouseEvent};
    use std::time::Duration as StdDuration;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 6,
    };

    fn ui() -> PickerUi {
        PickerUi {
            query: String::new(),
            selected_idx: 0,
            scroll: 0,
            last_click: None,
        }
    }

    fn click(col: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn wheel(down: bool) -> Event {
        Event::Mouse(MouseEvent {
            kind: if down {
                MouseEventKind::ScrollDown
            } else {
                MouseEventKind::ScrollUp
            },
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn wheel_moves_highlight_and_clamps_at_ends() {
        let mut ui = ui();
        let now = Instant::now();
        assert_eq!(
            handle_event(&mut ui, &wheel(false), 3, AREA, now),
            Verdict::Continue
        );
        assert_eq!(ui.selected_idx, 0, "wheel up at the top clamps");
        handle_event(&mut ui, &wheel(true), 3, AREA, now);
        assert_eq!(ui.selected_idx, 1);
        handle_event(&mut ui, &wheel(true), 3, AREA, now);
        handle_event(&mut ui, &wheel(true), 3, AREA, now);
        assert_eq!(ui.selected_idx, 2, "wheel down at the bottom clamps");
    }

    #[test]
    fn single_click_highlights_row_without_picking() {
        let mut ui = ui();
        // Row 0 of the list sits at y=1 (the query line is y=0).
        let verdict = handle_event(&mut ui, &click(5, 3), 5, AREA, Instant::now());
        assert_eq!(verdict, Verdict::Continue);
        assert_eq!(ui.selected_idx, 2);
    }

    #[test]
    fn click_maps_through_scroll_offset() {
        let mut ui = ui();
        ui.scroll = 4;
        handle_event(&mut ui, &click(5, 2), 10, AREA, Instant::now());
        assert_eq!(ui.selected_idx, 5);
    }

    #[test]
    fn double_click_on_same_row_picks_it() {
        let mut ui = ui();
        let t0 = Instant::now();
        handle_event(&mut ui, &click(5, 3), 5, AREA, t0);
        let verdict = handle_event(
            &mut ui,
            &click(5, 3),
            5,
            AREA,
            t0 + StdDuration::from_millis(100),
        );
        assert_eq!(verdict, Verdict::Pick(2));
    }

    #[test]
    fn slow_second_click_rehighlights_but_does_not_pick() {
        let mut ui = ui();
        let t0 = Instant::now();
        handle_event(&mut ui, &click(5, 3), 5, AREA, t0);
        let verdict = handle_event(
            &mut ui,
            &click(5, 3),
            5,
            AREA,
            t0 + DOUBLE_CLICK_WINDOW + StdDuration::from_millis(1),
        );
        assert_eq!(verdict, Verdict::Continue);
        assert_eq!(ui.selected_idx, 2);
    }

    #[test]
    fn clicks_on_different_rows_do_not_pick() {
        let mut ui = ui();
        let t0 = Instant::now();
        handle_event(&mut ui, &click(5, 3), 5, AREA, t0);
        let verdict = handle_event(
            &mut ui,
            &click(5, 4),
            5,
            AREA,
            t0 + StdDuration::from_millis(100),
        );
        assert_eq!(verdict, Verdict::Continue);
        assert_eq!(ui.selected_idx, 3);
    }

    #[test]
    fn click_off_the_list_clears_pending_first_click() {
        let mut ui = ui();
        let t0 = Instant::now();
        handle_event(&mut ui, &click(5, 3), 2, AREA, t0);
        // Row y=4 maps past the end of the 2-item list: not a row.
        handle_event(
            &mut ui,
            &click(5, 4),
            2,
            AREA,
            t0 + StdDuration::from_millis(50),
        );
        let verdict = handle_event(
            &mut ui,
            &click(5, 3),
            2,
            AREA,
            t0 + StdDuration::from_millis(100),
        );
        assert_eq!(verdict, Verdict::Continue, "pending click was cleared");
    }

    #[test]
    fn keystroke_clears_pending_first_click() {
        let mut ui = ui();
        let t0 = Instant::now();
        handle_event(&mut ui, &click(5, 3), 5, AREA, t0);
        handle_event(
            &mut ui,
            &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            5,
            AREA,
            t0 + StdDuration::from_millis(50),
        );
        let verdict = handle_event(
            &mut ui,
            &click(5, 3),
            5,
            AREA,
            t0 + StdDuration::from_millis(100),
        );
        assert_eq!(verdict, Verdict::Continue, "pending click was cleared");
    }

    #[test]
    fn enter_picks_highlighted_row_and_esc_cancels() {
        let mut ui = ui();
        ui.selected_idx = 3;
        let now = Instant::now();
        assert_eq!(
            handle_event(
                &mut ui,
                &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                5,
                AREA,
                now
            ),
            Verdict::Pick(3)
        );
        assert_eq!(
            handle_event(
                &mut ui,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
                5,
                AREA,
                now
            ),
            Verdict::Cancel
        );
    }

    #[test]
    fn enter_with_no_matches_does_not_pick() {
        let mut ui = ui();
        let verdict = handle_event(
            &mut ui,
            &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            0,
            AREA,
            Instant::now(),
        );
        assert_eq!(verdict, Verdict::Continue);
    }

    #[test]
    fn typing_updates_query_and_resets_selection() {
        let mut ui = ui();
        ui.selected_idx = 2;
        ui.scroll = 1;
        handle_event(
            &mut ui,
            &Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            5,
            AREA,
            Instant::now(),
        );
        assert_eq!(ui.query, "a");
        assert_eq!(ui.selected_idx, 0);
        assert_eq!(ui.scroll, 0);
    }

    #[test]
    fn fuzzy_query_falls_back_to_score_order_and_still_excludes_current() {
        let (mut state, project_id) = fixture();
        let _a = add_session(&mut state, project_id, "alpha");
        let b = add_session(&mut state, project_id, "bravo");
        let _c = add_session(&mut state, project_id, "alphabet");
        let current_tmux = state.sessions.get(&b).unwrap().tmux_session_name.clone();

        let matches = gather_matches(&state, "alp", Some(&current_tmux));
        let titles: Vec<&str> = matches.iter().map(|m| m.title.as_str()).collect();
        // Both alpha-prefix titles match; exact ranking comes from fuzzy
        // score, but the property we care about is: bravo is gone and
        // both alpha-family titles are present.
        assert!(titles.contains(&"alpha"));
        assert!(titles.contains(&"alphabet"));
        assert!(!titles.contains(&"bravo"));
    }
}
