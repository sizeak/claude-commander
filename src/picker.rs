//! Standalone session picker for use inside a tmux popup.
//!
//! Invoked as `claude-commander pick-session --out <path>` from within
//! an attached session (Ctrl+O while attached). Reads the persisted
//! `AppState`, lets the user fuzzy-filter sessions, and writes the
//! chosen session's `tmux_session_name` to `<path>`. The outer TUI
//! reads that file when the popup closes and re-attaches to the chosen
//! session.

use std::io::{self, Stdout};
use std::path::Path;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
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

use crate::config::AppState;
use crate::error::{Result, TuiError};
use crate::session::{SessionStatus, display_branch};

struct Match {
    tmux_name: String,
    title: String,
    branch: String,
    project_name: String,
    status: SessionStatus,
    score: i64,
}

fn gather_matches(state: &AppState, query: &str) -> Vec<Match> {
    let mut scored: Vec<Match> = Vec::new();
    for session in state.sessions.values() {
        if session.status == SessionStatus::Creating {
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
        });
    }
    if query.is_empty() {
        scored.sort_by(|a, b| a.title.cmp(&b.title));
    } else {
        scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
    }
    scored
}

/// Run the picker. Writes the chosen tmux session name to `out_path`
/// on Enter; leaves it untouched on Esc / Ctrl+C / no matches.
pub fn run_session_picker(out_path: &Path) -> Result<()> {
    let state = AppState::load().unwrap_or_else(|_| AppState::new());

    enable_raw_mode().map_err(|e| TuiError::InitFailed(e.to_string()))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| TuiError::InitFailed(e.to_string()))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| TuiError::InitFailed(e.to_string()))?;

    let result = run_loop(&mut terminal, &state);

    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    if let Some(tmux_name) = result? {
        std::fs::write(out_path, tmux_name)
            .map_err(|e| TuiError::InitFailed(format!("write out file: {}", e)))?;
    }
    Ok(())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &AppState,
) -> Result<Option<String>> {
    let mut query = String::new();
    let mut selected_idx: usize = 0;
    let mut scroll: usize = 0;

    loop {
        let matches = gather_matches(state, &query);
        if selected_idx >= matches.len() {
            selected_idx = matches.len().saturating_sub(1);
        }

        terminal
            .draw(|f| draw(f, &query, &matches, selected_idx, &mut scroll))
            .map_err(|e| TuiError::InitFailed(e.to_string()))?;

        let Event::Key(key) = event::read().map_err(|e| TuiError::InitFailed(e.to_string()))?
        else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => return Ok(None),
            (KeyCode::Char('c' | 'g'), KeyModifiers::CONTROL) => return Ok(None),
            (KeyCode::Enter, _) => {
                if let Some(m) = matches.get(selected_idx) {
                    return Ok(Some(m.tmux_name.clone()));
                }
            }
            (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                selected_idx = selected_idx.saturating_sub(1);
            }
            (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL)
                if selected_idx + 1 < matches.len() =>
            {
                selected_idx += 1;
            }
            (KeyCode::Backspace, _) => {
                query.pop();
                selected_idx = 0;
                scroll = 0;
            }
            (KeyCode::Char(c), m)
                if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
            {
                query.push(c);
                selected_idx = 0;
                scroll = 0;
            }
            _ => {}
        }
    }
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

    if area.height <= 1 {
        return;
    }
    let list_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height - 1,
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
