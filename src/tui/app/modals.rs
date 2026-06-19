//! Modal rendering: help, input, confirm, error, settings, quick-switch, checkout overlays.

use super::*;

/// Place the real terminal cursor for a single-line text input whose visible
/// text starts at `(text_x, text_y)` and spans `text_width` columns. Uses
/// tui-input's wide-char-aware `visual_cursor`, clamped within the row so a long
/// value can't push the cursor past the field's edge. Preferred over splicing a
/// caret glyph into the text: the glyph reads as a stray blank when the cursor
/// sits mid-string.
fn place_input_cursor(
    frame: &mut Frame,
    input: &tui_input::Input,
    text_x: u16,
    text_y: u16,
    text_width: u16,
) {
    let col = input.visual_cursor() as u16;
    let max_x = text_x + text_width.saturating_sub(1);
    frame.set_cursor_position(((text_x + col).min(max_x), text_y));
}

impl App {
    pub(super) fn render_modal(&mut self, frame: &mut Frame, area: Rect) {
        // Record the review body geometry (depends only on `area`) so mouse
        // events can map a screen position to a diff line. Done before the
        // borrow below since it mutates `ui_state`.
        self.ui_state.review_body_rect = match self.ui_state.modal {
            Modal::ReviewDiff(_) => Some(super::review::review_body_inner_rect(area)),
            _ => None,
        };

        // Record the rows-area of any open list modal so mouse events can
        // map a click position to a list index (same pattern as
        // `review_body_rect` above).
        self.ui_state.modal_list_rect = match &self.ui_state.modal {
            Modal::QuickSwitch { matches, .. } => Some(quick_switch_areas(area, matches.len()).1),
            Modal::CheckoutBranch { filtered, .. } => {
                Some(checkout_branch_areas(area, filtered.len()).1)
            }
            Modal::PathInput { .. } => Some(path_input_areas(area).1),
            _ => None,
        };

        match &self.ui_state.modal {
            Modal::None => {}

            // Full-screen takeovers are rendered directly in `render()`, not here.
            Modal::Conversation { .. } => {}

            Modal::ReviewDiff(state) => self.render_review_modal(frame, area, state),

            Modal::Input {
                title,
                prompt,
                value,
                existing_branches,
                ..
            } => {
                // Resolve the hint up-front so we know whether to reserve a
                // line for it. Only the new-session flows populate
                // `existing_branches`; everyone else gets the original
                // 5-row layout.
                let hint = existing_branches.as_ref().and_then(|branches| {
                    crate::session::match_existing_branch(
                        value.value(),
                        &self.config.branch_prefix,
                        branches,
                    )
                    .map(|b| format!("↳ existing branch: {} — will check out", b))
                });

                // Base: 2 borders + prompt + blank + input = 5 rows. Add one
                // row when the existing-branch hint is showing so the layout
                // doesn't jump when it appears/disappears.
                let modal_width = (area.width * 60 / 100).max(40);
                let modal_height = if hint.is_some() { 6u16 } else { 5u16 };
                let modal_area = Rect {
                    x: area.x + (area.width.saturating_sub(modal_width)) / 2,
                    y: area.y + (area.height.saturating_sub(modal_height)) / 2,
                    width: modal_width,
                    height: modal_height.min(area.height),
                };
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_type(self.border_type())
                    .border_style(Style::default().fg(self.theme.modal_warning));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let mut lines: Vec<Line> = vec![
                    Line::from(prompt.as_str()),
                    Line::from(""),
                    Line::from(format!("❯ {}", value.value())),
                ];
                if let Some(h) = hint {
                    lines.push(Line::from(Span::styled(
                        h,
                        Style::default()
                            .fg(self.theme.modal_info)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
                frame.render_widget(Paragraph::new(lines), inner);
                // Input is the third line ("❯ " + value); place the real cursor.
                place_input_cursor(
                    frame,
                    value,
                    inner.x + 2,
                    inner.y + 2,
                    inner.width.saturating_sub(2),
                );
            }

            Modal::PathInput {
                title,
                prompt,
                value,
                completer,
                scroll,
                ..
            } => {
                let (modal_area, rows_area) = path_input_areas(area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_type(self.border_type())
                    .border_style(Style::default().fg(self.theme.modal_warning));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                // Prompt + input on the top three rows, completions below,
                // hint on the last row (geometry shared with the mouse
                // handler via `path_input_areas`).
                let input_area = Rect {
                    height: inner.height.min(3),
                    ..inner
                };
                let input_text = format!("{}\n\n❯ {}", prompt, value.value());
                let input_para = Paragraph::new(input_text);
                frame.render_widget(input_para, input_area);
                // The "❯ " input line is the third row of `input_area`.
                place_input_cursor(
                    frame,
                    value,
                    input_area.x + 2,
                    input_area.y + 2,
                    input_area.width.saturating_sub(2),
                );

                // Render completions list with a scroll window so the
                // highlighted row stays on-screen even when the list is
                // longer than the visible area.
                let (completions, highlighted) = completer.visible_completions();
                if !completions.is_empty() && rows_area.height > 0 {
                    let visible = rows_area.height as usize;
                    let start = (*scroll).min(completions.len());
                    let lines: Vec<Line> = completions
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(visible)
                        .map(|(abs_idx, c)| {
                            // Show just the final path component for readability
                            let display = c.rsplit('/').next().unwrap_or(c);
                            if highlighted == Some(abs_idx) {
                                Line::from(Span::styled(
                                    format!("  ❯ {}", display),
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
                    frame.render_widget(completions_para, rows_area);
                }

                if inner.height >= 5 {
                    let hint_area = Rect {
                        y: inner.y + inner.height - 1,
                        height: 1,
                        ..inner
                    };
                    let hint = Line::from(Span::styled(
                        "↑/↓ navigate  Tab complete  Enter submit  Esc cancel",
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                    frame.render_widget(Paragraph::new(hint), hint_area);
                }
            }

            Modal::Loading {
                title,
                message,
                hint,
            } => {
                let modal_area = centered_rect(60, 20, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_type(self.border_type())
                    .border_style(Style::default().fg(self.theme.modal_info));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                // Spinner on the first row; an optional dimmed hint two rows
                // below it (a blank gap keeps them visually separate).
                let throbber_area = Rect { height: 1, ..inner };

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
                frame.render_stateful_widget(
                    throbber,
                    throbber_area,
                    &mut self.ui_state.throbber_state,
                );

                if let Some(hint) = hint
                    && inner.height >= 3
                {
                    // Indent to line up under the spinner's label (the throbber
                    // glyph + space offsets the message text by two cells).
                    let hint_area = Rect {
                        x: inner.x + 2,
                        y: inner.y + 2,
                        width: inner.width.saturating_sub(2),
                        height: 1,
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            hint.as_str(),
                            Style::default()
                                .fg(ratatui::style::Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        ))),
                        hint_area,
                    );
                }
            }

            Modal::Confirm { title, message, .. } => {
                let modal_area = centered_rect(50, 15, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_type(self.border_type())
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
                    .border_type(self.border_type())
                    .border_style(Style::default().fg(self.theme.modal_error));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\nPress any key to close.", message);
                let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
                frame.render_widget(paragraph, inner);
            }

            Modal::Help { scroll } => {
                let mut offset = *scroll;
                self.render_help_modal(frame, area, &mut offset);
                if let Modal::Help { scroll } = &mut self.ui_state.modal {
                    *scroll = offset;
                }
            }

            Modal::Settings(state) => {
                self.render_settings_modal(frame, area, state);
            }

            Modal::QuickSwitch {
                mode,
                query,
                matches,
                selected_idx,
                scroll,
            } => {
                let max_visible = super::actions::LIST_MAX_VISIBLE;
                let (modal_area, rows_area) = quick_switch_areas(area, matches.len());

                frame.render_widget(Clear, modal_area);

                // Switch the modal title by effective mode so a `>`-prefixed
                // query in unified mode reads as "Commands" while we type.
                let effective_mode = App::effective_palette_mode(*mode, query.value());
                let title = match effective_mode {
                    PaletteMode::Unified => " Quick Switch ",
                    PaletteMode::CommandOnly => " Commands ",
                    PaletteMode::SectionPicker { .. } => " Move to Section ",
                };
                let block = Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_type(self.border_type())
                    .border_style(Style::default().fg(self.theme.modal_info));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                if inner.height == 0 {
                    return;
                }

                // Input line
                let input_line = Line::from(format!("❯ {}", query.value()));
                let input_area = Rect { height: 1, ..inner };
                frame.render_widget(Paragraph::new(input_line), input_area);
                place_input_cursor(
                    frame,
                    query,
                    input_area.x + 2,
                    input_area.y,
                    input_area.width.saturating_sub(2),
                );

                // Match lines. The `scroll` offset lets us page through a
                // list longer than `max_visible`; rows below `scroll` are
                // off the top of the window, rows at/after
                // `scroll + max_visible` are off the bottom.
                let start = (*scroll).min(matches.len());
                for (i, item) in matches.iter().skip(start).take(max_visible).enumerate() {
                    let row = rows_area.y + i as u16;
                    if row >= rows_area.y + rows_area.height {
                        break;
                    }
                    let abs_idx = start + i;
                    let is_selected = abs_idx == *selected_idx;

                    let line_area = Rect {
                        y: row,
                        height: 1,
                        ..inner
                    };

                    match item {
                        QuickSwitchItem::Session(m) => {
                            let status_icon = match m.status {
                                SessionStatus::Creating
                                | SessionStatus::Merging
                                | SessionStatus::Pushing => "⠋",
                                SessionStatus::Running => "●",
                                SessionStatus::Stopped => "○",
                                SessionStatus::CascadePaused => "⏸",
                            };
                            let status_color = match m.status {
                                SessionStatus::Creating
                                | SessionStatus::Merging
                                | SessionStatus::Pushing => self.theme.status_creating,
                                SessionStatus::Running => self.theme.status_running,
                                SessionStatus::Stopped => self.theme.status_stopped,
                                SessionStatus::CascadePaused => self.theme.agent_waiting,
                            };

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
                            if let Some(shown_branch) =
                                crate::session::display_branch(&m.title, &m.branch)
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
                            frame.render_widget(Paragraph::new(Line::from(spans)), line_area);
                        }
                        QuickSwitchItem::Command(entry) => {
                            // Full-row background distinguishes commands from
                            // sessions at a glance. Selection highlight takes
                            // precedence over the command background.
                            let row_style = if is_selected {
                                self.theme.selection()
                            } else {
                                Style::default()
                                    .bg(self.theme.palette_command_bg)
                                    .fg(self.theme.palette_command_fg)
                            };

                            // Reserve trailing space for the right-aligned
                            // key hint; keep one space margin on each side.
                            let available = line_area.width as usize;
                            let glyph = " ❯ ";
                            let keys = &entry.keys;
                            let keys_width = keys.chars().count();
                            let label = entry.label;
                            let label_width = label.chars().count();
                            let glyph_width = glyph.chars().count();
                            let padding = available
                                .saturating_sub(glyph_width)
                                .saturating_sub(label_width)
                                .saturating_sub(keys_width)
                                // Leave a 1-char gutter before the key hint
                                // when it's non-empty.
                                .saturating_sub(if keys.is_empty() { 0 } else { 1 });

                            let gutter = if keys.is_empty() {
                                String::new()
                            } else {
                                " ".to_string()
                            };
                            let content = format!(
                                "{glyph}{label}{pad}{gutter}{keys}",
                                glyph = glyph,
                                label = label,
                                pad = " ".repeat(padding),
                                gutter = gutter,
                                keys = keys,
                            );
                            let line = Line::from(Span::styled(content, row_style));
                            frame.render_widget(Paragraph::new(line).style(row_style), line_area);
                        }
                        QuickSwitchItem::SectionMove { label, .. } => {
                            let style = if is_selected {
                                self.theme.selection()
                            } else {
                                Style::default()
                            };
                            let line = Line::from(Span::styled(format!(" ❯ {label}"), style));
                            frame.render_widget(Paragraph::new(line).style(style), line_area);
                        }
                    }
                }
            }

            Modal::CheckoutBranch {
                query,
                filtered,
                selected_idx,
                scroll,
                fetching,
                ..
            } => {
                let (modal_area, rows_area) = checkout_branch_areas(area, filtered.len());

                frame.render_widget(Clear, modal_area);

                let title = if *fetching {
                    " Checkout Branch — fetching origin… "
                } else {
                    " Checkout Branch "
                };
                let block = Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_type(self.border_type())
                    .border_style(Style::default().fg(self.theme.modal_info));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                if inner.height == 0 {
                    return;
                }

                // Input line
                let input_line = Line::from(format!("❯ {}", query.value()));
                let input_area = Rect { height: 1, ..inner };
                frame.render_widget(Paragraph::new(input_line), input_area);
                place_input_cursor(
                    frame,
                    query,
                    input_area.x + 2,
                    input_area.y,
                    input_area.width.saturating_sub(2),
                );

                // Hint line
                let hint = if filtered.is_empty() {
                    if query.value().is_empty() {
                        "No branches found. Press Esc to cancel.".to_string()
                    } else {
                        format!(
                            "No match — press Enter to use '{}' as-is, or keep typing.",
                            query.value()
                        )
                    }
                } else {
                    format!(
                        "{} match{} — ↑/↓ to select, Enter to checkout, Esc to cancel",
                        filtered.len(),
                        if filtered.len() == 1 { "" } else { "es" }
                    )
                };
                if inner.height >= 2 {
                    let hint_area = Rect {
                        y: inner.y + 1,
                        height: 1,
                        ..inner
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            hint,
                            Style::default().fg(self.theme.text_secondary),
                        ))),
                        hint_area,
                    );
                }

                // Match lines
                let list_top = rows_area.y;
                if rows_area.height == 0 {
                    return;
                }
                let list_height = rows_area.height as usize;
                let visible_end = (scroll + list_height).min(filtered.len());
                for (i, m) in filtered[*scroll..visible_end].iter().enumerate() {
                    let row = list_top + i as u16;
                    if row >= inner.y + inner.height {
                        break;
                    }
                    let abs_idx = *scroll + i;
                    let is_selected = abs_idx == *selected_idx;
                    let marker = if m.is_remote { "⟳ " } else { "● " };
                    let marker_color = if m.is_remote {
                        self.theme.text_secondary
                    } else {
                        self.theme.status_running
                    };

                    let spans = vec![
                        Span::styled(format!(" {}", marker), Style::default().fg(marker_color)),
                        Span::styled(
                            m.display_name.clone(),
                            if is_selected {
                                self.theme.selection()
                            } else {
                                Style::default()
                            },
                        ),
                    ];
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
            "  {:<width$}Quick switch — sessions and commands",
            leader_display,
            width = key_col_width,
        )));
        lines.push(Line::from(format!(
            "  {:<width$}Quick switch (same as in-session switcher)",
            "Ctrl+Space",
            width = key_col_width,
        )));
        lines.push(Line::from(format!(
            "  {:<width$}Command palette (commands only)",
            format!("Shift+{leader_display}"),
            width = key_col_width,
        )));
        lines.push(Line::from(format!(
            "  {:<width$}Filter palette to commands only",
            ">",
            width = key_col_width,
        )));

        // Global voice hotkey (a desktop shortcut, not an in-app keybinding).
        lines.push(Line::from(""));
        lines.push(Line::from("Global Voice Hotkey:"));
        lines.push(Line::from(format!(
            "  {:<width$}Toggle voice input system-wide. Linux: set the key",
            "system-wide",
            width = key_col_width,
        )));
        lines.push(Line::from(format!(
            "  {:<width$}in System Settings ▸ Shortcuts (toggle-voice);",
            "",
            width = key_col_width,
        )));
        lines.push(Line::from(format!(
            "  {:<width$}macOS: bind `claude-commander listen-toggle`",
            "",
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
            Span::styled("○", Style::default().fg(self.theme.status_stopped)),
            Span::raw("  Stopped"),
        ]));

        // PR badges legend
        lines.push(Line::from(""));
        lines.push(Line::from("PR Badges:"));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.pr_open)),
            Span::raw("  Open"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.status_pr)),
            Span::raw("  Open — awaiting review"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.pr_draft)),
            Span::raw("  Draft"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.pr_closed)),
            Span::raw("  Closed"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.status_pr_merged)),
            Span::raw("  Merged"),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(
            "Esc/Enter/q/? to close · ↑/↓ k/j to scroll · PgUp/PgDn · Home/End",
        ));

        lines
    }

    pub(super) fn render_help_modal(&mut self, frame: &mut Frame, area: Rect, scroll: &mut u16) {
        let modal_area = centered_rect(70, 80, area);
        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(" Help ")
            .borders(Borders::ALL)
            .border_type(self.border_type())
            .border_style(Style::default().fg(self.theme.modal_info));
        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let content_area = inner.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });

        let help_lines = self.build_help_lines();
        let total_lines = help_lines.len() as u16;
        let visible = content_area.height;
        let max_scroll = total_lines.saturating_sub(visible);

        if *scroll > max_scroll {
            *scroll = max_scroll;
        }
        let offset = *scroll;

        let paragraph = Paragraph::new(help_lines).scroll((offset, 0));
        frame.render_widget(paragraph, content_area);

        if max_scroll > 0 {
            // ratatui 0.29's Scrollbar treats `content_length - 1` as the
            // max scroll position (scrollbar.rs:562). Passing the full line
            // count leaves the thumb short of the bottom at max scroll —
            // use the number of distinct scroll positions instead so the
            // thumb hits the track ends at offset=0 and offset=max_scroll.
            let mut sb_state = ScrollbarState::new(max_scroll as usize + 1)
                .position(offset as usize)
                .viewport_content_length(visible as usize);
            let scrollbar = Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None);
            frame.render_stateful_widget(scrollbar, content_area, &mut sb_state);
        }
    }
}

