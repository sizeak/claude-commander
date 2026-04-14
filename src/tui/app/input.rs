//! Input handling: dispatch keyboard/mouse events to commands and modal handlers.

use super::*;

impl App {
    pub(super) async fn handle_input(&mut self, input: InputEvent) {
        match input {
            InputEvent::Key(key) => {
                debug!(
                    "Key event: code={:?} modifiers={:?} kind={:?}",
                    key.code, key.modifiers, key.kind
                );

                // Suppress stray bytes from unrecognized escape sequences.
                // When crossterm can't parse a multi-byte sequence (e.g. from
                // modifier combos the terminal encodes as CSI), it emits each
                // byte as a separate key event ~8ms apart.  We suppress all
                // events for a short window after an unrecognized one.
                let now = Instant::now();
                if now < self.suppress_keys_until {
                    debug!("Suppressing key event (escape sequence cooldown)");
                    return;
                }

                // Check for modal-specific handling first
                if !matches!(self.ui_state.modal, Modal::None) {
                    self.handle_modal_key(key).await;
                    return;
                }

                // Check for configurable leader key (quick-switch)
                let (leader_code, leader_mods) = self.config.parse_leader_key();
                if key.code == leader_code && key.modifiers == leader_mods {
                    self.open_quick_switch().await;
                    return;
                }

                // Number-jump: intercept digit keys when session numbers are enabled
                if self.config.show_session_numbers
                    && let crossterm::event::KeyCode::Char(c @ '0'..='9') = key.code
                    && key.modifiers.is_empty()
                {
                    let digit = c as u8 - b'0';
                    if let crate::tui::digit_accumulator::DigitResult::Jump(n) =
                        self.digit_accumulator.press(digit)
                    {
                        self.jump_to_session_number(n);
                    }
                    return;
                }

                // Convert to command and handle
                match UserCommand::from_key(key, &self.config.keybindings) {
                    Some(cmd) => self.handle_command(cmd).await,
                    None => {
                        // Unrecognized key event — likely the start of a
                        // broken escape sequence.  Suppress further events
                        // briefly so trailing bytes don't trigger commands.
                        self.suppress_keys_until = now + Duration::from_millis(50);
                    }
                }
            }
            InputEvent::Resize(_, _) => {
                // Terminal will re-render automatically
            }
            InputEvent::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_pane_at(mouse.column, ScrollDirection::Up);
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_pane_at(mouse.column, ScrollDirection::Down);
                }
                _ => {}
            },
            InputEvent::Paste(text) => {
                // Handle paste in modal input, ignore otherwise
                let clean = text.replace(['\n', '\r'], "");
                match &mut self.ui_state.modal {
                    Modal::Input { value, .. } => {
                        value.push_str(&clean);
                    }
                    Modal::PathInput {
                        value, completer, ..
                    } => {
                        value.push_str(&clean);
                        completer.invalidate();
                    }
                    _ => {}
                }
            }
        }
    }

    /// Handle modal key input
    pub(super) async fn handle_modal_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        match &mut self.ui_state.modal {
            Modal::Input {
                value, on_submit, ..
            } => match key.code {
                KeyCode::Enter => {
                    let action = on_submit.clone();
                    let value = value.clone();
                    self.ui_state.modal = Modal::None;
                    self.handle_input_submit(action, value).await;
                }
                KeyCode::Esc => {
                    self.ui_state.modal = Modal::None;
                }
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Char(c) => {
                    value.push(c);
                }
                _ => {}
            },

            Modal::PathInput {
                value,
                on_submit,
                completer,
                ..
            } => match key.code {
                KeyCode::Enter => {
                    let action = on_submit.clone();
                    let value = value.clone();
                    self.ui_state.modal = Modal::None;
                    self.handle_input_submit(action, value).await;
                }
                KeyCode::Esc => {
                    self.ui_state.modal = Modal::None;
                }
                KeyCode::Tab => {
                    let completed = completer.complete(value);
                    *value = completed;
                }
                KeyCode::Backspace => {
                    value.pop();
                    completer.invalidate();
                }
                KeyCode::Char(c) => {
                    value.push(c);
                    completer.invalidate();
                }
                _ => {}
            },

            Modal::Confirm { on_confirm, .. } => match key.code {
                KeyCode::Enter => {
                    let action = on_confirm.clone();
                    self.ui_state.modal = Modal::None;
                    self.handle_confirm(action).await;
                }
                KeyCode::Esc => {
                    self.ui_state.modal = Modal::None;
                }
                _ => {}
            },

            Modal::Loading { .. } => {
                // Non-interactive — swallow all keys while loading
            }

            Modal::Help | Modal::Error { .. } => {
                // Any key closes help/error
                self.ui_state.modal = Modal::None;
            }

            Modal::Settings(_) => {
                // Extract the state to avoid borrow conflict with &mut self
                let state = match std::mem::replace(&mut self.ui_state.modal, Modal::None) {
                    Modal::Settings(s) => s,
                    _ => unreachable!(),
                };
                self.handle_settings_key(key, state).await;
            }

            Modal::QuickSwitch {
                query,
                matches,
                selected_idx,
            } => {
                use crate::config::keybindings::BindableAction;

                // Resolve configurable bindings first for navigation
                match self.config.keybindings.resolve(&key) {
                    Some(BindableAction::NavigateUp) => {
                        if !matches.is_empty() {
                            *selected_idx = if *selected_idx == 0 {
                                matches.len() - 1
                            } else {
                                *selected_idx - 1
                            };
                        }
                    }
                    Some(BindableAction::NavigateDown) => {
                        if !matches.is_empty() {
                            *selected_idx = (*selected_idx + 1) % matches.len();
                        }
                    }
                    _ => match key.code {
                        KeyCode::Esc => {
                            self.ui_state.modal = Modal::None;
                        }
                        KeyCode::Enter => {
                            if let Some(m) = matches.get(*selected_idx) {
                                let session_id = m.session_id;
                                self.ui_state.modal = Modal::None;
                                self.ui_state.selected_session_id = Some(session_id);
                                if let Some(idx) =
                                    self.ui_state.list_items.iter().position(|item| {
                                        matches!(item, SessionListItem::Worktree { id, .. } if *id == session_id)
                                    })
                                {
                                    self.ui_state.list_state.select(Some(idx));
                                }
                                self.update_selection();
                                self.handle_select().await;
                            }
                        }
                        KeyCode::Tab => {
                            if let Some(m) = matches.get(*selected_idx) {
                                *query = m.title.clone();
                                self.refilter_quick_switch();
                            }
                        }
                        KeyCode::Backspace => {
                            query.pop();
                            self.refilter_quick_switch();
                        }
                        KeyCode::Char(c) => {
                            query.push(c);
                            self.refilter_quick_switch();
                        }
                        _ => {}
                    },
                }
            }

            Modal::None => {}
        }
    }

    /// Handle a user command
    pub(super) async fn handle_command(&mut self, cmd: UserCommand) {
        match cmd {
            UserCommand::NavigateUp => {
                self.ui_state.list_state.previous();
            }
            UserCommand::NavigateDown => {
                self.ui_state.list_state.next();
            }
            UserCommand::Select => {
                self.handle_select().await;
            }
            UserCommand::SelectShell => {
                self.handle_select_shell().await;
            }
            UserCommand::NewSession => {
                self.handle_new_session();
            }
            UserCommand::NewProject => {
                self.ui_state.modal = Modal::PathInput {
                    title: "Add Project".to_string(),
                    prompt: "Enter path to git repository:".to_string(),
                    value: std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    on_submit: InputAction::AddProject,
                    completer: PathCompleter::new(),
                };
            }
            UserCommand::DeleteSession => {
                self.handle_delete_session();
            }
            UserCommand::RestartSession => {
                self.handle_restart_session();
            }
            UserCommand::RemoveProject => {
                self.handle_remove_project();
            }
            UserCommand::OpenInEditor => {
                self.handle_open_in_editor().await;
            }
            UserCommand::OpenPullRequest => {
                self.handle_open_pull_request().await;
            }
            UserCommand::TogglePane => {
                let on_project = self.ui_state.selected_session_id.is_none()
                    && self.ui_state.selected_project_id.is_some();
                self.ui_state.right_pane_view = if on_project {
                    // Project: Shell → Info → Shell (no Preview)
                    match self.ui_state.right_pane_view {
                        RightPaneView::Shell => RightPaneView::Info,
                        _ => RightPaneView::Shell,
                    }
                } else {
                    // Session: Preview → Info → Shell → Preview
                    match self.ui_state.right_pane_view {
                        RightPaneView::Preview => RightPaneView::Info,
                        RightPaneView::Info => RightPaneView::Shell,
                        RightPaneView::Shell => RightPaneView::Preview,
                    }
                };
                self.ui_state.clear_right_pane = true;
                self.spawn_info_fetch();
            }
            UserCommand::TogglePaneReverse => {
                let on_project = self.ui_state.selected_session_id.is_none()
                    && self.ui_state.selected_project_id.is_some();
                self.ui_state.right_pane_view = if on_project {
                    // Project: Info → Shell → Info (no Preview)
                    match self.ui_state.right_pane_view {
                        RightPaneView::Info => RightPaneView::Shell,
                        _ => RightPaneView::Info,
                    }
                } else {
                    // Session: Shell → Info → Preview → Shell
                    match self.ui_state.right_pane_view {
                        RightPaneView::Preview => RightPaneView::Shell,
                        RightPaneView::Info => RightPaneView::Preview,
                        RightPaneView::Shell => RightPaneView::Info,
                    }
                };
                self.ui_state.clear_right_pane = true;
                self.spawn_info_fetch();
            }
            UserCommand::ShrinkLeftPane => {
                self.ui_state.left_pane_pct = self
                    .ui_state
                    .left_pane_pct
                    .saturating_sub(2)
                    .max(MIN_LEFT_PANE_PCT);
                self.save_left_pane_pct().await;
            }
            UserCommand::GrowLeftPane => {
                self.ui_state.left_pane_pct =
                    (self.ui_state.left_pane_pct + 2).min(MAX_LEFT_PANE_PCT);
                self.save_left_pane_pct().await;
            }
            UserCommand::ShowHelp => {
                self.ui_state.modal = Modal::Help;
            }
            UserCommand::ShowSettings => {
                let rows = self.build_settings_rows(SettingsTab::General);
                self.ui_state.modal = Modal::Settings(SettingsState {
                    tab: SettingsTab::General,
                    selected_row: 0,
                    editing: None,
                    rows,
                });
            }
            UserCommand::Quit => {
                self.ui_state.should_quit = true;
            }
            UserCommand::PageUp => self.active_pane_state().page_up(),
            UserCommand::PageDown => self.active_pane_state().page_down(),
            UserCommand::ScrollUp => self.active_pane_state().scroll_up(1),
            UserCommand::ScrollDown => self.active_pane_state().scroll_down(1),
            UserCommand::GenerateSummary => {
                // Context-specific: only works when Info pane is showing
                if self.ui_state.right_pane_view == RightPaneView::Info
                    && let Some(session_id) = self.ui_state.selected_session_id
                {
                    self.spawn_ai_summary_if_needed(session_id);
                }
            }
            _ => {}
        }
    }
}
