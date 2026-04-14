//! Modal rendering: input, confirm, error, help, loading, quick-switch overlays.

use super::*;

impl App {
    pub(super) fn render_modal(&mut self, frame: &mut Frame, area: Rect) {
        match &self.ui_state.modal {
            Modal::None => {}

            Modal::Input {
                title,
                prompt,
                value,
                ..
            } => {
                let modal_area = centered_rect(60, 20, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_warning));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\n> {}_", prompt, value);
                let paragraph = Paragraph::new(text);
                frame.render_widget(paragraph, inner);
            }

            Modal::PathInput {
                title,
                prompt,
                value,
                completer,
                ..
            } => {
                let modal_area = centered_rect(60, 40, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_warning));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                // Split: prompt+input at top, completions below, hint at bottom
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // prompt + input
                        Constraint::Min(1),    // completions list
                        Constraint::Length(1), // hint line
                    ])
                    .split(inner);

                let input_text = format!("{}\n\n> {}_", prompt, value);
                let input_para = Paragraph::new(input_text);
                frame.render_widget(input_para, chunks[0]);

                // Render completions list
                let (completions, highlighted) = completer.visible_completions();
                if !completions.is_empty() {
                    let lines: Vec<Line> = completions
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            // Show just the final path component for readability
                            let display = c.rsplit('/').next().unwrap_or(c);
                            if highlighted == Some(i) {
                                Line::from(Span::styled(
                                    format!("  > {}", display),
                                    Style::default()
                                        .fg(self.theme.modal_info)
                                        .add_modifier(Modifier::BOLD),
                                ))
                            } else {
                                Line::from(format!("    {}", display))
                            }
                        })
                        .collect();
                    let completions_para = Paragraph::new(lines);
                    frame.render_widget(completions_para, chunks[1]);
                }

                let hint = Line::from(Span::styled(
                    "[Tab] complete  [Enter] submit  [Esc] cancel",
                    Style::default().add_modifier(Modifier::DIM),
                ));
                frame.render_widget(Paragraph::new(hint), chunks[2]);
            }

            Modal::Loading { title, message } => {
                let modal_area = centered_rect(60, 20, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_info));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                const RAINBOW: &[ratatui::style::Color] = &[
                    ratatui::style::Color::Red,
                    ratatui::style::Color::Yellow,
                    ratatui::style::Color::Green,
                    ratatui::style::Color::Cyan,
                    ratatui::style::Color::Blue,
                    ratatui::style::Color::Magenta,
                ];
                let color = RAINBOW[self.ui_state.throbber_state.index() as usize % RAINBOW.len()];
                let throbber = throbber_widgets_tui::Throbber::default()
                    .throbber_set(throbber_widgets_tui::symbols::throbber::BRAILLE_EIGHT)
                    .label(message.as_str())
                    .throbber_style(Style::default().fg(color));
                frame.render_stateful_widget(throbber, inner, &mut self.ui_state.throbber_state);
            }

            Modal::Confirm { title, message, .. } => {
                let modal_area = centered_rect(50, 15, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_error));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\n[Enter] Confirm  [Esc] Cancel", message);
                let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
                frame.render_widget(paragraph, inner);
            }

            Modal::Error { message } => {
                let modal_area = centered_rect(60, 20, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(" Error ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_error));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\nPress any key to close.", message);
                let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
                frame.render_widget(paragraph, inner);
            }

            Modal::Help => {
                let modal_area = centered_rect(70, 80, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(" Help ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_info));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                // Add margin inside the modal for better readability
                let content_area = inner.inner(Margin {
                    horizontal: 2,
                    vertical: 1,
                });

                let help_lines = self.build_help_lines();

                let paragraph = Paragraph::new(help_lines);
                frame.render_widget(paragraph, content_area);
            }

            Modal::Settings(state) => {
                self.render_settings_modal(frame, area, state);
            }

            Modal::QuickSwitch {
                query,
                matches,
                selected_idx,
            } => {
                let max_visible = 10;
                let visible_matches = matches.len().min(max_visible);
                // Dynamic height: border(2) + input(1) + matches
                let modal_height = (3 + visible_matches) as u16;
                let modal_width = (area.width * 60 / 100).max(40);

                // Position in upper third
                let modal_area = Rect {
                    x: area.x + (area.width.saturating_sub(modal_width)) / 2,
                    y: area.y + area.height / 5,
                    width: modal_width,
                    height: modal_height.min(area.height),
                };

                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(" Quick Switch ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_info));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                if inner.height == 0 {
                    return;
                }

                // Input line
                let input_line = Line::from(format!("> {}_", query));
                let input_area = Rect { height: 1, ..inner };
                frame.render_widget(Paragraph::new(input_line), input_area);

                // Match lines
                for (i, m) in matches.iter().take(max_visible).enumerate() {
                    let row = inner.y + 1 + i as u16;
                    if row >= inner.y + inner.height {
                        break;
                    }

                    let status_icon = match m.status {
                        SessionStatus::Creating => "⠋",
                        SessionStatus::Running => "●",
                        SessionStatus::Stopped => "○",
                    };
                    let status_color = match m.status {
                        SessionStatus::Creating => self.theme.status_creating,
                        SessionStatus::Running => self.theme.status_running,
                        SessionStatus::Stopped => self.theme.status_stopped,
                    };

                    let is_selected = i == *selected_idx;
                    let mut spans = vec![
                        Span::styled(
                            format!(" {} ", status_icon),
                            Style::default().fg(status_color),
                        ),
                        Span::styled(
                            m.title.clone(),
                            if is_selected {
                                self.theme.selection()
                            } else {
                                Style::default()
                            },
                        ),
                    ];
                    if let Some(shown_branch) = crate::session::display_branch(&m.title, &m.branch)
                    {
                        spans.push(Span::styled(
                            format!(" [{}]", shown_branch),
                            Style::default().fg(self.theme.text_accent),
                        ));
                    }
                    spans.push(Span::styled(
                        format!(" ({})", m.project_name),
                        Style::default().fg(self.theme.text_secondary),
                    ));
                    let line = Line::from(spans);

                    let line_area = Rect {
                        y: row,
                        height: 1,
                        ..inner
                    };
                    frame.render_widget(Paragraph::new(line), line_area);
                }
            }
        }
    }

    pub(super) fn build_help_lines(&self) -> Vec<Line<'static>> {
        let kb = &self.config.keybindings;
        let mut lines: Vec<Line<'static>> = Vec::new();
        let key_col_width = 18;

        for (section_name, actions) in kb.sections() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(format!("{section_name}:")));

            for (action, keys_str) in &actions {
                let desc = action.description();
                let padded_keys = format!("  {keys_str:<width$}{desc}", width = key_col_width);
                lines.push(Line::from(padded_keys));
            }
        }

        // Quick-switch (hardcoded since leader_key is in config, not keybindings)
        lines.push(Line::from(""));
        lines.push(Line::from("Quick Switch:"));
        let leader_display =
            if self.config.leader_key.trim().is_empty() || self.config.leader_key == " " {
                "Space".to_string()
            } else {
                self.config.leader_key.clone()
            };
        lines.push(Line::from(format!(
            "  {:<width$}Fuzzy session search",
            leader_display,
            width = key_col_width,
        )));

        // Status indicators (not keybinding-related, stays hardcoded)
        lines.push(Line::from(""));
        lines.push(Line::from("Status Indicators:"));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.status_running)),
            Span::raw("  Running (agent active)"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.status_pr)),
            Span::raw("  PR open"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.status_pr_merged)),
            Span::raw("  PR merged"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("○", Style::default().fg(self.theme.status_stopped)),
            Span::raw("  Stopped"),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from("Press any key to close this help."));

        lines
    }
}

pub(super) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
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