/// Helper to center a rect within an area
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

// List-modal geometry, shared between the render arms and the mouse handler
// so a click maps onto exactly the rows the renderer drew. Each `*_areas`
// function returns `(modal_area, rows_area)` where `rows_area` covers only
// the selectable list rows (zero-height when the modal is too small to show
// any).

/// Geometry of the quick-switch palette for `n_matches` filtered rows.
pub(super) fn quick_switch_areas(area: Rect, n_matches: usize) -> (Rect, Rect) {
    let visible = n_matches.min(super::actions::LIST_MAX_VISIBLE);
    // Dynamic height: border(2) + input(1) + rows, positioned in the
    // upper third of the screen.
    let modal_height = (3 + visible) as u16;
    let modal_width = (area.width * 60 / 100).max(40);
    let modal_area = Rect {
        x: area.x + (area.width.saturating_sub(modal_width)) / 2,
        y: area.y + area.height / 5,
        width: modal_width,
        height: modal_height.min(area.height),
    };
    let inner = modal_area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let rows = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };
    (modal_area, rows)
}

/// Geometry of the checkout-branch modal for `n_filtered` branch rows.
pub(super) fn checkout_branch_areas(area: Rect, n_filtered: usize) -> (Rect, Rect) {
    // Target up to 12 visible branch rows, but always at least one row of
    // space so the "no match" state doesn't collapse the modal.
    let desired_visible = n_filtered.clamp(1, 12);
    // border(2) + input(1) + hint(1) + rows
    let modal_height = (4 + desired_visible) as u16;
    let modal_width = (area.width * 70 / 100).max(50);
    let modal_area = Rect {
        x: area.x + (area.width.saturating_sub(modal_width)) / 2,
        y: area.y + area.height / 6,
        width: modal_width,
        height: modal_height.min(area.height),
    };
    let inner = modal_area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let rows = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    (modal_area, rows)
}

