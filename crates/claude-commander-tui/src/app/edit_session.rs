use super::*;
use claude_commander_protocol::api::{CreateOptions, EditSession, SetSessionBase};
use crossterm::event::{KeyCode, KeyEvent};

#[derive(Debug, Clone)]
pub struct SessionEditor {
    pub session: SessionRef,
    pub name: Input,
    pub program: Input,
    pub original_program: String,
    pub sections: Vec<Option<String>>,
    pub section: usize,
    pub bases: Vec<(Option<SessionId>, String)>,
    pub base: usize,
    pub original_base_index: usize,
    pub keep_alive: bool,
    pub focus: usize,
    pub programs: Vec<String>,
    pub restart_now: bool,
}

impl SessionEditor {
    pub fn new(
        session: SessionRef,
        info: &claude_commander_protocol::api::SessionInfo,
        peers: &[claude_commander_protocol::api::SessionInfo],
        main_branch: &str,
    ) -> Self {
        let mut sections = vec![None];
        if let Some(section) = &info.section_override {
            sections.push(Some(section.clone()));
        }
        let section = usize::from(info.section_override.is_some());
        let mut bases = vec![(None, format!("Project base ({main_branch})"))];
        bases.extend(
            peers
                .iter()
                .filter(|s| s.project_id == info.project_id && s.session_id != info.session_id)
                .map(|s| (Some(s.session_id), format!("{} ({})", s.title, s.branch))),
        );
        let original_base = if let Some(branch) = &info.pr_base_branch {
            peers
                .iter()
                .find(|s| {
                    s.project_id == info.project_id
                        && s.branch == *branch
                        && s.session_id != info.session_id
                })
                .map(|s| s.session_id)
        } else {
            info.stack_parent_session_id
        };
        if let Some(id) = original_base
            && !bases.iter().any(|(base, _)| *base == Some(id))
        {
            bases.push((Some(id), "Current base (session unavailable)".into()));
        }
        if original_base.is_none()
            && let Some(branch) = &info.pr_base_branch
            && branch != main_branch
        {
            bases.push((None, format!("Current base ({branch})")));
        }
        let base = bases
            .iter()
            .position(|(id, _)| *id == original_base)
            .unwrap_or(0);
        let base = if original_base.is_none()
            && bases.len() > 1
            && bases.last().is_some_and(|(id, _)| id.is_none())
        {
            bases.len() - 1
        } else {
            base
        };
        Self {
            session,
            name: info.title.clone().into(),
            program: info
                .pending_program
                .as_ref()
                .unwrap_or(&info.program)
                .clone()
                .into(),
            original_program: info
                .pending_program
                .as_ref()
                .unwrap_or(&info.program)
                .clone(),
            sections,
            section,
            bases,
            base,
            original_base_index: base,
            keep_alive: info.keep_alive,
            focus: 0,
            programs: vec![
                info.pending_program
                    .as_ref()
                    .unwrap_or(&info.program)
                    .clone(),
            ],
            restart_now: false,
        }
    }
    pub fn load_options(&mut self, opts: CreateOptions) {
        for section in opts.sections {
            if !self.sections.contains(&Some(section.clone())) {
                self.sections.push(Some(section));
            }
        }
        for program in opts.programs {
            if !self.programs.contains(&program.command) {
                self.programs.push(program.command);
            }
        }
    }
    pub fn edit(&self, restart: bool) -> EditSession {
        let base = self.bases[self.base].0;
        EditSession {
            title: self.name.value().trim().into(),
            program: self.program.value().trim().into(),
            section: self.sections[self.section].clone(),
            keep_alive: self.keep_alive,
            base: (self.base != self.original_base_index).then_some(SetSessionBase {
                parent_session_id: base,
            }),
            restart,
        }
    }
    fn cycle(&mut self, forward: bool) {
        fn step(index: usize, len: usize, forward: bool) -> usize {
            if forward {
                (index + 1) % len
            } else {
                (index + len - 1) % len
            }
        }
        match self.focus {
            1 => {
                let index = self
                    .programs
                    .iter()
                    .position(|s| s == self.program.value())
                    .unwrap_or(0);
                self.program = self.programs[step(index, self.programs.len(), forward)]
                    .clone()
                    .into();
            }
            2 => self.section = step(self.section, self.sections.len(), forward),
            3 => self.base = step(self.base, self.bases.len(), forward),
            4 => self.keep_alive = !self.keep_alive,
            _ => {}
        }
    }
}

