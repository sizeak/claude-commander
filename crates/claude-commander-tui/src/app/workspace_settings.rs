//! The settings modal's **Workspaces** tab, modelled on the Sections tab: a
//! left list of workspaces (`n` new, `r` rename, `d` delete, `J`/`K` reorder)
//! and a right pane with the selected workspace's accent colour and its
//! projects (`m` moves one). A header row shows the startup workspace (`s`
//! cycles it).
//!
//! The list is the *merged* one across every backend, read from the snapshots
//! each frame — so the tab holds only cursor/editing state, and every edit goes
//! through the eager-propagation operations in [`super::workspaces`].

use super::colour_picker::{ColourPicker, ColourPickerFocus, PickerOutcome};
use super::settings::{list_scroll_offset, settings_areas, truncate_str};
use super::workspaces::workspace_color;
use super::*;
use claude_commander_protocol::workspace::StartupWorkspace;
use claude_commander_viewmodel::workspace::MergedWorkspace;

/// One project row in the right pane.
struct WorkspaceProjectRow {
    id: ProjectId,
    label: String,
}

/// The Workspaces tab's panes within the settings body: the workspace list,
/// the divider, and the detail pane (colour, projects, the colour picker).
pub(super) struct WorkspacesPanes {
    pub list: Rect,
    pub divider: Rect,
    pub detail: Rect,
}

pub(super) fn workspaces_panes(body_area: Rect) -> WorkspacesPanes {
    // Below the startup header row and a blank line.
    let panes = Rect {
        y: body_area.y + 2,
        height: body_area.height.saturating_sub(2),
        ..body_area
    };
    let list_width = panes.width.clamp(16, 28);
    WorkspacesPanes {
        list: Rect {
            width: list_width,
            ..panes
        },
        divider: Rect {
            x: panes.x + list_width,
            width: 1,
            ..panes
        },
        detail: Rect {
            x: panes.x + list_width + 2,
            width: panes.width.saturating_sub(list_width + 2),
            ..panes
        },
    }
}

/// How wide the colour picker's pane is when the settings modal is drawn in
/// a frame `area` — what [`ColourPicker::fit_to_width`] is given each frame.
pub(super) fn colour_picker_width(area: Rect) -> u16 {
    workspaces_panes(settings_areas(area).body).detail.width
}

/// How the Startup row reads.
pub(super) fn startup_label(startup: &StartupWorkspace, merged: &[MergedWorkspace]) -> String {
    match startup {
        StartupWorkspace::Last => "Last used".to_string(),
        StartupWorkspace::Main => merged
            .iter()
            .find(|w| w.is_main())
            .map_or_else(|| "Main".to_string(), |m| m.label.clone()),
        StartupWorkspace::Named(name) => name.clone(),
    }
}

/// The startup option after `current`: last → Main → each named workspace →
/// last. A pinned name no longer in the list restarts the cycle.
pub(super) fn next_startup(
    current: &StartupWorkspace,
    merged: &[MergedWorkspace],
) -> StartupWorkspace {
    let mut options = vec![StartupWorkspace::Last, StartupWorkspace::Main];
    options.extend(
        merged
            .iter()
            .filter_map(|w| w.name.clone())
            .map(StartupWorkspace::Named),
    );
    let idx = options.iter().position(|o| o == current).unwrap_or(0);
    options[(idx + 1) % options.len()].clone()
}

impl App {
    /// Projects in workspace `name` across every backend, name-sorted, each
    /// labelled with its server when more than one backend is configured.
    fn workspace_project_rows(&self, name: Option<&str>) -> Vec<WorkspaceProjectRow> {
        let multi = self.backends.len() > 1;
        let mut rows: Vec<WorkspaceProjectRow> = self
            .backends
            .iter()
            .flat_map(|h| {
                h.view
                    .snapshot
                    .projects
                    .iter()
                    .filter(move |p| p.workspace.as_deref() == name)
                    .map(move |p| WorkspaceProjectRow {
                        id: p.id,
                        label: if multi {
                            format!("{} ({})", p.name, h.backend.descriptor().name)
                        } else {
                            p.name.clone()
                        },
                    })
            })
            .collect();
        rows.sort_by_key(|r| r.label.to_lowercase());
        rows
    }