/// Geometry of the path-input modal. Fixed height: border(2) +
/// prompt/input(3) + LIST_MAX_VISIBLE rows + hint(1) when the full window
/// fits, capped to the terminal height. Keeps the modal size predictable so
/// navigation (which assumes LIST_MAX_VISIBLE) lines up with the rendered
/// window.
pub(super) fn path_input_areas(area: Rect) -> (Rect, Rect) {
    let list_rows = super::actions::LIST_MAX_VISIBLE as u16;
    let modal_height: u16 = (2 + 3 + list_rows + 1).min(area.height.max(1));
    let modal_width = (area.width * 60 / 100).max(50);
    let modal_area = Rect {
        x: area.x + (area.width.saturating_sub(modal_width)) / 2,
        y: area.y + (area.height.saturating_sub(modal_height)) / 2,
        width: modal_width,
        height: modal_height,
    };
    let inner = modal_area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let rows = Rect {
        y: inner.y + 3,
        height: inner.height.saturating_sub(4),
        ..inner
    };
    (modal_area, rows)
}

/// Map a mouse position to an absolute list index. `rows` is the rows-only
/// area recorded at render time, `scroll` the index of the first visible
/// row, `len` the list length. Returns `None` for positions outside `rows`
/// or on an unpopulated row below the end of the list.
pub(super) fn modal_list_index_at(
    col: u16,
    row: u16,
    rows: Rect,
    scroll: usize,
    len: usize,
) -> Option<usize> {
    let inside =
        col >= rows.x && col < rows.x + rows.width && row >= rows.y && row < rows.y + rows.height;
    if !inside {
        return None;
    }
    let idx = scroll + (row - rows.y) as usize;
    (idx < len).then_some(idx)
}