impl App {
    pub(super) fn handle_edit_session(&mut self) {
        let Some(session) = self.ui_state.selected_session_id else {
            return;
        };
        let Some(info) = self.session(session) else {
            return;
        };
        let Some(handle) = self.backend(session.backend) else {
            return;
        };
        let main = handle
            .view
            .snapshot
            .projects
            .iter()
            .find(|p| p.id == info.project_id)
            .map(|p| p.main_branch.as_str())
            .unwrap_or("main");
        let editor = SessionEditor::new(session, info, &handle.view.snapshot.sessions, main);
        self.ui_state.modal = Modal::EditSession(editor);
        let backend = self.backend_arc(session.backend);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let result = backend.create_options().await.map_err(|e| e.to_string());
            let _ = tx
                .send(AppEvent::StateUpdate(
                    StateUpdate::SessionEditOptionsLoaded { session, result },
                ))
                .await;
        });
    }
    pub(super) fn handle_edit_session_key(&mut self, key: KeyEvent) {
        let modal = std::mem::replace(&mut self.ui_state.modal, Modal::None);
        let (mut editor, warning) = match modal {
            Modal::EditSession(editor) => (editor, false),
            Modal::EditSessionRestart(editor) => (editor, true),
            other => {
                self.ui_state.modal = other;
                return;
            }
        };
        if warning {
            match key.code {
                KeyCode::Esc => self.ui_state.modal = Modal::EditSession(editor),
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                    editor.restart_now = !editor.restart_now;
                    self.ui_state.modal = Modal::EditSessionRestart(editor);
                }
                KeyCode::Char('y') => self.save_session_edit(editor, true),
                KeyCode::Char('n') => self.save_session_edit(editor, false),
                KeyCode::Enter => {
                    let restart = editor.restart_now;
                    self.save_session_edit(editor, restart);
                }
                _ => self.ui_state.modal = Modal::EditSessionRestart(editor),
            }
            return;
        }
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Tab | KeyCode::Down => editor.focus = (editor.focus + 1) % 5,
            KeyCode::BackTab | KeyCode::Up => editor.focus = (editor.focus + 4) % 5,
            KeyCode::Enter => {
                if editor.name.value().trim().is_empty() || editor.program.value().trim().is_empty()
                {
                    self.ui_state.status_message = Some((
                        "Name and program cannot be empty".into(),
                        Instant::now() + Duration::from_secs(4),
                    ));
                } else if editor.program.value().trim() != editor.original_program {
                    self.ui_state.modal = Modal::EditSessionRestart(editor);
                    return;
                } else {
                    self.save_session_edit(editor, false);
                    return;
                }
            }
            KeyCode::Left | KeyCode::Right
                if editor.focus == 1
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                editor.cycle(key.code == KeyCode::Right)
            }
            KeyCode::Left | KeyCode::Right if editor.focus >= 2 => {
                editor.cycle(key.code == KeyCode::Right)
            }
            KeyCode::Char(' ') if editor.focus >= 2 => editor.cycle(true),
            _ => match editor.focus {
                0 => {
                    edit_text_input(&mut editor.name, key);
                }
                1 => {
                    edit_text_input(&mut editor.program, key);
                }
                _ => {}
            },
        }
        self.ui_state.modal = Modal::EditSession(editor);
    }
    fn save_session_edit(&mut self, editor: SessionEditor, restart: bool) {
        let edit = editor.edit(restart);
        let backend = self.backend_arc(editor.session.backend);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            match backend.edit_session(editor.session.id, edit).await {
                Ok(outcome) => {
                    if let Some(outcome) = outcome
                        && let claude_commander_protocol::api::PrRetarget::Failed {
                            message, ..
                        } = outcome.pr
                    {
                        let _ = tx
                            .send(AppEvent::StateUpdate(StateUpdate::Error {
                                message: format!(
                                    "Session saved, but PR target update failed: {message}"
                                ),
                            }))
                            .await;
                    }

                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::SessionMutationApplied {
                            backend_id: editor.session.backend.0,
                            session_id: editor.session.id,
                        }))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::Error {
                            message: format!("Failed to edit session: {e}"),
                        }))
                        .await;
                }
            }
        });
    }
}

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    editor: &SessionEditor,
    warning: bool,
    theme: &Theme,
) {
    let area = super::modals::centered_rect(75, 14, area);
    frame.render_widget(Clear, area);
    let title = if warning {
        " Restart session? "
    } else {
        " Edit session "
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = if warning {
        vec![
            Line::from("Changing the program requires a fresh restart."),
            Line::from("Restarting stops the current agent and clears its conversation."),
            Line::from("Saving for later keeps the current agent running."),
            Line::from("The new program starts fresh on the next restart."),
            Line::from(""),
            Line::from(if editor.restart_now {
                "[Yes, restart and clear]    No, save for next restart"
            } else {
                " Yes, restart and clear    [No, save for next restart]"
            }),
            Line::from("←/→ choose · Enter confirm · Esc back"),
        ]
    } else {
        let values = [
            format!(
                "Name: {}",
                if editor.focus == 0 {
                    input_with_caret(&editor.name)
                } else {
                    editor.name.value().into()
                }
            ),
            format!(
                "Program: {}",
                if editor.focus == 1 {
                    input_with_caret(&editor.program)
                } else {
                    editor.program.value().into()
                }
            ),
            format!(
                "Section: {}",
                editor.sections[editor.section]
                    .as_deref()
                    .unwrap_or("Automatic")
            ),
            format!("Stack base: {}", editor.bases[editor.base].1),
            format!(
                "Keep alive: {}",
                if editor.keep_alive { "Yes" } else { "No" }
            ),
        ];
        let mut lines: Vec<Line> = values
            .into_iter()
            .enumerate()
            .map(|(i, text)| {
                Line::styled(
                    format!("{} {text}", if editor.focus == i { ">" } else { " " }),
                    Style::default().fg(if editor.focus == i {
                        theme.text_primary
                    } else {
                        theme.text_secondary
                    }),
                )
            })
            .collect();
        lines.extend([
            Line::from(""),
            Line::from("Tab/↑/↓ field · ←/→ option · Ctrl+←/→ program"),
            Line::from("Enter save · Esc cancel"),
            Line::from("Stack base changes metadata and PR target; git history is unchanged."),
        ]);
        lines
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
