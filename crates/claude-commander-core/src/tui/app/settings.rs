//! Settings modal: row building, rendering, editing, and key handling.

use super::*;

impl App {
    pub(super) fn build_settings_rows(&self, tab: SettingsTab) -> Vec<SettingsRow> {
        match tab {
            SettingsTab::General => {
                // Grouped into logical sections, each preceded by a
                // non-selectable header row; `with_section_spacers` inserts a
                // blank line between groups so the long list is easy to scan
                // (mirrors the Keybindings tab).
                let c = &self.config;
                with_section_spacers(vec![
                    SettingsRow::header("Sessions & Worktrees"),
                    SettingsRow::text(
                        "Default Program",
                        c.default_program.clone(),
                        "default_program",
                    ),
                    SettingsRow::text(
                        "Branch Prefix",
                        if c.branch_prefix.is_empty() {
                            "(none)".to_string()
                        } else {
                            c.branch_prefix.clone()
                        },
                        "branch_prefix",
                    ),
                    SettingsRow::text("Shell Program", c.shell_program.clone(), "shell_program"),
                    SettingsRow::text(
                        "Worktrees Directory",
                        c.worktrees_dir
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "(default)".into()),
                        "worktrees_dir",
                    ),
                    SettingsRow::toggle(
                        "Per-Repo Worktree Dirs",
                        c.per_repo_worktree_dirs,
                        "per_repo_worktree_dirs",
                    ),
                    SettingsRow::toggle(
                        "Fetch Before Create",
                        c.fetch_before_create,
                        "fetch_before_create",
                    ),
                    SettingsRow::toggle("Skip LFS Smudge", c.skip_lfs_smudge, "skip_lfs_smudge"),
                    SettingsRow::toggle("Resume Session", c.resume_session, "resume_session"),
                    SettingsRow::toggle("Nix Develop", c.nix_develop, "nix_develop"),
                    SettingsRow::text(
                        "In Progress WIP Limit",
                        c.in_progress_limit
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "(unlimited)".into()),
                        "in_progress_limit",
                    ),
                    SettingsRow::header("Editor"),
                    SettingsRow::text(
                        "Editor",
                        c.editor.clone().unwrap_or_else(|| "(auto)".into()),
                        "editor",
                    ),
                    SettingsRow::text(
                        "Editor is GUI",
                        match c.editor_gui {
                            Some(true) => "true",
                            Some(false) => "false",
                            None => "(auto)",
                        },
                        "editor_gui",
                    ),
                    SettingsRow::header("Pull Requests & Sync"),
                    SettingsRow::text(
                        "PR Check Interval (s)",
                        c.pr_check_interval_secs.to_string(),
                        "pr_check_interval_secs",
                    ),
                    SettingsRow::toggle(
                        "Project Pull Enabled",
                        c.project_pull_enabled,
                        "project_pull_enabled",
                    ),
                    SettingsRow::text(
                        "Project Pull Interval (s)",
                        c.project_pull_interval_secs.to_string(),
                        "project_pull_interval_secs",
                    ),
                    SettingsRow::header("Appearance"),
                    SettingsRow::toggle(
                        "Dim Unfocused Preview",
                        c.dim_unfocused_preview,
                        "dim_unfocused_preview",
                    ),
                    SettingsRow::text(
                        "Dim Opacity",
                        format!("{:.2}", c.dim_unfocused_opacity),
                        "dim_unfocused_opacity",
                    ),
                    SettingsRow::toggle(
                        "Invert PR Label Color",
                        c.invert_pr_label_color,
                        "invert_pr_label_color",
                    ),
                    SettingsRow::toggle(
                        "Show Session Program",
                        c.show_session_program,
                        "show_session_program",
                    ),
                    SettingsRow::toggle("Rounded Borders", c.rounded_borders, "rounded_borders"),
                    SettingsRow::header("Performance"),
                    SettingsRow::text(
                        "UI Refresh FPS",
                        c.ui_refresh_fps.to_string(),
                        "ui_refresh_fps",
                    ),
                    SettingsRow::text(
                        "Max Concurrent Tmux",
                        c.max_concurrent_tmux.to_string(),
                        "max_concurrent_tmux",
                    ),
                    SettingsRow::toggle(
                        "Precompute Review Caches",
                        c.precompute_review_caches,
                        "precompute_review_caches",
                    ),
                    SettingsRow::text(
                        "Number Debounce (ms)",
                        c.session_number_debounce_ms.to_string(),
                        "session_number_debounce_ms",
                    ),
                    SettingsRow::header("AI Summaries"),
                    SettingsRow::toggle(
                        "AI Summary Enabled",
                        c.ai_summary_enabled,
                        "ai_summary_enabled",
                    ),
                    SettingsRow::text(
                        "AI Summary Model",
                        c.ai_summary_model.clone(),
                        "ai_summary_model",
                    ),
                    SettingsRow::header("Commander"),
                    SettingsRow::toggle(
                        "Commander Enabled",
                        c.commander_enabled,
                        "commander_enabled",
                    ),
                    SettingsRow::text(
                        "Commander Program",
                        c.commander_program
                            .clone()
                            .unwrap_or_else(|| "(default)".into()),
                        "commander_program",
                    ),
                    SettingsRow::text(
                        "Commander Directory",
                        c.commander_dir
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "(default)".into()),
                        "commander_dir",
                    ),
                    SettingsRow::header("Privacy"),
                    SettingsRow::toggle(
                        "Usage Telemetry",
                        c.telemetry.enabled,
                        "telemetry_enabled",
                    ),
                ])
            }
            SettingsTab::Conversation => {
                let c = &self.config.conversation;
                let s = &self.config.stt;
                vec![
                    SettingsRow::toggle(
                        "Enable Conversation Mode",
                        c.enabled,
                        "conversation_enabled",
                    ),
                    SettingsRow::text("Assistant Name", c.name.clone(), "conversation_name"),
                    SettingsRow::text("TTS Base URL", c.base_url.clone(), "conversation_base_url"),
                    SettingsRow::text("Model", c.model.clone(), "conversation_model"),
                    SettingsRow::text(
                        "Voice",
                        c.voice.clone().unwrap_or_else(|| "(default)".into()),
                        "conversation_voice",
                    ),
                    SettingsRow::text(
                        "Response Format",
                        c.response_format.clone(),
                        "conversation_format",
                    ),
                    SettingsRow::text("Speed", format!("{:.2}", c.speed), "conversation_speed"),
                    SettingsRow::text("Volume", format!("{:.2}", c.volume), "conversation_volume"),
                    SettingsRow::text(
                        "Speak Scope",
                        c.speak_scope.label().to_string(),
                        "conversation_speak_scope",
                    ),
                    // Speech-to-text (voice input, Alt-V).
                    SettingsRow::toggle("Enable Voice Input (STT)", s.enabled, "stt_enabled"),
                    SettingsRow::text("STT Base URL", s.base_url.clone(), "stt_base_url"),
                    SettingsRow::text("STT Model", s.model.clone(), "stt_model"),
                    SettingsRow::text(
                        "STT Language",
                        s.language.clone().unwrap_or_else(|| "(auto)".into()),
                        "stt_language",
                    ),
                    SettingsRow::text(
                        "STT Prompt",
                        s.prompt.clone().unwrap_or_else(|| "(none)".into()),
                        "stt_prompt",
                    ),
                    SettingsRow::toggle(
                        "Pause Media While Recording",
                        s.pause_media,
                        "stt_pause_media",
                    ),
                ]
            }
            SettingsTab::Sections => {
                vec![]
            }
            SettingsTab::Keybindings => {
                // Grouped into logical sections (see `BindableAction::section`),
                // each preceded by a non-selectable header row and a blank
                // spacer so the long list is easy to scan.
                let kb = &self.config.keybindings;
                let mut rows = Vec::new();
                for (section, actions) in kb.sections() {
                    rows.push(SettingsRow::header(section));
                    for (action, keys) in actions {
                        rows.push(SettingsRow::text(
                            action.description(),
                            keys,
                            action.config_name(),
                        ));
                    }
                }
                with_section_spacers(rows)
            }
            SettingsTab::Theme => {
                // Show the current resolved color for each overridable field,
                // and whether it has a user override.
                let t = &self.theme;
                let o = &self.config.theme;

                macro_rules! theme_row {
                    ($label:expr, $field:ident) => {
                        SettingsRow::swatch(
                            $label,
                            o.$field
                                .map(|cv| {
                                    let s = toml::to_string(&cv).unwrap_or_default();
                                    s.trim().trim_matches('"').to_string()
                                })
                                .unwrap_or_else(|| format_color(t.$field)),
                            stringify!($field),
                            t.$field,
                        )
                    };
                }

                vec![
                    SettingsRow::text(
                        "Preset",
                        o.preset.clone().unwrap_or_else(|| "(auto)".into()),
                        "preset",
                    ),
                    theme_row!("Border Focused", border_focused),
                    theme_row!("Border Unfocused", border_unfocused),
                    theme_row!("Selection BG", selection_bg),
                    theme_row!("Status Running", status_running),
                    theme_row!("Status Stopped", status_stopped),
                    theme_row!("Status PR", status_pr),
                    theme_row!("Status PR Merged", status_pr_merged),
                    theme_row!("PR Open", pr_open),
                    theme_row!("PR Draft", pr_draft),
                    theme_row!("PR Closed", pr_closed),
                    theme_row!("Text Primary", text_primary),
                    theme_row!("Text Secondary", text_secondary),
                    theme_row!("Text Accent", text_accent),
                    theme_row!("Diff Added", diff_added),
                    theme_row!("Diff Removed", diff_removed),
                    theme_row!("Diff Hunk Header", diff_hunk_header),
                    theme_row!("Diff File Header", diff_file_header),
                    theme_row!("Modal Info", modal_info),
                    theme_row!("Modal Warning", modal_warning),
                    theme_row!("Modal Error", modal_error),
                    theme_row!("Status Bar BG", status_bar_bg),
                    theme_row!("Status Bar FG", status_bar_fg),
                ]
            }
        }
    }

    /// Render the settings modal.
    pub(super) fn render_settings_modal(
        &self,
        frame: &mut Frame,
        area: Rect,
        state: &SettingsState,
    ) {
        let modal_area = modals::centered_rect(75, 85, area);
        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(" Settings ")
            .borders(Borders::ALL)
            .border_type(self.border_type())
            .border_style(Style::default().fg(self.theme.modal_info));
        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let content_area = inner.inner(Margin {
            horizontal: 1,
            vertical: 0,
        });

        if content_area.height < 4 {
            return;
        }

        // --- Tab bar (row 0) ---
        let tab_area = Rect {
            height: 1,
            ..content_area
        };
        let mut tab_spans: Vec<Span> = Vec::new();
        for (i, tab) in SettingsTab::ALL.iter().enumerate() {
            if i > 0 {
                tab_spans.push(Span::raw("  "));
            }
            let style = if *tab == state.tab {
                Style::default()
                    .fg(self.theme.text_primary)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(self.theme.text_secondary)
            };
            tab_spans.push(Span::styled(tab.label(), style));
        }
        frame.render_widget(Paragraph::new(Line::from(tab_spans)), tab_area);

        // --- Separator ---
        let sep_area = Rect {
            y: content_area.y + 1,
            height: 1,
            ..content_area
        };
        let separator = "─".repeat(content_area.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                separator,
                Style::default().fg(self.theme.border_unfocused),
            ))),
            sep_area,
        );

        // --- Body area (between separator and footer) ---
        let body_area = Rect {
            y: content_area.y + 2,
            height: content_area.height.saturating_sub(4),
            ..content_area
        };

        // --- Footer ---
        let footer_area = Rect {
            y: content_area.y + content_area.height.saturating_sub(1),
            height: 1,
            ..content_area
        };

        if state.tab == SettingsTab::Sections {
            self.render_sections_tab(frame, body_area, footer_area, &state.sections_state);
        } else {
            self.render_settings_rows(frame, body_area, footer_area, state);
        }
    }

    fn render_settings_rows(
        &self,
        frame: &mut Frame,
        rows_area: Rect,
        footer_area: Rect,
        state: &SettingsState,
    ) {
        // When the Keybindings search box is focused, reserve the top line for
        // the filter input and shrink the list below it.
        let rows_area = if let Some(query) = &state.search {
            let search_area = Rect {
                height: 1,
                ..rows_area
            };
            let prompt = format!("/{}", super::input_with_caret(query));
            frame.render_widget(
                Paragraph::new(Span::styled(
                    prompt,
                    Style::default()
                        .fg(self.theme.text_accent)
                        .add_modifier(Modifier::BOLD),
                )),
                search_area,
            );
            Rect {
                y: rows_area.y + 1,
                height: rows_area.height.saturating_sub(1),
                ..rows_area
            }
        } else {
            rows_area
        };

        if state.rows.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "  (no matching shortcuts)",
                    Style::default().fg(self.theme.text_secondary),
                )),
                rows_area,
            );
            self.render_settings_footer(frame, footer_area, state);
            return;
        }

        let label_width = settings_label_width(&state.rows, rows_area.width);
        let value_width = rows_area.width.saturating_sub(label_width + 3);

        let visible_rows = rows_area.height as usize;
        let scroll_offset = if state.selected_row >= visible_rows {
            state.selected_row - visible_rows + 1
        } else {
            0
        };

        // Check if the OptionPicker is active and how many rows it occupies
        let picker_info: Option<(usize, &[String], usize)> =
            if let Some(SettingsEditing::OptionPicker { options, selected }) = &state.editing {
                // screen_row is the row index (within visible area) where the picker starts
                let screen_row = state.selected_row.saturating_sub(scroll_offset);
                Some((screen_row, options.as_slice(), *selected))
            } else {
                None
            };

        // How many rows the picker will overlay (starting from the selected row)
        let picker_row_count = picker_info
            .map(|(screen_row, opts, _)| {
                let rows_below = visible_rows.saturating_sub(screen_row);
                opts.len().min(rows_below)
            })
            .unwrap_or(0);

        for (i, row) in state
            .rows
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_rows)
        {
            let screen_idx = i - scroll_offset;
            let y = rows_area.y + screen_idx as u16;
            let is_selected = i == state.selected_row;

            // If the OptionPicker is open, skip rendering normal rows that are
            // overlaid by picker options (except the first picker row itself,
            // which replaces the selected row).
            if let Some((picker_screen_row, _, _)) = picker_info
                && screen_idx > picker_screen_row
                && screen_idx < picker_screen_row + picker_row_count
            {
                continue;
            }

            // Section headers span the full width and are never selectable.
            if matches!(row.kind, SettingsRowKind::Header) {
                let header_area = Rect {
                    x: rows_area.x,
                    y,
                    width: rows_area.width,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        row.label.clone(),
                        Style::default()
                            .fg(self.theme.text_accent)
                            .add_modifier(Modifier::BOLD),
                    )),
                    header_area,
                );
                continue;
            }

            let row_style = if is_selected {
                self.theme.selection()
            } else {
                Style::default()
            };

            // Label
            let label_area = Rect {
                x: rows_area.x,
                y,
                width: label_width.min(rows_area.width),
                height: 1,
            };
            let label = format!("{:<width$}", row.label, width = label_width as usize);
            frame.render_widget(Paragraph::new(Span::styled(label, row_style)), label_area);

            // Color swatch + Value
            if rows_area.width > label_width + 2 {
                let swatch_width: u16 = if row.color_swatch.is_some() { 3 } else { 0 };
                let val_x = rows_area.x + label_width + 2;

                // Render color swatch if present
                if let Some(swatch_color) = row.color_swatch {
                    let swatch_area = Rect {
                        x: val_x,
                        y,
                        width: swatch_width.min(value_width),
                        height: 1,
                    };
                    let swatch_style = if is_selected {
                        Style::default()
                            .fg(swatch_color)
                            .bg(self.theme.selection_bg)
                    } else {
                        Style::default().fg(swatch_color)
                    };
                    frame.render_widget(
                        Paragraph::new(Span::styled("██ ", swatch_style)),
                        swatch_area,
                    );
                }

                let val_area = Rect {
                    x: val_x + swatch_width,
                    y,
                    width: value_width.saturating_sub(swatch_width),
                    height: 1,
                };

                let display_val = match &row.kind {
                    // Toggles never enter an editing state; render as a checkbox.
                    SettingsRowKind::Toggle(on) => {
                        if *on {
                            "[x]".to_string()
                        } else {
                            "[ ]".to_string()
                        }
                    }
                    SettingsRowKind::Text(text) if is_selected => {
                        if let Some(SettingsEditing::TextInput { value }) = &state.editing {
                            super::input_with_caret(value)
                        } else if let Some(SettingsEditing::OptionPicker { options, selected }) =
                            &state.editing
                        {
                            // Show the currently highlighted option on the selected row
                            format!("▸ {}", options[*selected])
                        } else {
                            text.clone()
                        }
                    }
                    SettingsRowKind::Text(text) => text.clone(),
                    // Headers `continue` above and never reach the value column.
                    SettingsRowKind::Header => String::new(),
                };

                let val_style = if !matches!(row.kind, SettingsRowKind::Toggle(_))
                    && is_selected
                    && state.editing.is_some()
                {
                    row_style.add_modifier(Modifier::UNDERLINED)
                } else {
                    row_style.fg(self.theme.text_accent)
                };

                frame.render_widget(
                    Paragraph::new(Span::styled(display_val, val_style)),
                    val_area,
                );
            }
        }

        // Render the OptionPicker dropdown rows below the selected row
        if let Some((picker_screen_row, options, selected_opt)) = picker_info {
            let val_x = rows_area.x + label_width + 2;
            let val_w = value_width;

            for (opt_idx, option) in options.iter().enumerate().take(picker_row_count) {
                let row_y = rows_area.y + (picker_screen_row + opt_idx) as u16;
                let is_highlighted = opt_idx == selected_opt;

                // Clear the label area for overlay rows beyond the first
                if opt_idx > 0 {
                    let clear_area = Rect {
                        x: rows_area.x,
                        y: row_y,
                        width: label_width.min(rows_area.width),
                        height: 1,
                    };
                    frame.render_widget(Clear, clear_area);
                    frame.render_widget(Paragraph::new(Span::raw("")), clear_area);
                }

                let opt_area = Rect {
                    x: val_x,
                    y: row_y,
                    width: val_w,
                    height: 1,
                };

                // Clear before rendering
                frame.render_widget(Clear, opt_area);

                let prefix = if is_highlighted { "▸ " } else { "  " };
                let opt_style = if is_highlighted {
                    self.theme.selection()
                } else {
                    Style::default().fg(self.theme.text_accent)
                };

                frame.render_widget(
                    Paragraph::new(Span::styled(format!("{prefix}{option}"), opt_style)),
                    opt_area,
                );
            }
        }

        self.render_settings_footer(frame, footer_area, state);
    }

    /// Footer hint line for the settings rows view.
    fn render_settings_footer(&self, frame: &mut Frame, footer_area: Rect, state: &SettingsState) {
        let selected_is_toggle = state
            .rows
            .get(state.selected_row)
            .is_some_and(|r| matches!(r.kind, SettingsRowKind::Toggle(_)));
        let footer_text = if state.search.is_some() {
            "Type to filter  ↑/↓: navigate  Enter: keep  Esc: clear"
        } else if state.editing.is_some() {
            match &state.editing {
                Some(SettingsEditing::OptionPicker { .. }) => {
                    "j/k: navigate  Enter: select  Esc: cancel"
                }
                _ => "Enter: save  Esc: cancel",
            }
        } else if state.tab == SettingsTab::Keybindings {
            "Tab: switch tab  j/k: navigate  /: search  Esc: close"
        } else if selected_is_toggle {
            "Tab: switch tab  j/k: navigate  Space/Enter: toggle  Esc: close"
        } else {
            "Tab: switch tab  j/k: navigate  Enter: edit  Esc: close"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                footer_text,
                Style::default().fg(self.theme.text_secondary),
            )),
            footer_area,
        );
    }

    fn render_sections_tab(
        &self,
        frame: &mut Frame,
        body_area: Rect,
        footer_area: Rect,
        sec: &SectionsState,
    ) {
        let sections = &self.config.sections;
        let list_width = body_area.width.clamp(16, 28);
        let divider_width = 1_u16;
        let pred_width = body_area
            .width
            .saturating_sub(list_width + divider_width + 1);

        let list_area = Rect {
            width: list_width,
            ..body_area
        };
        let divider_area = Rect {
            x: body_area.x + list_width,
            width: divider_width,
            ..body_area
        };
        let pred_area = Rect {
            x: body_area.x + list_width + divider_width + 1,
            width: pred_width,
            ..body_area
        };

        // --- Divider ---
        for row in 0..body_area.height {
            let y = divider_area.y + row;
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "│",
                    Style::default().fg(self.theme.border_unfocused),
                )),
                Rect {
                    x: divider_area.x,
                    y,
                    width: 1,
                    height: 1,
                },
            );
        }

        // --- Section list ---
        if let Some(SectionsEditing::CreatingSection { value }) = &sec.editing {
            let visible = list_area.height as usize;
            let name_rows = sections.len().min(visible.saturating_sub(1));
            for (i, section) in sections.iter().enumerate().take(name_rows) {
                let y = list_area.y + i as u16;
                let style = Style::default().fg(self.theme.text_secondary);
                let name = truncate_str(&section.name, list_width as usize - 2);
                frame.render_widget(
                    Paragraph::new(Span::styled(format!("  {name}"), style)),
                    Rect {
                        y,
                        height: 1,
                        ..list_area
                    },
                );
            }
            let input_y = list_area.y + name_rows as u16;
            let input_style = self.theme.selection().add_modifier(Modifier::UNDERLINED);
            let display = format!("  {}", super::input_with_caret(value));
            frame.render_widget(
                Paragraph::new(Span::styled(display, input_style)),
                Rect {
                    y: input_y,
                    height: 1,
                    ..list_area
                },
            );
        } else {
            let visible = list_area.height as usize;
            let scroll = if sec.selected_section >= visible {
                sec.selected_section - visible + 1
            } else {
                0
            };
            for (i, section) in sections.iter().enumerate().skip(scroll).take(visible) {
                let y = list_area.y + (i - scroll) as u16;
                let is_selected = i == sec.selected_section;
                let is_focused = sec.focus == SectionsFocus::List;

                let style = if is_selected && is_focused {
                    self.theme.selection()
                } else if is_selected {
                    Style::default()
                        .fg(self.theme.text_primary)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.theme.text_secondary)
                };

                let prefix = if is_selected { "▸ " } else { "  " };
                let name = if is_selected {
                    if let Some(SectionsEditing::RenamingSection { value }) = &sec.editing {
                        format!("{prefix}{}", super::input_with_caret(value))
                    } else {
                        let n = truncate_str(&section.name, list_width as usize - 2);
                        format!("{prefix}{n}")
                    }
                } else {
                    let n = truncate_str(&section.name, list_width as usize - 2);
                    format!("{prefix}{n}")
                };

                let row_style = if is_selected
                    && matches!(sec.editing, Some(SectionsEditing::RenamingSection { .. }))
                {
                    style.add_modifier(Modifier::UNDERLINED)
                } else {
                    style
                };

                frame.render_widget(
                    Paragraph::new(Span::styled(name, row_style)),
                    Rect {
                        y,
                        height: 1,
                        ..list_area
                    },
                );
            }

            if sections.is_empty() {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "  (no sections)",
                        Style::default().fg(self.theme.text_secondary),
                    )),
                    Rect {
                        height: 1,
                        ..list_area
                    },
                );
            }
        }

        // --- Predicate editor (right side) ---
        if !sections.is_empty() && sec.selected_section < sections.len() {
            let section = &sections[sec.selected_section];
            let pred_rows = predicate_rows(section);
            let is_pred_focused = sec.focus == SectionsFocus::Predicates;

            for (i, (label, value)) in pred_rows.iter().enumerate() {
                if i as u16 >= pred_area.height {
                    break;
                }
                let y = pred_area.y + i as u16;
                let is_selected = is_pred_focused && i == sec.pred_selected;

                let style = if is_selected {
                    self.theme.selection()
                } else {
                    Style::default()
                };

                let label_w = 18_u16.min(pred_area.width);
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        format!("{:<w$}", label, w = label_w as usize),
                        style,
                    )),
                    Rect {
                        x: pred_area.x,
                        y,
                        width: label_w,
                        height: 1,
                    },
                );

                if pred_area.width > label_w + 1 {
                    let val_x = pred_area.x + label_w + 1;
                    let val_w = pred_area.width.saturating_sub(label_w + 1);

                    let display_val = if is_selected {
                        if let Some(SectionsEditing::EditingPredicate { value: v }) = &sec.editing {
                            super::input_with_caret(v)
                        } else {
                            value.clone()
                        }
                    } else {
                        value.clone()
                    };

                    let val_style = if is_selected && sec.editing.is_some() {
                        style.add_modifier(Modifier::UNDERLINED)
                    } else {
                        style.fg(self.theme.text_accent)
                    };

                    frame.render_widget(
                        Paragraph::new(Span::styled(display_val, val_style)),
                        Rect {
                            x: val_x,
                            y,
                            width: val_w,
                            height: 1,
                        },
                    );
                }
            }
        }

        // --- Footer ---
        let footer_text = match &sec.editing {
            Some(
                SectionsEditing::RenamingSection { .. } | SectionsEditing::EditingPredicate { .. },
            ) => "Enter: save  Esc: cancel",
            Some(SectionsEditing::CreatingSection { .. }) => "Enter: create  Esc: cancel",
            None if sec.focus == SectionsFocus::List => {
                "n: new  r: rename  d: delete  J/K: reorder  →/Enter: predicates  Tab: switch tab"
            }
            None => "Enter: edit  ←: back to list  Tab: switch tab",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                footer_text,
                Style::default().fg(self.theme.text_secondary),
            )),
            footer_area,
        );
    }

    /// Apply an edited value from the settings modal to the config.
    pub(super) fn apply_settings_edit(&mut self, tab: SettingsTab, field_key: &str, value: &str) {
        match tab {
            SettingsTab::General => match field_key {
                "default_program" => self.config.default_program = value.to_string(),
                "branch_prefix" => self.config.branch_prefix = value.to_string(),
                "shell_program" => self.config.shell_program = value.to_string(),
                "worktrees_dir" => {
                    self.config.worktrees_dir = if value.is_empty() || value == "(default)" {
                        None
                    } else {
                        Some(PathBuf::from(value))
                    };
                }
                "editor" => {
                    self.config.editor = if value.is_empty() || value == "(auto)" {
                        None
                    } else {
                        Some(value.to_string())
                    };
                }
                "editor_gui" => {
                    self.config.editor_gui = match value {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    };
                }
                "ui_refresh_fps" => match value.parse::<u32>() {
                    Ok(v) if v >= 1 => {
                        self.config.ui_refresh_fps = v;
                    }
                    Ok(_) => {
                        self.ui_state.status_message = Some((
                            "UI Refresh FPS must be at least 1".into(),
                            std::time::Instant::now() + std::time::Duration::from_secs(4),
                        ));
                    }
                    Err(_) => {}
                },
                "pr_check_interval_secs" => {
                    if let Ok(v) = value.parse::<u64>() {
                        self.config.pr_check_interval_secs = v;
                    }
                }
                "project_pull_interval_secs" => match value.parse::<u64>() {
                    Ok(v) if v >= 60 => {
                        self.config.project_pull_interval_secs = v;
                    }
                    Ok(_) => {
                        self.ui_state.status_message = Some((
                            "Project Pull Interval must be at least 60 seconds".into(),
                            std::time::Instant::now() + std::time::Duration::from_secs(4),
                        ));
                    }
                    Err(_) => {}
                },
                "max_concurrent_tmux" => match value.parse::<usize>() {
                    Ok(v) if v >= 1 => {
                        self.config.max_concurrent_tmux = v;
                    }
                    Ok(_) => {
                        self.ui_state.status_message = Some((
                            "Max Concurrent Tmux must be at least 1".into(),
                            std::time::Instant::now() + std::time::Duration::from_secs(4),
                        ));
                    }
                    Err(_) => {}
                },
                "dim_unfocused_opacity" => {
                    if let Ok(v) = value.parse::<f32>() {
                        self.config.dim_unfocused_opacity = v.clamp(0.0, 1.0);
                    }
                }
                "session_number_debounce_ms" => {
                    if let Ok(v) = value.parse::<u64>() {
                        self.config.session_number_debounce_ms = v;
                    }
                }
                "ai_summary_model" => {
                    self.config.ai_summary_model = value.to_string();
                }
                "commander_enabled" => {
                    if let Ok(b) = value.parse::<bool>() {
                        self.config.commander_enabled = b;
                    }
                }
                "commander_program" => {
                    self.config.commander_program = if value.is_empty() || value == "(default)" {
                        None
                    } else {
                        Some(value.to_string())
                    };
                }
                "commander_dir" => {
                    self.config.commander_dir = if value.is_empty() || value == "(default)" {
                        None
                    } else {
                        Some(PathBuf::from(value))
                    };
                }
                "in_progress_limit" => {
                    self.config.in_progress_limit = if value.is_empty() || value == "(unlimited)" {
                        None
                    } else {
                        value.parse::<u32>().ok().filter(|&n| n > 0)
                    };
                }
                _ => {}
            },
            SettingsTab::Conversation => match field_key {
                "conversation_name" => {
                    let v = value.trim();
                    self.config.conversation.name = if v.is_empty() {
                        "Claudette".to_string()
                    } else {
                        v.to_string()
                    };
                }
                "conversation_base_url" => self.config.conversation.base_url = value.to_string(),
                "conversation_model" => self.config.conversation.model = value.to_string(),
                "conversation_voice" => {
                    self.config.conversation.voice = if value.is_empty() || value == "(default)" {
                        None
                    } else {
                        Some(value.to_string())
                    };
                }
                "conversation_format" => {
                    self.config.conversation.response_format = value.to_string();
                }
                "conversation_speed" => {
                    if let Ok(v) = value.parse::<f32>() {
                        self.config.conversation.speed = v.clamp(0.25, 4.0);
                    }
                }
                "conversation_volume" => {
                    if let Ok(v) = value.parse::<f32>() {
                        self.config.conversation.volume = v.clamp(0.0, 2.0);
                    }
                }
                "conversation_speak_scope" => {
                    // The picker passes the human label; config/tests use tokens.
                    if let Some(scope) = crate::conversation::SpeakScope::from_token(value)
                        .or_else(|| crate::conversation::SpeakScope::from_label(value))
                    {
                        self.config.conversation.speak_scope = scope;
                    }
                }
                "stt_base_url" => self.config.stt.base_url = value.to_string(),
                "stt_model" => self.config.stt.model = value.to_string(),
                "stt_language" => {
                    self.config.stt.language = if value.is_empty() || value == "(auto)" {
                        None
                    } else {
                        Some(value.to_string())
                    };
                }
                "stt_prompt" => {
                    self.config.stt.prompt = if value.is_empty() || value == "(none)" {
                        None
                    } else {
                        Some(value.to_string())
                    };
                }
                _ => {}
            },
            SettingsTab::Theme => {
                use crate::config::theme::ColorValue;

                if field_key == "preset" {
                    self.config.theme.preset = if value.is_empty() || value == "(auto)" {
                        None
                    } else {
                        Some(value.to_string())
                    };
                } else {
                    // Try to parse the value as a ColorValue via TOML
                    let toml_input = if value.starts_with('#')
                        || value.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
                    {
                        format!("c = \"{value}\"")
                    } else {
                        format!("c = {value}")
                    };

                    #[derive(serde::Deserialize)]
                    struct Wrap {
                        c: ColorValue,
                    }

                    if let Ok(w) = toml::from_str::<Wrap>(&toml_input) {
                        macro_rules! set_theme_field {
                            ($($name:ident),*) => {
                                match field_key {
                                    $(stringify!($name) => self.config.theme.$name = Some(w.c),)*
                                    _ => {}
                                }
                            };
                        }
                        set_theme_field!(
                            border_focused,
                            border_unfocused,
                            selection_bg,
                            selection_fg,
                            status_running,
                            status_stopped,
                            status_pr,
                            status_pr_merged,
                            pr_open,
                            pr_draft,
                            pr_closed,
                            text_primary,
                            text_secondary,
                            text_accent,
                            diff_added,
                            diff_removed,
                            diff_hunk_header,
                            diff_file_header,
                            diff_context,
                            modal_info,
                            modal_warning,
                            modal_error,
                            status_bar_bg,
                            status_bar_fg
                        );
                    }
                }

                // Rebuild theme from updated overrides
                let base = self
                    .config
                    .theme
                    .preset
                    .as_deref()
                    .and_then(Theme::from_preset)
                    .unwrap_or_default();
                self.theme = base.with_overrides(&self.config.theme);
            }
            SettingsTab::Keybindings => {
                use crate::config::keybindings::{BindableAction, KeyBinding};
                use std::str::FromStr;

                let Ok(action) = BindableAction::from_str(field_key) else {
                    warn!("Unknown keybinding action: {}", field_key);
                    return;
                };

                // The row value is rendered as a comma-separated list
                // (e.g. `"k, Up, Ctrl-p"`). Parse each entry back into a
                // `KeyBinding`, ignoring empty tokens. If every token fails
                // to parse we leave the binding alone rather than silently
                // clear it.
                let mut parsed: Vec<KeyBinding> = Vec::new();
                let mut had_token = false;
                let mut any_err = false;
                for token in value.split(',') {
                    let t = token.trim();
                    if t.is_empty() {
                        continue;
                    }
                    had_token = true;
                    match KeyBinding::from_str(t) {
                        Ok(kb) => parsed.push(kb),
                        Err(e) => {
                            warn!("Invalid keybinding '{}': {}", t, e);
                            any_err = true;
                        }
                    }
                }

                if had_token && parsed.is_empty() && any_err {
                    // User tried to edit but every token was malformed —
                    // show the error but don't wipe their existing binding.
                    self.ui_state.modal = Modal::Error {
                        message: format!(
                            "Could not parse any key bindings from '{}'. \
                             Use e.g. 'k', 'Ctrl-p', 'Shift-N', 'Enter'.",
                            value
                        ),
                    };
                    return;
                }

                self.config.keybindings.set_keys_for(action, parsed);
            }
            SettingsTab::Sections => {
                // Sections tab handles its own persistence via save_sections_config
                return;
            }
        }

        self.persist_config();
    }

    /// Set a boolean General-tab setting to a typed value and persist.
    ///
    /// Booleans are stored as real `bool`s on [`Config`]; this avoids the
    /// stringify/parse round-trip used by the text-input edit path.
    pub(super) fn apply_bool_setting(&mut self, field_key: &str, value: bool) {
        match field_key {
            "per_repo_worktree_dirs" => self.config.per_repo_worktree_dirs = value,
            "fetch_before_create" => self.config.fetch_before_create = value,
            "skip_lfs_smudge" => self.config.skip_lfs_smudge = value,
            "resume_session" => self.config.resume_session = value,
            "nix_develop" => self.config.nix_develop = value,
            "project_pull_enabled" => self.config.project_pull_enabled = value,
            "dim_unfocused_preview" => self.config.dim_unfocused_preview = value,
            "invert_pr_label_color" => self.config.invert_pr_label_color = value,
            "show_session_program" => self.config.show_session_program = value,
            "rounded_borders" => self.config.rounded_borders = value,
            "precompute_review_caches" => self.config.precompute_review_caches = value,
            "ai_summary_enabled" => self.config.ai_summary_enabled = value,
            "commander_enabled" => self.config.commander_enabled = value,
            "conversation_enabled" => self.config.conversation.enabled = value,
            "stt_enabled" => self.config.stt.enabled = value,
            "stt_pause_media" => self.config.stt.pause_media = value,
            "telemetry_enabled" => self.config.telemetry.enabled = value,
            _ => {
                warn!("Unknown boolean setting: {}", field_key);
                return;
            }
        }
        self.persist_config();
    }

    /// Persist the current config via the store (updates mtime so hot-reload
    /// won't re-read our own write).
    fn persist_config(&mut self) {
        let updated = self.config.clone();
        if let Err(e) = self.service.update_config(updated) {
            warn!("Failed to save config: {}", e);
        }
    }

    /// Handle a keypress in the settings modal.
    pub(super) async fn handle_settings_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        mut state: SettingsState,
    ) {
        use crossterm::event::KeyCode;

        if state.tab == SettingsTab::Sections {
            self.handle_sections_key(key, state).await;
            return;
        }

        // Keybindings-tab search: while the filter box is focused, typing
        // narrows the list live and the arrows navigate the matches.
        if state.search.is_some() {
            self.handle_keybinding_search_key(key, state);
            return;
        }

        // `/` opens the search box on the Keybindings tab.
        if state.tab == SettingsTab::Keybindings
            && state.editing.is_none()
            && key.code == KeyCode::Char('/')
        {
            state.search = Some(Input::default());
            state.selected_row = first_selectable_from(&state.rows, 0);
            self.ui_state.modal = Modal::Settings(state);
            return;
        }

        if let Some(ref mut editing) = state.editing {
            // Currently editing a field
            match editing {
                SettingsEditing::TextInput { value } => match key.code {
                    KeyCode::Enter => {
                        let val = value.value().to_string();
                        let field_key = state.rows[state.selected_row].field_key.clone();
                        state.editing = None;
                        self.apply_settings_edit(state.tab, &field_key, &val);
                        // Refresh rows after applying
                        state.rows = self.build_settings_rows(state.tab);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Esc => {
                        state.editing = None;
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    _ => {
                        super::edit_text_input(value, key);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                },
                SettingsEditing::KeyCapture { .. } => {
                    // For key capture, any keypress except Esc is captured as the new binding
                    match key.code {
                        KeyCode::Esc => {
                            state.editing = None;
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        _ => {
                            // Key capture is a simplified version — store the key display
                            // Full keybinding editing would require more complex UX
                            state.editing = None;
                            self.ui_state.modal = Modal::Settings(state);
                        }
                    }
                }
                SettingsEditing::OptionPicker { options, selected } => match key.code {
                    KeyCode::Enter => {
                        let chosen = options[*selected].clone();
                        let field_key = state.rows[state.selected_row].field_key.clone();
                        // Treat "(auto)" as empty string for apply_settings_edit
                        let val = if chosen == "(auto)" {
                            String::new()
                        } else {
                            chosen
                        };
                        state.editing = None;
                        self.apply_settings_edit(state.tab, &field_key, &val);
                        state.rows = self.build_settings_rows(state.tab);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Esc => {
                        state.editing = None;
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        *selected = (*selected + 1) % options.len();
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        *selected = if *selected == 0 {
                            options.len() - 1
                        } else {
                            *selected - 1
                        };
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    _ => {
                        self.ui_state.modal = Modal::Settings(state);
                    }
                },
            }
        } else {
            // Not editing — navigation mode: resolve via configurable keybindings
            use crate::config::keybindings::BindableAction;

            // Boolean rows flip in place on Enter/Space/Left/Right without
            // opening an editor.
            let toggle_key = matches!(
                key.code,
                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
            );
            let new_val = toggle_key
                .then(|| {
                    state
                        .rows
                        .get(state.selected_row)
                        .and_then(SettingsRow::toggled)
                })
                .flatten();
            if let Some(new_val) = new_val {
                let field_key = state.rows[state.selected_row].field_key.clone();
                self.apply_bool_setting(&field_key, new_val);
                state.rows = self.build_settings_rows(state.tab);
                self.ui_state.modal = Modal::Settings(state);
                return;
            }

            match self.config.keybindings.resolve(&key) {
                Some(BindableAction::NavigateDown) => {
                    state.selected_row = step_selectable(&state.rows, state.selected_row, true);
                    self.ui_state.modal = Modal::Settings(state);
                }
                Some(BindableAction::NavigateUp) => {
                    state.selected_row = step_selectable(&state.rows, state.selected_row, false);
                    self.ui_state.modal = Modal::Settings(state);
                }
                Some(BindableAction::Quit) => {
                    self.ui_state.modal = Modal::None;
                }
                _ => match key.code {
                    KeyCode::Esc => {
                        self.ui_state.modal = Modal::None;
                    }
                    KeyCode::Tab => {
                        state.tab = state.tab.next();
                        state.rows = self.build_settings_rows(state.tab);
                        state.selected_row = first_selectable_from(&state.rows, 0);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::BackTab => {
                        state.tab = state.tab.prev();
                        state.rows = self.build_settings_rows(state.tab);
                        state.selected_row = first_selectable_from(&state.rows, 0);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Enter => {
                        if !state.rows.is_empty() {
                            let field_key = &state.rows[state.selected_row].field_key;
                            if state.tab == SettingsTab::Theme && field_key == "preset" {
                                // Open an inline option picker for theme presets
                                use crate::tui::theme::PRESET_NAMES;
                                let options: Vec<String> =
                                    PRESET_NAMES.iter().map(|s| (*s).to_string()).collect();
                                let current_value = state.rows[state.selected_row].text_value();
                                let selected =
                                    options.iter().position(|o| o == current_value).unwrap_or(0);
                                state.editing =
                                    Some(SettingsEditing::OptionPicker { options, selected });
                            } else if field_key == "conversation_speak_scope" {
                                // Inline option picker for the speak-scope enum.
                                use crate::conversation::SpeakScope;
                                let options: Vec<String> = SpeakScope::ALL
                                    .iter()
                                    .map(|s| s.label().to_string())
                                    .collect();
                                let current_value = state.rows[state.selected_row].text_value();
                                let selected =
                                    options.iter().position(|o| o == current_value).unwrap_or(0);
                                state.editing =
                                    Some(SettingsEditing::OptionPicker { options, selected });
                            } else {
                                let current_value = state.rows[state.selected_row].text_value();
                                let initial =
                                    if current_value == "(auto)" || current_value == "(none)" {
                                        String::new()
                                    } else {
                                        current_value.to_string()
                                    };
                                state.editing = Some(SettingsEditing::TextInput {
                                    value: initial.into(),
                                });
                            }
                        }
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    _ => {
                        self.ui_state.modal = Modal::Settings(state);
                    }
                },
            }
        }
    }

    /// Handle a keypress while the Keybindings search box is focused. Typing
    /// filters the shortcut list live; arrows navigate the matches; `Enter`
    /// keeps the filter and returns to browsing; `Esc` clears it.
    fn handle_keybinding_search_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        mut state: SettingsState,
    ) {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Esc => {
                // Clear the filter and restore the full grouped list.
                state.search = None;
                state.rows = self.build_settings_rows(state.tab);
                state.selected_row = first_selectable_from(&state.rows, 0);
            }
            KeyCode::Enter => {
                // Keep the filtered view; drop back to list navigation.
                state.search = None;
                state.selected_row = first_selectable_from(&state.rows, state.selected_row);
            }
            KeyCode::Down => {
                state.selected_row = step_selectable(&state.rows, state.selected_row, true);
            }
            KeyCode::Up => {
                state.selected_row = step_selectable(&state.rows, state.selected_row, false);
            }
            _ => {
                if let Some(input) = state.search.as_mut()
                    && super::edit_text_input(input, key)
                {
                    // Query changed: re-filter from the full grouped list.
                    let query = input.value().to_string();
                    let full = self.build_settings_rows(state.tab);
                    state.rows = filter_keybinding_rows(full, &query);
                    state.selected_row = first_selectable_from(&state.rows, 0);
                }
            }
        }
        self.ui_state.modal = Modal::Settings(state);
    }

    /// Handle a keypress while the Sections tab is active.
    async fn handle_sections_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        mut state: SettingsState,
    ) {
        use crossterm::event::KeyCode;

        let sec = &mut state.sections_state;

        // --- Editing mode ---
        if let Some(ref mut editing) = sec.editing {
            match editing {
                SectionsEditing::RenamingSection { value } => match key.code {
                    KeyCode::Enter => {
                        let new_name = value.value().trim().to_string();
                        if !new_name.is_empty() && sec.selected_section < self.config.sections.len()
                        {
                            let has_dup = self
                                .config
                                .sections
                                .iter()
                                .enumerate()
                                .any(|(i, s)| i != sec.selected_section && s.name == new_name);
                            if !has_dup {
                                self.config.sections[sec.selected_section].name = new_name;
                                self.save_sections_config().await;
                            }
                        }
                        sec.editing = None;
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Esc => {
                        sec.editing = None;
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    _ => {
                        super::edit_text_input(value, key);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                },
                SectionsEditing::EditingPredicate { value } => match key.code {
                    KeyCode::Enter => {
                        let val = value.value().to_string();
                        let pred_idx = sec.pred_selected;
                        if sec.selected_section < self.config.sections.len() {
                            apply_predicate_edit(
                                &mut self.config.sections[sec.selected_section],
                                pred_idx,
                                &val,
                            );
                            self.save_sections_config().await;
                        }
                        sec.editing = None;
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Esc => {
                        sec.editing = None;
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    _ => {
                        super::edit_text_input(value, key);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                },
                SectionsEditing::CreatingSection { value } => match key.code {
                    KeyCode::Enter => {
                        let new_name = value.value().trim().to_string();
                        if !new_name.is_empty() {
                            let has_dup = self.config.sections.iter().any(|s| s.name == new_name);
                            if !has_dup {
                                self.config.sections.push(crate::session::SectionConfig {
                                    name: new_name,
                                    ..Default::default()
                                });
                                sec.selected_section = self.config.sections.len() - 1;
                                self.save_sections_config().await;
                            }
                        }
                        sec.editing = None;
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Esc => {
                        sec.editing = None;
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    _ => {
                        super::edit_text_input(value, key);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                },
            }
            return;
        }

        // --- Navigation mode ---
        let sections_len = self.config.sections.len();

        match sec.focus {
            SectionsFocus::List => {
                use crate::config::keybindings::BindableAction;

                match self.config.keybindings.resolve(&key) {
                    Some(BindableAction::NavigateDown) => {
                        if sections_len > 0 {
                            sec.selected_section = (sec.selected_section + 1) % sections_len;
                            sec.pred_selected = 0;
                        }
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    Some(BindableAction::NavigateUp) => {
                        if sections_len > 0 {
                            sec.selected_section = if sec.selected_section == 0 {
                                sections_len - 1
                            } else {
                                sec.selected_section - 1
                            };
                            sec.pred_selected = 0;
                        }
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    Some(BindableAction::Quit) => {
                        self.ui_state.modal = Modal::None;
                    }
                    _ => match key.code {
                        KeyCode::Esc => {
                            self.ui_state.modal = Modal::None;
                        }
                        KeyCode::Tab => {
                            state.tab = state.tab.next();
                            state.rows = self.build_settings_rows(state.tab);
                            state.selected_row = first_selectable_from(&state.rows, 0);
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::BackTab => {
                            state.tab = state.tab.prev();
                            state.rows = self.build_settings_rows(state.tab);
                            state.selected_row = first_selectable_from(&state.rows, 0);
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::Right | KeyCode::Enter => {
                            if sections_len > 0 {
                                sec.focus = SectionsFocus::Predicates;
                                sec.pred_selected = 0;
                            }
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::Char('n') => {
                            sec.editing = Some(SectionsEditing::CreatingSection {
                                value: super::Input::default(),
                            });
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::Char('r') => {
                            if sec.selected_section < sections_len {
                                let current =
                                    self.config.sections[sec.selected_section].name.clone();
                                sec.editing = Some(SectionsEditing::RenamingSection {
                                    value: current.into(),
                                });
                            }
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::Char('d') => {
                            if sec.selected_section < sections_len {
                                self.config.sections.remove(sec.selected_section);
                                if sec.selected_section >= self.config.sections.len()
                                    && sec.selected_section > 0
                                {
                                    sec.selected_section -= 1;
                                }
                                self.save_sections_config().await;
                            }
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::Char('J') => {
                            if sec.selected_section + 1 < sections_len {
                                self.config
                                    .sections
                                    .swap(sec.selected_section, sec.selected_section + 1);
                                sec.selected_section += 1;
                                self.save_sections_config().await;
                            }
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::Char('K') => {
                            if sec.selected_section > 0 && sections_len > 0 {
                                self.config
                                    .sections
                                    .swap(sec.selected_section, sec.selected_section - 1);
                                sec.selected_section -= 1;
                                self.save_sections_config().await;
                            }
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        _ => {
                            self.ui_state.modal = Modal::Settings(state);
                        }
                    },
                }
            }
            SectionsFocus::Predicates => {
                use crate::config::keybindings::BindableAction;

                let pred_count = if sec.selected_section < sections_len {
                    predicate_rows(&self.config.sections[sec.selected_section]).len()
                } else {
                    0
                };

                match self.config.keybindings.resolve(&key) {
                    Some(BindableAction::NavigateDown) => {
                        if pred_count > 0 {
                            sec.pred_selected = (sec.pred_selected + 1) % pred_count;
                        }
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    Some(BindableAction::NavigateUp) => {
                        if pred_count > 0 {
                            sec.pred_selected = if sec.pred_selected == 0 {
                                pred_count - 1
                            } else {
                                sec.pred_selected - 1
                            };
                        }
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    Some(BindableAction::Quit) => {
                        self.ui_state.modal = Modal::None;
                    }
                    _ => match key.code {
                        KeyCode::Esc | KeyCode::Left => {
                            sec.focus = SectionsFocus::List;
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::Tab => {
                            state.tab = state.tab.next();
                            state.rows = self.build_settings_rows(state.tab);
                            state.selected_row = first_selectable_from(&state.rows, 0);
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::BackTab => {
                            state.tab = state.tab.prev();
                            state.rows = self.build_settings_rows(state.tab);
                            state.selected_row = first_selectable_from(&state.rows, 0);
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        KeyCode::Enter => {
                            if sec.selected_section < sections_len && pred_count > 0 {
                                let rows =
                                    predicate_rows(&self.config.sections[sec.selected_section]);
                                let (_, current_val) = &rows[sec.pred_selected];
                                let initial = if current_val == "(not set)" {
                                    String::new()
                                } else {
                                    current_val.clone()
                                };
                                sec.editing = Some(SectionsEditing::EditingPredicate {
                                    value: initial.into(),
                                });
                            }
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        _ => {
                            self.ui_state.modal = Modal::Settings(state);
                        }
                    },
                }
            }
        }
    }

    /// Persist the current sections config to disk and reconcile session assignments.
    async fn save_sections_config(&mut self) {
        let updated = self.config.clone();
        if let Err(e) = self.service.update_config(updated) {
            warn!("Failed to save sections config: {}", e);
        }
        self.reconcile_section_assignments().await;
    }
}

/// Build displayable rows for a section's predicates.
fn predicate_rows(section: &crate::session::SectionConfig) -> Vec<(String, String)> {
    use crate::session::section::{
        DecisionPredicate, LabelPredicate, ReviewerPredicate, StatePredicate,
    };

    let fmt_state = |p: &StatePredicate| match p {
        crate::session::section::OneOrMany::One(v) => format!("{v:?}").to_lowercase(),
        crate::session::section::OneOrMany::Any(vs) => vs
            .iter()
            .map(|v| format!("{v:?}").to_lowercase())
            .collect::<Vec<_>>()
            .join(", "),
    };

    let fmt_decision = |p: &DecisionPredicate| match p {
        crate::session::section::OneOrMany::One(v) => format!("{v:?}").to_lowercase(),
        crate::session::section::OneOrMany::Any(vs) => vs
            .iter()
            .map(|v| format!("{v:?}").to_lowercase())
            .collect::<Vec<_>>()
            .join(", "),
    };

    let fmt_label = |p: &LabelPredicate| match p {
        LabelPredicate::One(s) => s.clone(),
        LabelPredicate::Any(vs) => vs.join(", "),
    };

    let fmt_reviewer = |p: &ReviewerPredicate| match p {
        ReviewerPredicate::Bool(b) => b.to_string(),
        ReviewerPredicate::One(s) => s.clone(),
        ReviewerPredicate::Any(vs) => vs.join(", "),
    };

    let not_set = "(not set)".to_string();
    vec![
        (
            "pr_state".into(),
            section
                .pr_state
                .as_ref()
                .map_or_else(|| not_set.clone(), fmt_state),
        ),
        (
            "is_draft".into(),
            section
                .is_draft
                .map_or_else(|| not_set.clone(), |b| b.to_string()),
        ),
        (
            "has_label".into(),
            section
                .has_label
                .as_ref()
                .map_or_else(|| not_set.clone(), fmt_label),
        ),
        (
            "has_pr".into(),
            section
                .has_pr
                .map_or_else(|| not_set.clone(), |b| b.to_string()),
        ),
        (
            "review_decision".into(),
            section
                .review_decision
                .as_ref()
                .map_or_else(|| not_set.clone(), fmt_decision),
        ),
        (
            "has_reviewer".into(),
            section
                .has_reviewer
                .as_ref()
                .map_or_else(|| not_set.clone(), fmt_reviewer),
        ),
        (
            "max_sessions".into(),
            section
                .max_sessions
                .map_or_else(|| not_set.clone(), |n| n.to_string()),
        ),
    ]
}

/// Apply a user-edited predicate value string to a SectionConfig field.
fn apply_predicate_edit(section: &mut crate::session::SectionConfig, pred_idx: usize, value: &str) {
    use crate::git::{PrState, ReviewDecision};
    use crate::session::section::{LabelPredicate, OneOrMany, ReviewerPredicate};

    let trimmed = value.trim();

    match pred_idx {
        // pr_state
        0 => {
            if trimmed.is_empty() {
                section.pr_state = None;
            } else {
                let parts: Vec<&str> = trimmed.split(',').map(str::trim).collect();
                let parsed: Vec<PrState> = parts.iter().filter_map(|s| parse_pr_state(s)).collect();
                section.pr_state = match parsed.len() {
                    0 => None,
                    1 => Some(OneOrMany::One(parsed[0])),
                    _ => Some(OneOrMany::Any(parsed)),
                };
            }
        }
        // is_draft
        1 => {
            section.is_draft = match trimmed {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        }
        // has_label
        2 => {
            if trimmed.is_empty() {
                section.has_label = None;
            } else {
                let labels: Vec<String> = trimmed
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                section.has_label = match labels.len() {
                    0 => None,
                    1 => Some(LabelPredicate::One(labels.into_iter().next().unwrap())),
                    _ => Some(LabelPredicate::Any(labels)),
                };
            }
        }
        // has_pr
        3 => {
            section.has_pr = match trimmed {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        }
        // review_decision
        4 => {
            if trimmed.is_empty() {
                section.review_decision = None;
            } else {
                let parts: Vec<&str> = trimmed.split(',').map(str::trim).collect();
                let parsed: Vec<ReviewDecision> = parts
                    .iter()
                    .filter_map(|s| parse_review_decision(s))
                    .collect();
                section.review_decision = match parsed.len() {
                    0 => None,
                    1 => Some(OneOrMany::One(parsed[0])),
                    _ => Some(OneOrMany::Any(parsed)),
                };
            }
        }
        // has_reviewer
        5 => {
            if trimmed.is_empty() {
                section.has_reviewer = None;
            } else {
                match trimmed {
                    "true" => section.has_reviewer = Some(ReviewerPredicate::Bool(true)),
                    "false" => section.has_reviewer = Some(ReviewerPredicate::Bool(false)),
                    _ => {
                        let logins: Vec<String> = trimmed
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        section.has_reviewer = match logins.len() {
                            0 => None,
                            1 => Some(ReviewerPredicate::One(logins.into_iter().next().unwrap())),
                            _ => Some(ReviewerPredicate::Any(logins)),
                        };
                    }
                }
            }
        }
        // max_sessions
        6 => {
            section.max_sessions = if trimmed.is_empty() {
                None
            } else {
                trimmed.parse::<u32>().ok().filter(|&n| n > 0)
            };
        }
        _ => {}
    }
}

fn parse_pr_state(s: &str) -> Option<crate::git::PrState> {
    match s.to_lowercase().as_str() {
        "open" => Some(crate::git::PrState::Open),
        "merged" => Some(crate::git::PrState::Merged),
        "closed" => Some(crate::git::PrState::Closed),
        _ => None,
    }
}

fn parse_review_decision(s: &str) -> Option<crate::git::ReviewDecision> {
    match s.to_lowercase().replace(['-', '_'], "").as_str() {
        "approved" => Some(crate::git::ReviewDecision::Approved),
        "changesrequested" => Some(crate::git::ReviewDecision::ChangesRequested),
        "reviewrequired" => Some(crate::git::ReviewDecision::ReviewRequired),
        _ => None,
    }
}

/// Width of the label column for the settings rows.
///
/// Sized to fit the longest label (plus a trailing space) so descriptions are
/// not clipped — the Keybindings tab has labels well over 24 columns — while
/// keeping at least a usable minimum value column on narrow terminals and a
/// sensible floor so short-labelled tabs still align tidily.
fn settings_label_width(rows: &[SettingsRow], area_width: u16) -> u16 {
    const MIN_LABEL: u16 = 24;
    const MIN_VALUE: u16 = 16;
    // 2-column gap between the label and value columns (see `val_x`).
    const GAP: u16 = 2;

    let longest = rows
        .iter()
        // Section headers span the full width, so they don't drive the label
        // column; only value-bearing rows do.
        .filter(|r| r.is_selectable())
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let desired = longest.saturating_add(1).max(MIN_LABEL);

    // Cap so the value column keeps at least MIN_VALUE columns, but never force
    // the label below its floor (on a very narrow terminal labels truncate).
    let cap = area_width.saturating_sub(MIN_VALUE + GAP).max(MIN_LABEL);
    desired.min(cap)
}

/// Index of the first selectable row at or after `from`, wrapping to the start
/// if none is found below. Returns `from` when there are no selectable rows.
pub(super) fn first_selectable_from(rows: &[SettingsRow], from: usize) -> usize {
    rows.iter()
        .enumerate()
        .skip(from)
        .find(|(_, r)| r.is_selectable())
        .or_else(|| rows.iter().enumerate().find(|(_, r)| r.is_selectable()))
        .map(|(i, _)| i)
        .unwrap_or(from)
}

/// Next selectable row from `current` in the given direction, wrapping around
/// and skipping non-selectable header rows. Returns `current` when there is no
/// other selectable row.
fn step_selectable(rows: &[SettingsRow], current: usize, forward: bool) -> usize {
    let n = rows.len();
    if n == 0 {
        return current;
    }
    let mut idx = current;
    for _ in 0..n {
        idx = if forward {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        if rows[idx].is_selectable() {
            return idx;
        }
    }
    current
}

/// A blank spacer row (an empty-label header) drawn between sections.
fn is_spacer(row: &SettingsRow) -> bool {
    matches!(row.kind, SettingsRowKind::Header) && row.label.is_empty()
}

/// Insert a blank spacer row before every section header except the first, so
/// the grouped list breathes. Expects a spacer-free list of headers + rows.
fn with_section_spacers(rows: Vec<SettingsRow>) -> Vec<SettingsRow> {
    let mut out: Vec<SettingsRow> = Vec::with_capacity(rows.len() + 8);
    for row in rows {
        if matches!(row.kind, SettingsRowKind::Header) && !out.is_empty() {
            out.push(SettingsRow::header(""));
        }
        out.push(row);
    }
    out
}

/// Filter grouped keybinding rows to those matching `query` (case-insensitive
/// fuzzy match against the description and the bound keys). Section headers are
/// kept only when at least one of their actions survives the filter, and blank
/// spacers are re-inserted between the surviving sections. An empty query
/// returns every binding (still grouped and spaced).
pub(super) fn filter_keybinding_rows(rows: Vec<SettingsRow>, query: &str) -> Vec<SettingsRow> {
    let query = query.trim();
    // Work on the spacer-free list so the grouping logic stays simple; spacers
    // are re-added at the end based on the surviving headers.
    let rows: Vec<SettingsRow> = rows.into_iter().filter(|r| !is_spacer(r)).collect();
    if query.is_empty() {
        return with_section_spacers(rows);
    }
    let matches = |row: &SettingsRow| {
        crate::fuzzy::fuzzy_score(&row.label, query).is_some()
            || crate::fuzzy::fuzzy_score(row.text_value(), query).is_some()
    };

    let mut out: Vec<SettingsRow> = Vec::new();
    for row in rows {
        if row.is_selectable() {
            if matches(&row) {
                out.push(row);
            }
        } else {
            // Drop the previous header if it ended up with no matching children.
            if out.last().is_some_and(|r| !r.is_selectable()) {
                out.pop();
            }
            out.push(row);
        }
    }
    // A trailing header with no children (last section didn't match) is dropped.
    if out.last().is_some_and(|r| !r.is_selectable()) {
        out.pop();
    }
    with_section_spacers(out)
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max > 1 {
        let kept: String = s.chars().take(max - 1).collect();
        format!("{kept}…")
    } else {
        "…".to_string()
    }
}

/// Format a ratatui Color for display in the settings modal.
fn format_color(color: ratatui::style::Color) -> String {
    use ratatui::style::Color;
    match color {
        Color::Reset => "reset".into(),
        Color::Black => "black".into(),
        Color::Red => "red".into(),
        Color::Green => "green".into(),
        Color::Yellow => "yellow".into(),
        Color::Blue => "blue".into(),
        Color::Magenta => "magenta".into(),
        Color::Cyan => "cyan".into(),
        Color::Gray => "gray".into(),
        Color::DarkGray => "dark_gray".into(),
        Color::LightRed => "light_red".into(),
        Color::LightGreen => "light_green".into(),
        Color::LightYellow => "light_yellow".into(),
        Color::LightBlue => "light_blue".into(),
        Color::LightMagenta => "light_magenta".into(),
        Color::LightCyan => "light_cyan".into(),
        Color::White => "white".into(),
        Color::Indexed(i) => format!("{i}"),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_with_labels(labels: &[&str]) -> Vec<SettingsRow> {
        labels
            .iter()
            .map(|l| SettingsRow::text(*l, "", "key"))
            .collect()
    }

    #[test]
    fn label_width_grows_to_fit_long_labels() {
        // A long keybinding-style description must not be clipped to the old
        // fixed 24-column label width.
        let rows = rows_with_labels(&["Cascade merge main through stack"]); // 32 chars
        let w = settings_label_width(&rows, 120);
        assert_eq!(
            w, 33,
            "label column should fit the longest label plus a space"
        );
    }

    #[test]
    fn label_width_has_a_floor_for_short_labels() {
        let rows = rows_with_labels(&["Quit", "Editor"]);
        assert_eq!(settings_label_width(&rows, 120), 24);
    }

    #[test]
    fn label_width_caps_to_keep_value_column_usable() {
        // On a narrow terminal a very long label must not starve the value column.
        let rows = rows_with_labels(&["A really really really long label here"]);
        let w = settings_label_width(&rows, 50);
        assert_eq!(w, 50 - (16 + 2), "value column keeps its minimum width");
    }

    #[test]
    fn label_width_never_below_floor_even_when_very_narrow() {
        let rows = rows_with_labels(&["Some long label that won't fit"]);
        // Cap would be negative; floor wins and labels simply truncate.
        assert_eq!(settings_label_width(&rows, 10), 24);
    }

    #[test]
    fn truncate_str_truncates_multibyte_names_without_panicking() {
        // Regression: byte-index slicing panicked on non-ASCII section names
        // ("byte index is not a char boundary") when the Sections tab rendered.
        assert_eq!(truncate_str("ありがとうございます", 6), "ありがとう…");
        assert_eq!(truncate_str("ありがとう", 5), "ありがとう");
    }

    #[test]
    fn truncate_str_ascii_behaviour() {
        assert_eq!(truncate_str("short", 10), "short");
        assert_eq!(truncate_str("longer-name", 7), "longer…");
        assert_eq!(truncate_str("ab", 1), "…");
    }

    fn keybinding_rows() -> Vec<SettingsRow> {
        vec![
            SettingsRow::header("Navigation"),
            SettingsRow::text("Navigate up", "k, Up", "navigate_up"),
            SettingsRow::text("Navigate down", "j, Down", "navigate_down"),
            SettingsRow::header("Sessions"),
            SettingsRow::text("Delete/kill session", "d", "delete_session"),
        ]
    }

    #[test]
    fn first_selectable_skips_leading_header() {
        let rows = keybinding_rows();
        // Row 0 is a header; the first selectable row is index 1.
        assert_eq!(first_selectable_from(&rows, 0), 1);
        // From a header index, the next selectable is found forward.
        assert_eq!(first_selectable_from(&rows, 3), 4);
    }

    #[test]
    fn step_selectable_skips_headers_and_wraps() {
        let rows = keybinding_rows();
        // From "Navigate down" (2), forward skips the "Sessions" header (3).
        assert_eq!(step_selectable(&rows, 2, true), 4);
        // Forward from the last row wraps past the leading header to row 1.
        assert_eq!(step_selectable(&rows, 4, true), 1);
        // Backward from row 1 wraps to the last selectable row, skipping headers.
        assert_eq!(step_selectable(&rows, 1, false), 4);
    }

    #[test]
    fn filter_matches_description_and_keys() {
        // Matches on the description text.
        let out = filter_keybinding_rows(keybinding_rows(), "delete");
        assert_eq!(out.len(), 2); // "Sessions" header + the delete row
        assert!(matches!(out[0].kind, SettingsRowKind::Header));
        assert_eq!(out[0].label, "Sessions");
        assert_eq!(out[1].field_key, "delete_session");

        // Matches on the bound keys ("Up" belongs to navigate_up).
        let out = filter_keybinding_rows(keybinding_rows(), "Up");
        assert_eq!(out[0].label, "Navigation");
        assert!(out.iter().any(|r| r.field_key == "navigate_up"));
    }

    #[test]
    fn filter_drops_empty_section_headers() {
        // A query that hits nothing yields no rows (no orphan headers).
        let out = filter_keybinding_rows(keybinding_rows(), "zzzznomatch");
        assert!(out.is_empty());

        // A query hitting only Navigation drops the Sessions header entirely.
        let out = filter_keybinding_rows(keybinding_rows(), "navigate");
        assert!(out.iter().all(|r| r.label != "Sessions"));
        assert_eq!(out[0].label, "Navigation");
    }

    #[test]
    fn filter_empty_query_returns_all_rows() {
        // An empty query keeps every binding; only presentational spacers are
        // added between sections.
        let rows = keybinding_rows();
        let selectable = rows.iter().filter(|r| r.is_selectable()).count();
        let out = filter_keybinding_rows(rows, "   ");
        assert_eq!(out.iter().filter(|r| r.is_selectable()).count(), selectable);
        // Two sections → exactly one blank spacer between them.
        assert_eq!(out.iter().filter(|r| is_spacer(r)).count(), 1);
    }

    #[test]
    fn spacers_separate_sections_but_not_the_first() {
        let rows = with_section_spacers(keybinding_rows());
        // No leading spacer.
        assert!(!is_spacer(&rows[0]));
        assert_eq!(rows[0].label, "Navigation");
        // Exactly one spacer, and it sits immediately before the second header.
        let spacer_positions: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| is_spacer(r))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(spacer_positions.len(), 1);
        let s = spacer_positions[0];
        assert!(matches!(rows[s + 1].kind, SettingsRowKind::Header));
        assert_eq!(rows[s + 1].label, "Sessions");
    }

    #[test]
    fn settings_tab_cycle_includes_conversation() {
        assert_eq!(SettingsTab::ALL.len(), 5);
        assert!(SettingsTab::ALL.contains(&SettingsTab::Conversation));
        assert_eq!(SettingsTab::General.next(), SettingsTab::Conversation);
        assert_eq!(SettingsTab::Conversation.prev(), SettingsTab::General);
        // A full forward cycle returns to the start.
        let mut t = SettingsTab::General;
        for _ in 0..SettingsTab::ALL.len() {
            t = t.next();
        }
        assert_eq!(t, SettingsTab::General);
        assert_eq!(SettingsTab::Conversation.label(), "Conversation");
    }
}