    pub(super) fn render_workspaces_tab(
        &self,
        frame: &mut Frame,
        body_area: Rect,
        footer_area: Rect,
        ws: &WorkspacesState,
    ) {
        let merged = self.merged_workspaces();
        let active = self.active_workspace();

        // --- Startup header row ---
        let header = Line::from(vec![
            Span::styled(
                "Startup workspace: ",
                Style::default().fg(self.theme.text_secondary),
            ),
            Span::styled(
                startup_label(&self.config.startup_workspace, &merged),
                Style::default().fg(self.theme.text_accent),
            ),
            Span::styled(
                "  (s: change)",
                Style::default().fg(self.theme.text_secondary),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(header),
            Rect {
                height: 1.min(body_area.height),
                ..body_area
            },
        );
        let WorkspacesPanes {
            list: list_area,
            divider: divider_area,
            detail: detail_area,
        } = workspaces_panes(body_area);
        let list_width = list_area.width;
        self.render_settings_divider(frame, divider_area);

        // --- Workspace list ---
        let moving_target = match &ws.editing {
            Some(WorkspacesEditing::MovingProject { target, .. }) => Some(*target),
            _ => None,
        };
        let creating = match &ws.editing {
            Some(WorkspacesEditing::Creating { value }) => Some(value),
            _ => None,
        };
        let visible = (list_area.height as usize).saturating_sub(usize::from(creating.is_some()));
        let scroll = list_scroll_offset(ws.selected, visible);
        let name_width = (list_width as usize).saturating_sub(5);
        let mut last_y = list_area.y;
        for (i, w) in merged.iter().enumerate().skip(scroll).take(visible) {
            let y = list_area.y + (i - scroll) as u16;
            last_y = y + 1;
            let is_selected = i == ws.selected;
            let focused = ws.focus == WorkspacesFocus::List && creating.is_none();
            let style = if moving_target == Some(i) || (is_selected && focused) {
                self.theme.selection()
            } else if is_selected {
                Style::default()
                    .fg(self.theme.text_primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.text_secondary)
            };
            let prefix = if moving_target == Some(i) {
                "→ "
            } else if is_selected {
                "▸ "
            } else {
                "  "
            };
            let name = match (&ws.editing, is_selected) {
                (Some(WorkspacesEditing::Renaming { value }), true) => {
                    super::input_with_caret(value)
                }
                _ => {
                    let mut label = truncate_str(&w.label, name_width);
                    if w.name == active {
                        label.push_str(" •");
                    }
                    label
                }
            };
            let swatch = match workspace_color(w.color.as_deref()) {
                Some(c) => Span::styled("■ ", Style::default().fg(c)),
                None => Span::raw("  "),
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(prefix, style),
                    swatch,
                    Span::styled(name, style),
                ])),
                Rect {
                    y,
                    height: 1,
                    ..list_area
                },
            );
        }
        if let Some(value) = creating {
            let input_style = self.theme.selection().add_modifier(Modifier::UNDERLINED);
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!("  {}", super::input_with_caret(value)),
                    input_style,
                )),
                Rect {
                    y: last_y,
                    height: 1,
                    ..list_area
                },
            );
        }

        // --- Detail pane: colour, then projects ---
        if let Some(w) = merged.get(ws.selected) {
            let detail_focused = ws.focus == WorkspacesFocus::Detail;
            let row_style = |selected: bool| {
                if selected {
                    self.theme.selection()
                } else {
                    Style::default()
                }
            };
            let colour_selected = detail_focused && ws.detail_selected == 0;
            let colour_value = w.color.clone().unwrap_or_else(|| "(default)".to_string());
            let mut colour_spans = vec![Span::styled(
                format!("{:<10}", "Colour"),
                row_style(colour_selected),
            )];
            if let Some(c) = workspace_color(w.color.as_deref()) {
                colour_spans.push(Span::styled("■ ", Style::default().fg(c)));
            }
            colour_spans.push(Span::styled(
                colour_value,
                row_style(colour_selected).fg(self.theme.text_accent),
            ));
            let mut lines = vec![Line::from(colour_spans), Line::from("")];
            if let Some(WorkspacesEditing::Colour { picker }) = &ws.editing {
                // The picker takes the projects' place while it is open.
                lines.extend(self.colour_picker_lines(picker));
            } else {
                lines.push(Line::from(Span::styled(
                    "Projects",
                    Style::default().fg(self.theme.text_secondary),
                )));
                let projects = self.workspace_project_rows(w.name.as_deref());
                if projects.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  (no projects)",
                        Style::default().fg(self.theme.text_secondary),
                    )));
                }
                for (i, p) in projects.iter().enumerate() {
                    let selected = detail_focused && ws.detail_selected == i + 1;
                    let moving = matches!(
                        &ws.editing,
                        Some(WorkspacesEditing::MovingProject { project_id, .. }) if *project_id == p.id
                    );
                    let prefix = if moving { "» " } else { "  " };
                    lines.push(Line::from(Span::styled(
                        format!("{prefix}{}", p.label),
                        row_style(selected || moving),
                    )));
                }
            }
            frame.render_widget(Paragraph::new(lines), detail_area);
        }

        self.render_workspaces_footer(frame, footer_area, ws);
    }

    fn render_workspaces_footer(&self, frame: &mut Frame, footer_area: Rect, ws: &WorkspacesState) {
        let footer_text = match &ws.editing {
            Some(WorkspacesEditing::Creating { .. }) => "Enter: create  Esc: cancel",
            Some(WorkspacesEditing::Renaming { .. }) => "Enter: save  Esc: cancel",
            Some(WorkspacesEditing::Colour { picker }) => match picker.focus {
                ColourPickerFocus::Grid => "←↓↑→/hjkl: move  Enter: pick  Tab/#: hex  Esc: cancel",
                ColourPickerFocus::Hex => {
                    "Enter: save  type or paste #rrggbb  Tab: swatches  Esc: cancel"
                }
            },
            Some(WorkspacesEditing::MovingProject { .. }) => {
                "j/k: pick workspace  Enter: move  Esc: cancel"
            }
            None if ws.focus == WorkspacesFocus::List => {
                "n: new  r: rename  d: delete  J/K: reorder  s: startup  →: details  Tab: switch tab"
            }
            None => "Enter: pick colour  m: move project  ←: back  Tab: switch tab",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                footer_text,
                Style::default().fg(self.theme.text_secondary),
            )),
            footer_area,
        );
    }

    /// The open colour picker, as the lines under the Colour row: the swatch
    /// grid (cell 0 is "No colour"), the highlighted swatch's role and hex,
    /// then the hex row with a live preview.
    fn colour_picker_lines(&self, picker: &ColourPicker) -> Vec<Line<'static>> {
        let grid_focused = picker.focus == ColourPickerFocus::Grid;
        let secondary = Style::default().fg(self.theme.text_secondary);
        let bracket = if grid_focused {
            Style::default().fg(self.theme.text_primary)
        } else {
            secondary
        };
        let mut lines = Vec::new();
        let cells: Vec<Option<&crate::theme::ThemeSwatch>> = std::iter::once(None)
            .chain(picker.swatches.iter().map(Some))
            .collect();
        // `columns` was fitted to this pane's width before the frame was drawn.
        let columns = picker.columns.max(1);
        for (row, chunk) in cells.chunks(columns).enumerate() {
            let mut spans = Vec::with_capacity(chunk.len() * 3);
            for (col, cell) in chunk.iter().enumerate() {
                let selected = row * columns + col == picker.selected;
                let (open, close) = if selected { ("[", "]") } else { (" ", " ") };
                spans.push(Span::styled(open, bracket));
                spans.push(match cell {
                    Some(s) => Span::styled("■■", Style::default().fg(s.color)),
                    None => Span::styled("··", secondary),
                });
                spans.push(Span::styled(close, bracket));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(match picker.selected_swatch() {
            Some(s) => vec![
                Span::styled(
                    format!(" {}", s.role),
                    Style::default().fg(self.theme.text_primary),
                ),
                Span::styled(format!("  {}", s.hex), secondary),
            ],
            None => vec![Span::styled(" No colour (default)", secondary)],
        }));
        lines.push(Line::from(""));

        let label_style = if grid_focused {
            Style::default()
        } else {
            self.theme.selection()
        };
        let mut hex_spans = vec![Span::styled(format!("{:<10}", "Hex"), label_style)];
        let preview = picker.hex_value().and_then(|h| workspace_color(Some(&h)));
        hex_spans.push(match preview {
            Some(c) => Span::styled("■■ ", Style::default().fg(c)),
            None => Span::raw("   "),
        });
        let raw = picker.hex.value();
        let value_style = if !raw.trim().is_empty() && preview.is_none() {
            Style::default().fg(self.theme.diff_removed)
        } else {
            Style::default().fg(self.theme.text_accent)
        };
        if grid_focused && raw.is_empty() {
            hex_spans.push(Span::styled(
                "#rrggbb (Tab or # to type or paste)",
                secondary,
            ));
        } else if grid_focused {
            hex_spans.push(Span::styled(raw.to_string(), value_style));
        } else {
            hex_spans.push(Span::styled(
                super::input_with_caret(&picker.hex),
                value_style,
            ));
        }
        lines.push(Line::from(hex_spans));
        if let Some(err) = &picker.error {
            lines.push(Line::from(Span::styled(
                format!(" {err}"),
                Style::default().fg(self.theme.modal_error),
            )));
        }
        lines
    }

    /// Handle a keypress while the Workspaces tab is active. Every edit goes to
    /// every backend (see [`Self::apply_workspace_op`]).
    pub(super) async fn handle_workspaces_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        mut state: SettingsState,
    ) {
        use crossterm::event::KeyCode;

        let merged = self.merged_workspaces();
        let len = merged.len();

        // --- Editing ---
        if let Some(editing) = state.workspaces_state.editing.take() {
            let ws = &mut state.workspaces_state;
            match editing {
                WorkspacesEditing::Creating { mut value } => match key.code {
                    KeyCode::Enter => {
                        let raw = value.value().to_string();
                        if self.create_workspace(&raw).await.is_some() {
                            ws.selected = self.merged_workspaces().len().saturating_sub(1);
                        }
                    }
                    KeyCode::Esc => {}
                    _ => {
                        super::edit_text_input(&mut value, key);
                        ws.editing = Some(WorkspacesEditing::Creating { value });
                    }
                },
                WorkspacesEditing::Renaming { mut value } => match key.code {
                    KeyCode::Enter => {
                        if let Some(w) = merged.get(ws.selected) {
                            let raw = value.value().to_string();
                            self.rename_workspace_everywhere(w.name.clone(), &raw).await;
                        }
                    }
                    KeyCode::Esc => {}
                    _ => {
                        super::edit_text_input(&mut value, key);
                        ws.editing = Some(WorkspacesEditing::Renaming { value });
                    }
                },
                WorkspacesEditing::Colour { mut picker } => match picker.handle_key(key) {
                    PickerOutcome::Open => ws.editing = Some(WorkspacesEditing::Colour { picker }),
                    PickerOutcome::Cancel => {}
                    PickerOutcome::Pick(color) => {
                        if let Some(w) = merged.get(ws.selected) {
                            // An empty colour is `set_workspace_color`'s "clear".
                            let raw = color.unwrap_or_default();
                            self.set_workspace_color(w.name.clone(), &raw).await;
                        }
                    }
                },
                WorkspacesEditing::MovingProject { project_id, target } => {
                    use claude_commander_core::config::keybindings::BindableAction;
                    let step = |down: bool| {
                        if len == 0 {
                            0
                        } else if down {
                            (target + 1) % len
                        } else {
                            (target + len - 1) % len
                        }
                    };
                    match (self.config.keybindings.resolve(&key), key.code) {
                        (Some(BindableAction::NavigateDown), _) | (_, KeyCode::Down) => {
                            ws.editing = Some(WorkspacesEditing::MovingProject {
                                project_id,
                                target: step(true),
                            });
                        }
                        (Some(BindableAction::NavigateUp), _) | (_, KeyCode::Up) => {
                            ws.editing = Some(WorkspacesEditing::MovingProject {
                                project_id,
                                target: step(false),
                            });
                        }
                        (_, KeyCode::Enter) => {
                            if let Some(w) = merged.get(target) {
                                self.move_project_to_workspace(project_id, w.name.clone())
                                    .await;
                            }
                        }
                        (_, KeyCode::Esc) => {}
                        _ => {
                            ws.editing =
                                Some(WorkspacesEditing::MovingProject { project_id, target });
                        }
                    }
                }
            }
            self.clamp_workspaces_state(&mut state.workspaces_state);
            self.ui_state.modal = Modal::Settings(state);
            return;
        }

        // --- Navigation ---
        use claude_commander_core::config::keybindings::BindableAction;
        let action = self.config.keybindings.resolve(&key);
        match state.workspaces_state.focus {
            WorkspacesFocus::List => match (action, key.code) {
                (Some(BindableAction::NavigateDown), _) => {
                    let ws = &mut state.workspaces_state;
                    if len > 0 {
                        ws.selected = (ws.selected + 1) % len;
                        ws.detail_selected = 0;
                    }
                }
                (Some(BindableAction::NavigateUp), _) => {
                    let ws = &mut state.workspaces_state;
                    if len > 0 {
                        ws.selected = (ws.selected + len - 1) % len;
                        ws.detail_selected = 0;
                    }
                }
                (Some(BindableAction::Quit), _) | (_, KeyCode::Esc) => {
                    self.ui_state.modal = Modal::None;
                    return;
                }
                (_, KeyCode::Tab) => self.switch_settings_tab(&mut state, true),
                (_, KeyCode::BackTab) => self.switch_settings_tab(&mut state, false),
                (_, KeyCode::Right | KeyCode::Enter) => {
                    let ws = &mut state.workspaces_state;
                    ws.focus = WorkspacesFocus::Detail;
                    ws.detail_selected = 0;
                }
                (_, KeyCode::Char('n')) => {
                    state.workspaces_state.editing = Some(WorkspacesEditing::Creating {
                        value: Input::default(),
                    });
                }
                (_, KeyCode::Char('r')) => {
                    if let Some(w) = merged.get(state.workspaces_state.selected) {
                        state.workspaces_state.editing = Some(WorkspacesEditing::Renaming {
                            value: w.label.clone().into(),
                        });
                    }
                }
                (_, KeyCode::Char('d')) => match merged.get(state.workspaces_state.selected) {
                    Some(MergedWorkspace {
                        name: Some(name), ..
                    }) => {
                        let name = name.clone();
                        self.delete_workspace_everywhere(&name).await;
                    }
                    Some(_) => {
                        self.toast("Main can't be deleted — rename it instead".to_string());
                    }
                    None => {}
                },
                (_, KeyCode::Char(c @ ('J' | 'K'))) => {
                    let idx = state.workspaces_state.selected;
                    if let Some(moved) = self.reorder_workspace(idx, c == 'J').await {
                        state.workspaces_state.selected = moved;
                    }
                }
                (_, KeyCode::Char('s')) => {
                    let next = next_startup(&self.config.startup_workspace, &merged);
                    self.set_startup_workspace(next).await;
                }
                _ => {}
            },
            WorkspacesFocus::Detail => {
                let projects = merged
                    .get(state.workspaces_state.selected)
                    .map(|w| self.workspace_project_rows(w.name.as_deref()))
                    .unwrap_or_default();
                let rows = projects.len() + 1;
                let ws = &mut state.workspaces_state;
                match (action, key.code) {
                    (Some(BindableAction::NavigateDown), _) => {
                        ws.detail_selected = (ws.detail_selected + 1) % rows;
                    }
                    (Some(BindableAction::NavigateUp), _) => {
                        ws.detail_selected = (ws.detail_selected + rows - 1) % rows;
                    }
                    (Some(BindableAction::Quit), _) => {
                        self.ui_state.modal = Modal::None;
                        return;
                    }
                    (_, KeyCode::Esc | KeyCode::Left) => ws.focus = WorkspacesFocus::List,
                    (_, KeyCode::Tab) => self.switch_settings_tab(&mut state, true),
                    (_, KeyCode::BackTab) => self.switch_settings_tab(&mut state, false),
                    (_, KeyCode::Enter) if ws.detail_selected == 0 => {
                        let current = merged.get(ws.selected).and_then(|w| w.color.as_deref());
                        ws.editing = Some(WorkspacesEditing::Colour {
                            picker: Box::new(ColourPicker::open(&self.theme, current)),
                        });
                    }
                    (_, KeyCode::Char('m')) if ws.detail_selected > 0 => {
                        if let Some(p) = projects.get(ws.detail_selected - 1) {
                            ws.editing = Some(WorkspacesEditing::MovingProject {
                                project_id: p.id,
                                target: ws.selected,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        self.clamp_workspaces_state(&mut state.workspaces_state);
        self.ui_state.modal = Modal::Settings(state);
    }

    /// Keep the cursors inside the (possibly just shrunk) lists.
    fn clamp_workspaces_state(&self, ws: &mut WorkspacesState) {
        let merged = self.merged_workspaces();
        ws.selected = ws.selected.min(merged.len().saturating_sub(1));
        let projects = merged
            .get(ws.selected)
            .map(|w| self.workspace_project_rows(w.name.as_deref()).len())
            .unwrap_or(0);
        ws.detail_selected = ws.detail_selected.min(projects);
    }
}
