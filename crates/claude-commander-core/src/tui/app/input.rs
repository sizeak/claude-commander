//! Input handling: keyboard events, modal keys, and command dispatch.

use super::*;

/// One screen's worth of scroll for the Help modal. Approximate — the
/// render pass clamps against the real content-area height each frame.
const HELP_PAGE: u16 = 10;

/// Outcome of interpreting a key inside the Help modal.
#[derive(Debug, PartialEq, Eq)]
enum HelpKey {
    ScrollBy(i16),
    Home,
    End,
    Close,
    Ignore,
}

/// Classify a key press within the Help modal. Kept as a free function
/// so it can be unit-tested without constructing an `App`.
///
/// Raw `KeyCode` matches take precedence over `kb.resolve` so modal-native
/// keys (arrows, Enter, Esc) are not shadowed by global bindings like
/// `NavigateUp`/`Submit`. `kb.resolve` fills in configured scroll bindings
/// — notably the default `Ctrl-u`/`Ctrl-d` for `PageUp`/`PageDown`.
fn classify_help_key(key: &crossterm::event::KeyEvent, kb: &crate::config::KeyBindings) -> HelpKey {
    use crossterm::event::KeyCode;

    match key.code {
        KeyCode::Up => return HelpKey::ScrollBy(-1),
        KeyCode::Down => return HelpKey::ScrollBy(1),
        KeyCode::PageUp => return HelpKey::ScrollBy(-(HELP_PAGE as i16)),
        KeyCode::PageDown => return HelpKey::ScrollBy(HELP_PAGE as i16),
        KeyCode::Home => return HelpKey::Home,
        KeyCode::End => return HelpKey::End,
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?') => {
            return HelpKey::Close;
        }
        _ => {}
    }

    match kb.resolve(key) {
        Some(BindableAction::ScrollUp) => HelpKey::ScrollBy(-1),
        Some(BindableAction::ScrollDown) => HelpKey::ScrollBy(1),
        Some(BindableAction::PageUp) => HelpKey::ScrollBy(-(HELP_PAGE as i16)),
        Some(BindableAction::PageDown) => HelpKey::ScrollBy(HELP_PAGE as i16),
        _ => HelpKey::Ignore,
    }
}

/// Which filterable modal needs its filter recomputed after a paste.
/// Used to defer the `&mut self` refilter call until after the
/// `&mut self.ui_state.modal` borrow has been released.
#[derive(Debug, PartialEq, Eq)]
enum PasteRefilter {
    CheckoutBranch,
    QuickSwitch,
}

/// Append clipboard text to the open modal's input field. Newlines are
/// stripped so a multi-line paste doesn't accidentally submit. Returns
/// `Some(PasteRefilter::…)` when the caller still needs to recompute a
/// filtered list via an `&mut self` helper; `None` when handling is
/// complete (or the modal has no text field).
fn apply_paste_to_modal(modal: &mut Modal, text: &str) -> Option<PasteRefilter> {
    let clean = text.replace(['\n', '\r'], "");
    match modal {
        Modal::Input { value, .. } => {
            super::insert_into_input(value, &clean);
            None
        }
        Modal::PathInput {
            value,
            completer,
            scroll,
            ..
        } => {
            super::insert_into_input(value, &clean);
            completer.refilter(value.value());
            *scroll = 0;
            None
        }
        Modal::CheckoutBranch { query, .. } => {
            super::insert_into_input(query, &clean);
            Some(PasteRefilter::CheckoutBranch)
        }
        Modal::QuickSwitch { query, .. } => {
            super::insert_into_input(query, &clean);
            Some(PasteRefilter::QuickSwitch)
        }
        // The comment draft is multi-line capable, so it gets the raw text
        // (newline handling lives in `paste_into_draft`), not `clean`.
        Modal::ReviewDiff(state) => {
            state.paste_into_draft(text);
            None
        }
        _ => None,
    }
}

/// Plain printable characters (no modifier, or Shift only) belong in a
/// fuzzy-search query box and must not be intercepted by global j/k →
/// NavigateUp/Down bindings. Other key combos (Ctrl/Alt, arrows, Tab, …)
/// return `None` and fall through to the configurable resolver.
fn palette_text_char(key: &crossterm::event::KeyEvent) -> Option<char> {
    use crossterm::event::{KeyCode, KeyModifiers};
    if let KeyCode::Char(c) = key.code
        && (key.modifiers - KeyModifiers::SHIFT).is_empty()
    {
        return Some(c);
    }
    None
}

/// Re-list the highlighted project's branches so the New Session dialog's
/// existing-branch collision hint tracks the project the user is now targeting.
/// Results are memoized per repo path in the picker, so listing runs at most
/// once per project rather than on every navigation keystroke.
fn refresh_branch_hint(
    existing_branches: &mut Option<Vec<String>>,
    picker: &mut super::ProjectPicker,
) {
    let Some(path) = picker.selected_repo_path() else {
        *existing_branches = None;
        return;
    };
    *existing_branches = picker
        .branch_cache
        .entry(path.clone())
        .or_insert_with(|| super::actions::existing_branch_names(&path))
        .clone();
}

/// Result of routing a key through the New Session (`Modal::Input`) dialog.
#[derive(Debug, PartialEq, Eq)]
enum InputKeyOutcome {
    /// The key mutated in-modal state (focus, expansion, filter, name text) and
    /// the dialog stays open.
    Handled,
    /// Submit the dialog (create the session / apply the input action).
    Submit,
    /// Close the dialog without submitting.
    Cancel,
}

/// Pure key routing for the New Session dialog, mirroring `apply_paste_to_modal`
/// so it can be unit-tested without an `App`. Owns focus movement, dropdown
/// expand/collapse, project-filter editing, and name-field editing. Returns
/// `Submit`/`Cancel` for the two outcomes the caller must action (they need
/// `App` to run); everything else is `Handled`.
///
/// Interaction (collapsed): ↑/↓ and Tab/Shift+Tab move focus between the present
/// rows; Enter submits; Esc cancels; on a picker row Space/→ opens the dropdown,
/// and typing on the Project row opens it and starts filtering. Expanded: ↑/↓
/// navigate the picker, Enter/Space/→ confirm-and-collapse, Esc collapses, and
/// (Project only) characters filter.
fn handle_input_modal_key(modal: &mut Modal, key: crossterm::event::KeyEvent) -> InputKeyOutcome {
    use super::InputFocus;
    use crossterm::event::KeyCode;
    let Modal::Input {
        value,
        existing_branches,
        project_picker,
        program_picker,
        focus,
        expanded,
        ..
    } = modal
    else {
        return InputKeyOutcome::Handled;
    };

    // --- Expanded: keys drive the open dropdown. ---
    if *expanded {
        match focus {
            InputFocus::Project => {
                let Some(picker) = project_picker.as_mut() else {
                    *expanded = false;
                    return InputKeyOutcome::Handled;
                };
                match key.code {
                    KeyCode::Up => {
                        picker.select_up();
                        refresh_branch_hint(existing_branches, picker);
                    }
                    KeyCode::Down => {
                        picker.select_down();
                        refresh_branch_hint(existing_branches, picker);
                    }
                    KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Esc => {
                        *expanded = false;
                    }
                    KeyCode::Backspace => {
                        if picker.filter.pop().is_some() {
                            picker.apply_filter();
                            refresh_branch_hint(existing_branches, picker);
                        }
                    }
                    _ => {
                        if let Some(c) = palette_text_char(&key) {
                            picker.filter.push(c);
                            picker.apply_filter();
                            refresh_branch_hint(existing_branches, picker);
                        }
                    }
                }
            }
            InputFocus::Program => {
                let Some(picker) = program_picker.as_mut() else {
                    *expanded = false;
                    return InputKeyOutcome::Handled;
                };
                match key.code {
                    KeyCode::Up => picker.select_up(),
                    KeyCode::Down => picker.select_down(),
                    KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Esc => {
                        *expanded = false;
                    }
                    _ => {}
                }
            }
            // The name field never expands; treat as a stray state and reset.
            InputFocus::Name => *expanded = false,
        }
        return InputKeyOutcome::Handled;
    }

    // --- Collapsed: Enter/Esc act on the whole dialog; movement + activation. ---
    let has_project = project_picker.is_some();
    let has_program = program_picker.is_some();
    match key.code {
        KeyCode::Enter => {
            // A project picker with no current match has nothing to create
            // under. Rather than silently ignore Enter, reopen the Project
            // dropdown so the `(no matching projects)` row is visible and the
            // user can see why — and keep the gate here (pure + testable)
            // rather than in the `App` caller.
            if project_picker
                .as_ref()
                .is_some_and(|p| p.selected_id().is_none())
            {
                *focus = InputFocus::Project;
                *expanded = true;
                return InputKeyOutcome::Handled;
            }
            return InputKeyOutcome::Submit;
        }
        KeyCode::Esc => return InputKeyOutcome::Cancel,
        KeyCode::Tab | KeyCode::Down => *focus = focus.next(has_project, has_program),
        KeyCode::BackTab | KeyCode::Up => *focus = focus.prev(has_project, has_program),
        _ => match focus {
            InputFocus::Name => {
                super::edit_text_input(value, key);
            }
            InputFocus::Project => match key.code {
                // Space / → open the dropdown.
                KeyCode::Char(' ') | KeyCode::Right if has_project => *expanded = true,
                // Typing a filter char opens the dropdown and starts filtering.
                _ => {
                    if let (Some(picker), Some(c)) =
                        (project_picker.as_mut(), palette_text_char(&key))
                    {
                        *expanded = true;
                        picker.filter.push(c);
                        picker.apply_filter();
                        refresh_branch_hint(existing_branches, picker);
                    }
                }
            },
            InputFocus::Program => {
                if matches!(key.code, KeyCode::Char(' ') | KeyCode::Right) && has_program {
                    *expanded = true;
                }
            }
        },
    }
    InputKeyOutcome::Handled
}

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

                // Voice input (Alt-V) is intercepted before modal routing so it
                // works whether the conversation overlay (or any modal) is open
                // or not — mirroring how spoken replies play regardless of UI
                // state. Its Alt modifier means it never shadows text entry.
                if self.config.keybindings.resolve(&key) == Some(BindableAction::ToggleVoiceInput) {
                    self.toggle_voice_input().await;
                    return;
                }

                // Check for modal-specific handling first
                if !matches!(self.ui_state.modal, Modal::None) {
                    self.handle_modal_key(key).await;
                    return;
                }

                // Check for configurable leader key (quick-switch).
                // Shift+<leader> opens directly in command-only mode
                // (VSCode-style command palette). We check the Shift-variant
                // first so it wins when the leader itself carries no Shift.
                let (leader_code, leader_mods) = self.config.parse_leader_key();
                if key.code == leader_code
                    && key.modifiers == (leader_mods | crossterm::event::KeyModifiers::SHIFT)
                    && !leader_mods.contains(crossterm::event::KeyModifiers::SHIFT)
                {
                    self.open_quick_switch_with_mode(PaletteMode::CommandOnly)
                        .await;
                    return;
                }
                if key.code == leader_code && key.modifiers == leader_mods {
                    self.open_quick_switch_with_mode(PaletteMode::Unified).await;
                    return;
                }

                // Ctrl+Space always opens the quick-switch palette, mirroring
                // the in-session switcher (see `tmux/attach.rs`) so the same
                // physical shortcut works whether attached or in the tree.
                if key.code == crossterm::event::KeyCode::Char(' ')
                    && key.modifiers == crossterm::event::KeyModifiers::CONTROL
                {
                    self.open_quick_switch_with_mode(PaletteMode::Unified).await;
                    return;
                }

                // Number-jump: intercept digit keys to select by session number.
                if let crossterm::event::KeyCode::Char(c @ '0'..='9') = key.code
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
                    if let Modal::ReviewDiff(state) = &mut self.ui_state.modal {
                        state.wheel(false);
                    } else if !matches!(self.ui_state.modal, Modal::None) {
                        self.modal_wheel(false);
                    } else {
                        self.scroll_pane_at(mouse.column, ScrollDirection::Up);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Modal::ReviewDiff(state) = &mut self.ui_state.modal {
                        state.wheel(true);
                    } else if !matches!(self.ui_state.modal, Modal::None) {
                        self.modal_wheel(true);
                    } else {
                        self.scroll_pane_at(mouse.column, ScrollDirection::Down);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    // In the review view, a click selects a file in the tree or
                    // positions the diff cursor, depending on which pane it hits.
                    let body = self.ui_state.review_body_rect;
                    let files = self.ui_state.review_file_list_rect;
                    if matches!(self.ui_state.modal, Modal::ReviewDiff(_)) {
                        let (col, row) = (mouse.column, mouse.row);
                        // A footer button replays the key it labels through the
                        // normal review key path (which expects the modal to be
                        // extracted first, exactly like the keyboard dispatch).
                        if let Some(key) =
                            super::review::review_button_at(&self.ui_state.review_buttons, col, row)
                        {
                            self.ui_state.review_last_click = None;
                            if let Modal::ReviewDiff(state) =
                                std::mem::replace(&mut self.ui_state.modal, Modal::None)
                            {
                                self.handle_review_key(key, state).await;
                            }
                            return;
                        }
                        let mut selected_file = false;
                        if let Modal::ReviewDiff(state) = &mut self.ui_state.modal {
                            let in_files = files.is_some_and(|r| {
                                col >= r.x
                                    && col < r.x + r.width
                                    && row >= r.y
                                    && row < r.y + r.height
                            });
                            if let Some(rect) = files.filter(|_| in_files) {
                                self.ui_state.review_last_click = None;
                                state.click_file_list_at(col, row, rect);
                                selected_file = true;
                            } else if let Some(rect) = body {
                                // A double-click on the same body row selects that
                                // line and opens its comment box (like right-click);
                                // a single click just positions the cursor.
                                use super::selection::DOUBLE_CLICK_WINDOW;
                                let now = Instant::now();
                                let is_double = matches!(
                                    self.ui_state.review_last_click,
                                    Some((prev_row, prev_at))
                                        if prev_row == row
                                            && now.duration_since(prev_at) <= DOUBLE_CLICK_WINDOW
                                );
                                if is_double {
                                    self.ui_state.review_last_click = None;
                                    if !state.double_click_comment(col, row, rect) {
                                        state.click_at(col, row, rect);
                                    }
                                } else {
                                    state.click_at(col, row, rect);
                                    self.ui_state.review_last_click = Some((row, now));
                                }
                            }
                        }
                        // A file-list click may have changed the selected file;
                        // kick off the lazy image fetch for it. The keyboard nav
                        // path does this in `handle_review_key`, but mouse
                        // selection bypasses that — without this, clicking an
                        // image file leaves it stuck on "Loading image…".
                        if selected_file && let Modal::ReviewDiff(state) = &self.ui_state.modal {
                            self.ensure_review_image(state).await;
                        }
                        return;
                    }
                    // List modals: a click highlights the row under the
                    // cursor, a double-click activates it (same convention
                    // as the session tree).
                    if matches!(
                        self.ui_state.modal,
                        Modal::QuickSwitch { .. }
                            | Modal::CheckoutBranch { .. }
                            | Modal::PathInput { .. }
                    ) {
                        self.handle_modal_list_click(mouse.column, mouse.row).await;
                        return;
                    }
                    // Remaining modals are keyboard-only; an underlying row
                    // select would be confusing.
                    if !matches!(self.ui_state.modal, Modal::None) {
                        return;
                    }
                    self.handle_left_click(mouse.column, mouse.row).await;
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    // Drag-select a line range in the review view.
                    let body = self.ui_state.review_body_rect;
                    if let Modal::ReviewDiff(state) = &mut self.ui_state.modal
                        && let Some(rect) = body
                    {
                        state.drag_at(mouse.column, mouse.row, rect);
                    }
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    // Right-click comments in the review view: with no active
                    // selection it first selects the clicked line, otherwise it
                    // comments on the current selection (mouse equivalent of v+Enter).
                    let body = self.ui_state.review_body_rect;
                    if let Modal::ReviewDiff(state) = &mut self.ui_state.modal
                        && let Some(rect) = body
                    {
                        state.right_click_comment(mouse.column, mouse.row, rect);
                    }
                }
                _ => {}
            },
            InputEvent::Paste(text) => {
                // A paste refilters the list, so drop any pending first-click.
                self.ui_state.modal_list_last_click = None;
                // Handle paste in modal input, ignore otherwise
                match apply_paste_to_modal(&mut self.ui_state.modal, &text) {
                    Some(PasteRefilter::CheckoutBranch) => self.refilter_checkout_branches(),
                    Some(PasteRefilter::QuickSwitch) => self.refilter_quick_switch(),
                    None => {}
                }
            }
        }
    }

    /// Handle modal key input
    pub(super) async fn handle_modal_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        // Any keystroke can refilter the list or swap the modal, so a
        // pending first-click no longer points at a meaningful row.
        self.ui_state.modal_list_last_click = None;

        // The conversation overlay owns all keys while open (typing, send,
        // scroll, close) — dispatch before the shared modal match to avoid a
        // double mutable borrow of `self`.
        if matches!(self.ui_state.modal, Modal::Conversation { .. }) {
            self.handle_conversation_key(key).await;
            return;
        }

        match &mut self.ui_state.modal {
            Modal::Input { .. } => {
                // All in-modal routing (focus, dropdown expand/collapse, filter
                // editing) lives in the pure `handle_input_modal_key` helper so
                // it is unit-testable without an `App`.
                match handle_input_modal_key(&mut self.ui_state.modal, key) {
                    InputKeyOutcome::Handled => {}
                    InputKeyOutcome::Cancel => self.ui_state.modal = Modal::None,
                    InputKeyOutcome::Submit => {
                        // `handle_input_modal_key` only returns `Submit` once a
                        // project (if any) is selectable, so no re-gating here.
                        let Modal::Input {
                            value,
                            on_submit,
                            project_picker,
                            program_picker,
                            ..
                        } = &self.ui_state.modal
                        else {
                            return;
                        };
                        let mut action = on_submit.clone();
                        // A chosen project overrides the one baked in at open time.
                        if let (InputAction::CreateSession { project_id, .. }, Some(picker)) =
                            (&mut action, project_picker.as_ref())
                            && let Some(chosen) = picker.selected_id()
                        {
                            *project_id = chosen;
                        }
                        let value = value.value().to_string();
                        let program = program_picker.as_ref().and_then(|p| p.selected_command());
                        self.ui_state.modal = Modal::None;
                        self.handle_input_submit(action, value, program).await;
                    }
                }
            }

            Modal::PathInput {
                value,
                completer,
                scroll,
                ..
            } => {
                use crate::config::keybindings::BindableAction;

                // Plain printable chars are text input — keep them out of
                // the j/k navigation bindings.
                if palette_text_char(&key).is_some() {
                    if super::edit_text_input(value, key) {
                        completer.refilter(value.value());
                        *scroll = 0;
                    }
                    return;
                }

                // Arrow keys (and Ctrl-n/p aliases) navigate the completion
                // list via the configurable resolver.
                match self.config.keybindings.resolve(&key) {
                    Some(BindableAction::NavigateUp) => {
                        completer.move_selection_up();
                        if let (_, Some(idx)) = completer.visible_completions() {
                            *scroll = super::actions::adjust_list_scroll(
                                idx,
                                *scroll,
                                super::actions::LIST_MAX_VISIBLE,
                            );
                        }
                    }
                    Some(BindableAction::NavigateDown) => {
                        completer.move_selection_down();
                        if let (_, Some(idx)) = completer.visible_completions() {
                            *scroll = super::actions::adjust_list_scroll(
                                idx,
                                *scroll,
                                super::actions::LIST_MAX_VISIBLE,
                            );
                        }
                    }
                    _ => match key.code {
                        KeyCode::Enter => {
                            self.submit_path_input().await;
                        }
                        KeyCode::Esc => {
                            self.ui_state.modal = Modal::None;
                        }
                        KeyCode::Tab => {
                            // Tab extends the input to the longest common
                            // prefix. A single match completes fully + `/`
                            // and `refilter` below surfaces that dir's
                            // children so the user can keep drilling in.
                            let completed = completer.complete(value.value());
                            *value = completed.into();
                            completer.refilter(value.value());
                            *scroll = 0;
                        }
                        // Backspace/Delete/cursor moves and word/line edits.
                        _ => {
                            if super::edit_text_input(value, key) {
                                completer.refilter(value.value());
                                *scroll = 0;
                            }
                        }
                    },
                }
            }

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

            Modal::Help { scroll } => match classify_help_key(&key, &self.config.keybindings) {
                HelpKey::ScrollBy(n) => {
                    *scroll = scroll.saturating_add_signed(n);
                }
                HelpKey::Home => {
                    *scroll = 0;
                }
                HelpKey::End => {
                    *scroll = u16::MAX;
                }
                HelpKey::Close => {
                    self.ui_state.modal = Modal::None;
                }
                HelpKey::Ignore => {}
            },

            Modal::Error { .. } => {
                // Any key closes the error modal.
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
                scroll,
                ..
            } => {
                use crate::config::keybindings::BindableAction;

                // Plain printable chars are text input — keep them out of
                // the j/k navigation bindings.
                if palette_text_char(&key).is_some() {
                    if super::edit_text_input(query, key) {
                        self.refilter_quick_switch();
                    }
                    return;
                }

                // Arrow keys (and Ctrl-n/p aliases) navigate the match list.
                match self.config.keybindings.resolve(&key) {
                    Some(BindableAction::NavigateUp) => {
                        if !matches.is_empty() {
                            *selected_idx = if *selected_idx == 0 {
                                matches.len() - 1
                            } else {
                                *selected_idx - 1
                            };
                            *scroll = super::actions::adjust_list_scroll(
                                *selected_idx,
                                *scroll,
                                super::actions::LIST_MAX_VISIBLE,
                            );
                        }
                    }
                    Some(BindableAction::NavigateDown) => {
                        if !matches.is_empty() {
                            *selected_idx = (*selected_idx + 1) % matches.len();
                            *scroll = super::actions::adjust_list_scroll(
                                *selected_idx,
                                *scroll,
                                super::actions::LIST_MAX_VISIBLE,
                            );
                        }
                    }
                    _ => match key.code {
                        KeyCode::Esc => {
                            self.ui_state.modal = Modal::None;
                        }
                        KeyCode::Enter => {
                            self.activate_quick_switch_selection().await;
                        }
                        KeyCode::Tab => {
                            // Tab autocompletes a session title into the
                            // query for further refinement. For command rows
                            // this is meaningless, so skip.
                            if let Some(QuickSwitchItem::Session(m)) =
                                matches.get(*selected_idx).cloned()
                            {
                                *query = m.title.into();
                                self.refilter_quick_switch();
                            }
                        }
                        // Backspace/Delete/cursor moves and word/line edits.
                        _ => {
                            if super::edit_text_input(query, key) {
                                self.refilter_quick_switch();
                            }
                        }
                    },
                }
            }

            Modal::CheckoutBranch {
                query,
                all_branches: _,
                filtered,
                selected_idx,
                scroll,
                ..
            } => {
                use crate::config::keybindings::BindableAction;

                // Plain printable chars are text input — keep them out of
                // the j/k navigation bindings.
                if palette_text_char(&key).is_some() {
                    if super::edit_text_input(query, key) {
                        self.refilter_checkout_branches();
                    }
                    return;
                }

                // Arrow keys (and Ctrl-n/p aliases) navigate the branch list.
                match self.config.keybindings.resolve(&key) {
                    Some(BindableAction::NavigateUp) => {
                        if !filtered.is_empty() {
                            *selected_idx = if *selected_idx == 0 {
                                filtered.len() - 1
                            } else {
                                *selected_idx - 1
                            };
                            // Ensure selection stays visible
                            if *selected_idx < *scroll {
                                *scroll = *selected_idx;
                            }
                        }
                    }
                    Some(BindableAction::NavigateDown) => {
                        if !filtered.is_empty() {
                            *selected_idx = (*selected_idx + 1) % filtered.len();
                            // Scroll forward when running off the bottom; a
                            // conservative window of 10 rows keeps the selection
                            // visible without knowing the exact pane height here.
                            let visible_rows: usize = 10;
                            if *selected_idx >= scroll.saturating_add(visible_rows) {
                                *scroll = selected_idx.saturating_sub(visible_rows - 1);
                            }
                            if *selected_idx < *scroll {
                                *scroll = *selected_idx;
                            }
                        }
                    }
                    _ => match key.code {
                        KeyCode::Esc => {
                            self.ui_state.modal = Modal::None;
                        }
                        KeyCode::Enter => {
                            self.activate_checkout_selection().await;
                        }
                        // Backspace/Delete/cursor moves and word/line edits.
                        _ => {
                            if super::edit_text_input(query, key) {
                                self.refilter_checkout_branches();
                            }
                        }
                    },
                }
            }

            Modal::ReviewDiff(_) => {
                // Extract the state to avoid a borrow conflict with &mut self.
                let state = match std::mem::replace(&mut self.ui_state.modal, Modal::None) {
                    Modal::ReviewDiff(s) => s,
                    _ => unreachable!(),
                };
                self.handle_review_key(key, state).await;
            }

            // Handled by the early dispatch above.
            Modal::Conversation { .. } => {}

            Modal::None => {}
        }
    }

    /// Handle a left-mouse click at the given absolute terminal position.
    ///
    /// Clicks outside the session list area are ignored. Clicks on a selectable
    /// row move the highlight there and refresh the preview; two clicks on the
    /// same row within [`DOUBLE_CLICK_WINDOW`] act as `UserCommand::Select`
    /// (attach for sessions, toggle for section headers).
    async fn handle_left_click(&mut self, col: u16, row: u16) {
        use super::selection::DOUBLE_CLICK_WINDOW;

        // Status-bar action buttons sit outside the list. A hit dispatches the
        // bound command — behaving exactly like the keypress — and consumes the
        // click.
        if let Some(action) = crate::tui::hotkey::button_at(&self.ui_state.action_buttons, col, row)
        {
            self.ui_state.last_left_click = None;
            self.handle_command(UserCommand::from(action)).await;
            return;
        }

        let Some(idx) = self.list_index_at(col, row) else {
            self.ui_state.last_left_click = None;
            return;
        };
        // Skip rows that aren't selectable (e.g. Spacer).
        match self.ui_state.list_items.get(idx) {
            Some(item) if !item.is_selectable() => {
                self.ui_state.last_left_click = None;
                return;
            }
            None => {
                self.ui_state.last_left_click = None;
                return;
            }
            _ => {}
        }

        let now = Instant::now();
        let is_double_click = matches!(
            self.ui_state.last_left_click,
            Some((prev_idx, prev_at))
                if prev_idx == idx && now.duration_since(prev_at) <= DOUBLE_CLICK_WINDOW
        );

        if self.ui_state.list_state.selected() != Some(idx) {
            self.ui_state.list_state.select(Some(idx));
            self.update_selection();
            self.ui_state.preview_update_spawned_at = None;
            self.spawn_preview_update();
        }

        if is_double_click {
            // Consume the click pair so a third click within the window
            // doesn't fire again.
            self.ui_state.last_left_click = None;
            self.handle_command(UserCommand::Select).await;
        } else {
            self.ui_state.last_left_click = Some((idx, now));
        }
    }

    /// Handle a left click while a list modal (quick-switch, checkout-branch,
    /// path-input) is open. A click on a row moves the highlight there; a
    /// second click on the same row within [`DOUBLE_CLICK_WINDOW`] activates
    /// it, exactly as Enter would. Clicks anywhere else are ignored.
    pub(super) async fn handle_modal_list_click(&mut self, col: u16, row: u16) {
        use super::modals::modal_list_index_at;
        use super::selection::DOUBLE_CLICK_WINDOW;

        let Some(rows) = self.ui_state.modal_list_rect else {
            return;
        };
        let clicked = match &mut self.ui_state.modal {
            Modal::QuickSwitch {
                matches,
                selected_idx,
                scroll,
                ..
            } => modal_list_index_at(col, row, rows, *scroll, matches.len())
                .inspect(|&idx| *selected_idx = idx),
            Modal::CheckoutBranch {
                filtered,
                selected_idx,
                scroll,
                ..
            } => modal_list_index_at(col, row, rows, *scroll, filtered.len())
                .inspect(|&idx| *selected_idx = idx),
            Modal::PathInput {
                completer, scroll, ..
            } => {
                let len = completer.visible_completions().0.len();
                modal_list_index_at(col, row, rows, *scroll, len)
                    .inspect(|&idx| completer.select(idx))
            }
            _ => return,
        };

        let Some(idx) = clicked else {
            // Border, input line, or an empty row: not a row, so any
            // pending first-click is stale.
            self.ui_state.modal_list_last_click = None;
            return;
        };
        let now = Instant::now();
        let is_double_click = matches!(
            self.ui_state.modal_list_last_click,
            Some((prev_idx, prev_at))
                if prev_idx == idx && now.duration_since(prev_at) <= DOUBLE_CLICK_WINDOW
        );
        if is_double_click {
            // Consume the click pair so a third click doesn't re-fire.
            self.ui_state.modal_list_last_click = None;
            self.activate_modal_list_selection().await;
        } else {
            self.ui_state.modal_list_last_click = Some((idx, now));
        }
    }

    /// Activate the highlighted row of the open list modal — the shared
    /// endpoint for Enter and double-click.
    async fn activate_modal_list_selection(&mut self) {
        match &self.ui_state.modal {
            Modal::QuickSwitch { .. } => self.activate_quick_switch_selection().await,
            Modal::CheckoutBranch { .. } => self.activate_checkout_selection().await,
            Modal::PathInput { .. } => self.submit_path_input().await,
            _ => {}
        }
    }

    /// Activate the highlighted quick-switch row: jump to the session, run
    /// the command, or apply the section move.
    async fn activate_quick_switch_selection(&mut self) {
        // Clone the selected item so the borrow on `matches` is released
        // before we mutate `modal` and dispatch.
        let selected = match &self.ui_state.modal {
            Modal::QuickSwitch {
                matches,
                selected_idx,
                ..
            } => matches.get(*selected_idx).cloned(),
            _ => return,
        };
        match selected {
            Some(QuickSwitchItem::Session(m)) => {
                let session_id = m.session_id;
                self.ui_state.modal = Modal::None;
                self.ui_state.selected_session_id = Some(SessionRef::new(
                    self.backend_of_session(session_id),
                    session_id,
                ));
                if let Some(idx) = self.ui_state.list_items.iter().position(|item| {
                    matches!(item, SessionListItem::Worktree { id, .. } if *id == session_id)
                }) {
                    self.ui_state.list_state.select(Some(idx));
                }
                self.update_selection();
                self.handle_select().await;
            }
            Some(QuickSwitchItem::Command(entry)) => {
                self.ui_state.modal = Modal::None;
                self.handle_command(entry.action.into()).await;
            }
            Some(QuickSwitchItem::SectionMove {
                session_id, target, ..
            }) => {
                self.ui_state.modal = Modal::None;
                self.apply_section_move(session_id, target).await;
            }
            Some(QuickSwitchItem::RemoteServerRemove { name, .. }) => {
                self.ui_state.modal = Modal::Confirm {
                    title: "Remove Remote Server".to_string(),
                    message: format!(
                        "Remove remote server \"{name}\"?\n\nSessions keep running on the server; this only removes it from this TUI's config."
                    ),
                    on_confirm: ConfirmAction::RemoveRemoteServer { name },
                };
            }
            None => {}
        }
    }

    /// Check out the branch the checkout modal currently points at: the
    /// highlighted match when the filter produced any, otherwise the raw
    /// query text (so a pasted branch name still works — a leading
    /// "origin/" is stripped to always get a local branch name).
    async fn activate_checkout_selection(&mut self) {
        let (project_id, branch_label) = match &self.ui_state.modal {
            Modal::CheckoutBranch {
                project_id,
                query,
                filtered,
                selected_idx,
                ..
            } => {
                let label = if let Some(m) = filtered.get(*selected_idx) {
                    m.local_name.clone()
                } else {
                    let trimmed = query.value().trim();
                    if trimmed.is_empty() {
                        return;
                    }
                    trimmed
                        .strip_prefix("origin/")
                        .unwrap_or(trimmed)
                        .to_string()
                };
                (*project_id, label)
            }
            _ => return,
        };
        self.ui_state.modal = Modal::None;
        self.start_checkout_session(project_id, branch_label).await;
    }

    /// Submit the path-input modal, preferring the highlighted completion
    /// over the typed value (so arrow-to-select-then-Enter works without
    /// first pressing Tab) and falling back to the typed value when the
    /// list is empty (e.g. a path that doesn't exist yet).
    async fn submit_path_input(&mut self) {
        let (action, submit_value) = match &self.ui_state.modal {
            Modal::PathInput {
                value,
                on_submit,
                completer,
                ..
            } => (
                on_submit.clone(),
                completer
                    .selected_completion()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.value().to_string()),
            ),
            _ => return,
        };
        self.ui_state.modal = Modal::None;
        self.handle_input_submit(action, submit_value, None).await;
    }

    /// Mouse wheel while a (non-review) modal is open. List modals move the
    /// highlighted row, clamping at the ends; the Help modal scrolls its
    /// content. Other modals swallow the event so the panes underneath
    /// don't scroll while covered.
    fn modal_wheel(&mut self, down: bool) {
        use super::actions::{LIST_MAX_VISIBLE, adjust_list_scroll, wheel_step};
        match &mut self.ui_state.modal {
            Modal::QuickSwitch {
                matches,
                selected_idx,
                scroll,
                ..
            } if !matches.is_empty() => {
                *selected_idx = wheel_step(*selected_idx, down, matches.len());
                *scroll = adjust_list_scroll(*selected_idx, *scroll, LIST_MAX_VISIBLE);
            }
            Modal::CheckoutBranch {
                filtered,
                selected_idx,
                scroll,
                ..
            } if !filtered.is_empty() => {
                *selected_idx = wheel_step(*selected_idx, down, filtered.len());
                *scroll = adjust_list_scroll(*selected_idx, *scroll, LIST_MAX_VISIBLE);
            }
            Modal::PathInput {
                completer, scroll, ..
            } => {
                let (list, highlighted) = completer.visible_completions();
                if let Some(idx) = highlighted {
                    let new_idx = wheel_step(idx, down, list.len());
                    completer.select(new_idx);
                    *scroll = adjust_list_scroll(new_idx, *scroll, LIST_MAX_VISIBLE);
                }
            }
            Modal::Help { scroll } => {
                *scroll = scroll.saturating_add_signed(if down { 1 } else { -1 });
            }
            _ => {}
        }
    }

    /// Handle a user command
    pub(super) async fn handle_command(&mut self, cmd: UserCommand) {
        // Single dispatch chokepoint: record UI-level feature usage here.
        // Commands handled by an instrumented service method (and pure
        // navigation noise) map to `None` — see `UserCommand::telemetry_feature`.
        if let Some(feature) = cmd.telemetry_feature() {
            self.record_feature(feature);
        }
        match cmd {
            UserCommand::NavigateUp => {
                self.ui_state.list_state.previous();
            }
            UserCommand::NavigateDown => {
                self.ui_state.list_state.next();
            }
            UserCommand::NextGroup => {
                self.ui_state.list_state.next_group();
            }
            UserCommand::PreviousGroup => {
                self.ui_state.list_state.previous_group();
            }
            UserCommand::NavigateFirst => {
                self.ui_state.list_state.select_first();
            }
            UserCommand::NavigateLast => {
                self.ui_state.list_state.select_last();
            }
            UserCommand::Select => {
                if self.selected_item_is_section_header() {
                    self.handle_toggle_section().await;
                } else {
                    self.handle_select().await;
                }
            }
            UserCommand::SelectShell => {
                self.handle_select_shell().await;
            }
            UserCommand::NewSession => {
                self.handle_new_session().await;
            }
            UserCommand::NewStackedSession => {
                self.handle_new_stacked_session().await;
            }
            UserCommand::CascadeMergeMain => {
                self.handle_cascade_merge_main().await;
            }
            UserCommand::CascadeResume => {
                self.handle_cascade_resume().await;
            }
            UserCommand::CascadeAbandon => {
                self.handle_cascade_abandon().await;
            }
            UserCommand::PushStack => {
                self.handle_push_stack();
            }
            UserCommand::CheckoutBranch => {
                self.handle_checkout_branch().await;
            }
            UserCommand::NewProject => {
                self.open_path_input(
                    "Add Project".to_string(),
                    "Enter path to git repository:".to_string(),
                    InputAction::AddProject,
                );
            }
            UserCommand::ScanDirectory => {
                self.open_path_input(
                    "Scan Directory".to_string(),
                    "Enter directory to scan for git repos:".to_string(),
                    InputAction::ScanDirectory,
                );
            }
            UserCommand::DeleteSession => {
                self.handle_delete_session().await;
            }
            UserCommand::DeleteMergedPrSessions => {
                self.handle_delete_merged_pr_sessions().await;
            }
            UserCommand::RenameSession => {
                self.handle_rename_session().await;
            }
            UserCommand::MoveToSection => {
                self.handle_move_to_section().await;
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
            UserCommand::RefreshPrStatus => {
                // Wake the service's PR-status loop; refreshed results arrive via
                // the backend change feed.
                let _ = self.local_arc().request_pr_refresh().await;
            }
            UserCommand::AddRemoteServer => {
                self.handle_add_remote_server();
            }
            UserCommand::RemoveRemoteServer => {
                self.handle_remove_remote_server();
            }
            UserCommand::OpenCommander => {
                self.handle_open_commander().await;
            }
            UserCommand::ToggleConversationOverlay => {
                self.toggle_conversation_overlay().await;
            }
            UserCommand::ToggleVoiceInput => {
                self.toggle_voice_input().await;
            }
            UserCommand::OpenReviewDiff => {
                self.handle_open_review().await;
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
                self.ui_state.modal = Modal::Help { scroll: 0 };
            }
            UserCommand::ShowSettings => {
                let rows = self.build_settings_rows(SettingsTab::General);
                let selected_row = super::settings::first_selectable_from(&rows, 0);
                self.ui_state.modal = Modal::Settings(SettingsState {
                    tab: SettingsTab::General,
                    selected_row,
                    editing: None,
                    rows,
                    sections_state: SectionsState::default(),
                    search: None,
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
                    && let Some(session_id) = self.ui_state.selected_session_id.map(|r| r.id)
                {
                    self.spawn_ai_summary_if_needed(session_id);
                }
            }
            UserCommand::ToggleSection => {
                self.handle_toggle_section().await;
            }
            UserCommand::ToggleViewMode => {
                if self.config.sections.is_empty() {
                    return;
                }
                let new_view = self.ui_state.view_mode.next();
                self.ui_state.view_mode = new_view;
                // Persist the chosen view so it survives restarts. A failed
                // write is logged (not surfaced) inside the prefs store — the
                // runtime behaviour is correct either way.
                self.tui_prefs.set_view_mode(new_view).await;
                let selected_session = self.ui_state.selected_session_id.map(|r| r.id);
                let selected_project = self.ui_state.selected_project_id.map(|(_, p)| p);
                self.refresh_list_items().await;
                // Restore selection after rebuilding the list
                if let Some(sid) = selected_session {
                    if let Some(idx) = self.ui_state.list_items.iter().position(
                        |item| matches!(item, SessionListItem::Worktree { id, .. } if *id == sid),
                    ) {
                        self.ui_state.list_state.select(Some(idx));
                    }
                } else if let Some(pid) = selected_project
                    && let Some(idx) = self.ui_state.list_items.iter().position(
                        |item| matches!(item, SessionListItem::Project { id, .. } if *id == pid),
                    )
                {
                    self.ui_state.list_state.select(Some(idx));
                }
                self.update_selection();
                self.spawn_preview_update();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeyBindings;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn arrows_scroll_one_line() {
        let kb = KeyBindings::default();
        assert_eq!(
            classify_help_key(&key(KeyCode::Down), &kb),
            HelpKey::ScrollBy(1)
        );
        assert_eq!(
            classify_help_key(&key(KeyCode::Up), &kb),
            HelpKey::ScrollBy(-1)
        );
    }

    #[test]
    fn default_jk_bindings_scroll_one_line() {
        // Default KeyBindings bind j/k to NavigateDown/NavigateUp, not scroll,
        // so plain j/k should NOT produce ScrollBy here — they are ignored in
        // the Help modal. This pins the current default so a future remapping
        // doesn't silently change modal behavior.
        let kb = KeyBindings::default();
        assert_eq!(
            classify_help_key(&key(KeyCode::Char('j')), &kb),
            HelpKey::Ignore
        );
        assert_eq!(
            classify_help_key(&key(KeyCode::Char('k')), &kb),
            HelpKey::Ignore
        );
    }

    #[test]
    fn page_keys_scroll_by_page() {
        let kb = KeyBindings::default();
        let page = HELP_PAGE as i16;
        assert_eq!(
            classify_help_key(&key(KeyCode::PageDown), &kb),
            HelpKey::ScrollBy(page)
        );
        assert_eq!(
            classify_help_key(&key(KeyCode::PageUp), &kb),
            HelpKey::ScrollBy(-page)
        );
        // Default bindings: Ctrl-d / Ctrl-u for PageDown / PageUp.
        assert_eq!(
            classify_help_key(&ctrl(KeyCode::Char('d')), &kb),
            HelpKey::ScrollBy(page)
        );
        assert_eq!(
            classify_help_key(&ctrl(KeyCode::Char('u')), &kb),
            HelpKey::ScrollBy(-page)
        );
    }

    #[test]
    fn home_and_end_jump() {
        let kb = KeyBindings::default();
        assert_eq!(classify_help_key(&key(KeyCode::Home), &kb), HelpKey::Home);
        assert_eq!(classify_help_key(&key(KeyCode::End), &kb), HelpKey::End);
    }

    #[test]
    fn close_keys() {
        let kb = KeyBindings::default();
        for code in [
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Char('q'),
            KeyCode::Char('?'),
        ] {
            assert_eq!(
                classify_help_key(&key(code), &kb),
                HelpKey::Close,
                "{code:?}"
            );
        }
    }

    #[test]
    fn unrelated_key_is_ignored() {
        let kb = KeyBindings::default();
        assert_eq!(
            classify_help_key(&key(KeyCode::Char('x')), &kb),
            HelpKey::Ignore
        );
    }

    #[test]
    fn palette_text_char_accepts_plain_letters_including_jk() {
        for c in ['j', 'k', 'a', 'z', ' ', '1', '?'] {
            assert_eq!(
                palette_text_char(&key(KeyCode::Char(c))),
                Some(c),
                "plain {c:?}"
            );
        }
    }

    #[test]
    fn palette_text_char_accepts_shifted_letters() {
        // Kitty-style: Char('K') with SHIFT.
        assert_eq!(
            palette_text_char(&KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)),
            Some('K')
        );
        // Non-kitty: Char('J') with no modifier.
        assert_eq!(palette_text_char(&key(KeyCode::Char('J'))), Some('J'));
    }

    #[test]
    fn palette_text_char_rejects_modifier_combos() {
        assert_eq!(palette_text_char(&ctrl(KeyCode::Char('p'))), None);
        assert_eq!(palette_text_char(&ctrl(KeyCode::Char('n'))), None);
        assert_eq!(
            palette_text_char(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT)),
            None
        );
    }

    #[test]
    fn palette_text_char_rejects_non_char_keys() {
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Tab,
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Backspace,
        ] {
            assert_eq!(palette_text_char(&key(code)), None, "{code:?}");
        }
    }

    // -----------------------------------------------------------------------
    // apply_paste_to_modal: routes clipboard text into the open modal's
    // text field and reports whether the caller still needs to recompute
    // a filtered list.
    // -----------------------------------------------------------------------

    use crate::session::ProjectId;
    use crate::tui::app::{InputAction, Modal, PaletteMode};
    use crate::tui::path_completer::PathCompleter;

    fn checkout_modal(query: &str) -> Modal {
        Modal::CheckoutBranch {
            project_id: ProjectId::new(),
            query: query.into(),
            all_branches: Vec::new(),
            filtered: Vec::new(),
            selected_idx: 0,
            scroll: 0,
            fetching: false,
        }
    }

    fn quick_switch_modal(query: &str) -> Modal {
        Modal::QuickSwitch {
            mode: PaletteMode::Unified,
            query: query.into(),
            matches: Vec::new(),
            selected_idx: 0,
            scroll: 0,
        }
    }

    fn input_modal(value: &str) -> Modal {
        Modal::Input {
            title: String::new(),
            prompt: String::new(),
            value: value.into(),
            on_submit: InputAction::AddProject,
            existing_branches: None,
            project_picker: None,
            program_picker: None,
            focus: crate::tui::app::InputFocus::Name,
            expanded: false,
            mask: false,
        }
    }

    #[test]
    fn paste_into_checkout_branch_appends_and_requests_refilter() {
        // Regression: paste was silently dropped because the CheckoutBranch
        // arm was missing from the InputEvent::Paste match.
        let mut modal = checkout_modal("");
        let refilter = apply_paste_to_modal(&mut modal, "feature-foo");
        assert_eq!(refilter, Some(PasteRefilter::CheckoutBranch));
        match modal {
            Modal::CheckoutBranch { query, .. } => assert_eq!(query.value(), "feature-foo"),
            _ => panic!("modal variant changed"),
        }
    }

    #[test]
    fn paste_into_checkout_branch_appends_to_existing_query() {
        let mut modal = checkout_modal("feat-");
        apply_paste_to_modal(&mut modal, "bar");
        match modal {
            Modal::CheckoutBranch { query, .. } => assert_eq!(query.value(), "feat-bar"),
            _ => panic!("modal variant changed"),
        }
    }

    #[test]
    fn paste_into_checkout_branch_strips_newlines() {
        // A multi-line paste must not contain \n / \r — Enter handling would
        // otherwise submit prematurely if the input handler ever forwarded
        // newlines as KeyCode::Enter.
        let mut modal = checkout_modal("");
        apply_paste_to_modal(&mut modal, "feature-foo\nfeature-bar\r\n");
        match modal {
            Modal::CheckoutBranch { query, .. } => {
                assert_eq!(query.value(), "feature-foofeature-bar");
            }
            _ => panic!("modal variant changed"),
        }
    }

    #[test]
    fn paste_into_quick_switch_appends_and_requests_refilter() {
        let mut modal = quick_switch_modal("");
        let refilter = apply_paste_to_modal(&mut modal, "hello");
        assert_eq!(refilter, Some(PasteRefilter::QuickSwitch));
        match modal {
            Modal::QuickSwitch { query, .. } => assert_eq!(query.value(), "hello"),
            _ => panic!("modal variant changed"),
        }
    }

    fn review_modal_with_open_draft() -> Modal {
        use crate::git::parse_unified_diff;
        use crate::session::SessionId;
        use crate::tui::app::DiffReviewState;
        let diff = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,3 @@
 fn main() {
+    let y = 3;
 }
",
        );
        let mut state = DiffReviewState::new(
            SessionId::new(),
            "test".to_string(),
            "main".to_string(),
            diff,
            Vec::new(),
        );
        state.begin_comment();
        Modal::ReviewDiff(Box::new(state))
    }

    #[test]
    fn paste_into_review_comment_draft_appends_without_refilter() {
        // Regression: paste in the review view fell into the `_ => None`
        // arm and was dropped even with the comment box open.
        let mut modal = review_modal_with_open_draft();
        let refilter = apply_paste_to_modal(&mut modal, "use a helper");
        assert_eq!(refilter, None);
        match modal {
            Modal::ReviewDiff(state) => {
                assert_eq!(
                    state.comment.as_ref().unwrap().input.value(),
                    "use a helper"
                );
            }
            _ => panic!("modal variant changed"),
        }
    }

    #[test]
    fn paste_into_input_appends_without_refilter() {
        // Modal::Input has no filtered list — no refilter is requested.
        let mut modal = input_modal("foo");
        let refilter = apply_paste_to_modal(&mut modal, "bar");
        assert_eq!(refilter, None);
        match modal {
            Modal::Input { value, .. } => assert_eq!(value.value(), "foobar"),
            _ => panic!("modal variant changed"),
        }
    }

    #[test]
    fn paste_into_path_input_appends_and_refilters_inline() {
        // PathInput owns its completer, so it can refilter inline without
        // the caller's help — `None` is returned.
        let mut modal = Modal::PathInput {
            title: String::new(),
            prompt: String::new(),
            value: "/tm".into(),
            on_submit: InputAction::AddProject,
            completer: PathCompleter::new(),
            scroll: 7,
        };
        let refilter = apply_paste_to_modal(&mut modal, "p");
        assert_eq!(refilter, None);
        match modal {
            Modal::PathInput { value, scroll, .. } => {
                assert_eq!(value.value(), "/tmp");
                assert_eq!(scroll, 0, "scroll resets on input change");
            }
            _ => panic!("modal variant changed"),
        }
    }

    #[test]
    fn paste_into_no_modal_is_noop() {
        let mut modal = Modal::None;
        assert_eq!(apply_paste_to_modal(&mut modal, "hello"), None);
        assert!(matches!(modal, Modal::None));
    }

    #[test]
    fn paste_into_unhandled_modal_is_noop() {
        // Help/Error/Confirm/Loading/Settings have no text input — paste is
        // intentionally ignored. Spot-check one to pin the behavior.
        let mut modal = Modal::Help { scroll: 0 };
        assert_eq!(apply_paste_to_modal(&mut modal, "hello"), None);
        assert!(matches!(modal, Modal::Help { scroll: 0 }));
    }

    // -----------------------------------------------------------------------
    // handle_input_modal_key: pure routing for the New Session dialog —
    // collapsed field focus, dropdown expand/collapse, filtering, submit.
    // -----------------------------------------------------------------------

    use crate::config::ProgramEntry;
    use crate::tui::app::{InputFocus, ProgramPicker, ProjectChoice, ProjectPicker};

    fn project_fixture(names: &[&str], selected: usize) -> ProjectPicker {
        let choices: Vec<ProjectChoice> = names
            .iter()
            .map(|n| ProjectChoice {
                id: ProjectId::new(),
                name: n.to_string(),
                repo_path: std::path::PathBuf::from(format!("/repos/{n}")),
            })
            .collect();
        let id = choices[selected].id;
        ProjectPicker::new(choices, id)
    }

    fn program_fixture(cmds: &[&str], selected: usize) -> ProgramPicker {
        ProgramPicker {
            choices: cmds
                .iter()
                .map(|c| ProgramEntry {
                    label: c.to_string(),
                    command: c.to_string(),
                })
                .collect(),
            selected,
        }
    }

    fn session_modal(project: Option<ProjectPicker>, program: Option<ProgramPicker>) -> Modal {
        Modal::Input {
            title: String::new(),
            prompt: String::new(),
            value: "".into(),
            on_submit: InputAction::AddProject,
            existing_branches: None,
            project_picker: project,
            program_picker: program,
            focus: InputFocus::Name,
            expanded: false,
            mask: false,
        }
    }

    fn focus_of(m: &Modal) -> InputFocus {
        match m {
            Modal::Input { focus, .. } => *focus,
            _ => panic!("not an Input modal"),
        }
    }

    fn expanded_of(m: &Modal) -> bool {
        match m {
            Modal::Input { expanded, .. } => *expanded,
            _ => panic!("not an Input modal"),
        }
    }

    #[test]
    fn collapsed_enter_submits_and_esc_cancels() {
        let mut m = session_modal(
            Some(project_fixture(&["a", "b"], 0)),
            Some(program_fixture(&["claude"], 0)),
        );
        assert_eq!(
            handle_input_modal_key(&mut m, key(KeyCode::Enter)),
            InputKeyOutcome::Submit
        );
        assert_eq!(
            handle_input_modal_key(&mut m, key(KeyCode::Esc)),
            InputKeyOutcome::Cancel
        );
    }

    #[test]
    fn arrows_move_focus_between_present_rows() {
        let mut m = session_modal(
            Some(project_fixture(&["a"], 0)),
            Some(program_fixture(&["claude"], 0)),
        );
        handle_input_modal_key(&mut m, key(KeyCode::Down));
        assert_eq!(focus_of(&m), InputFocus::Project);
        handle_input_modal_key(&mut m, key(KeyCode::Down));
        assert_eq!(focus_of(&m), InputFocus::Program);
    }

    #[test]
    fn name_row_edits_text_and_stays_collapsed() {
        let mut m = session_modal(Some(project_fixture(&["a"], 0)), None);
        assert_eq!(
            handle_input_modal_key(&mut m, key(KeyCode::Char('x'))),
            InputKeyOutcome::Handled
        );
        match &m {
            Modal::Input {
                value,
                focus,
                expanded,
                ..
            } => {
                assert_eq!(value.value(), "x");
                assert_eq!(*focus, InputFocus::Name);
                assert!(!expanded);
            }
            _ => panic!("not an Input modal"),
        }
    }

    #[test]
    fn space_opens_project_dropdown() {
        let mut m = session_modal(Some(project_fixture(&["a", "b"], 0)), None);
        handle_input_modal_key(&mut m, key(KeyCode::Down)); // focus Project
        assert_eq!(focus_of(&m), InputFocus::Project);
        assert_eq!(
            handle_input_modal_key(&mut m, key(KeyCode::Char(' '))),
            InputKeyOutcome::Handled
        );
        assert!(expanded_of(&m));
    }

    #[test]
    fn typing_on_project_row_opens_and_filters() {
        let mut m = session_modal(Some(project_fixture(&["alpha", "beta"], 0)), None);
        handle_input_modal_key(&mut m, key(KeyCode::Down)); // focus Project
        handle_input_modal_key(&mut m, key(KeyCode::Char('b')));
        match &m {
            Modal::Input {
                expanded,
                project_picker: Some(p),
                ..
            } => {
                assert!(expanded);
                assert_eq!(p.filter, "b");
                assert_eq!(p.filtered.len(), 1); // only "beta" matches
            }
            _ => panic!("not an Input modal with project picker"),
        }
    }

    #[test]
    fn dropdown_navigation_then_enter_confirms_and_collapses() {
        let mut m = session_modal(Some(project_fixture(&["alpha", "beta", "gamma"], 0)), None);
        handle_input_modal_key(&mut m, key(KeyCode::Down)); // focus Project
        handle_input_modal_key(&mut m, key(KeyCode::Char(' '))); // open
        handle_input_modal_key(&mut m, key(KeyCode::Down)); // move to index 1
        assert_eq!(
            handle_input_modal_key(&mut m, key(KeyCode::Enter)),
            InputKeyOutcome::Handled
        );
        assert!(!expanded_of(&m));
        match &m {
            Modal::Input {
                project_picker: Some(p),
                ..
            } => assert_eq!(p.selected, 1),
            _ => panic!("not an Input modal with project picker"),
        }
    }

    #[test]
    fn dropdown_esc_collapses_without_cancelling() {
        let mut m = session_modal(Some(project_fixture(&["a", "b"], 0)), None);
        handle_input_modal_key(&mut m, key(KeyCode::Down));
        handle_input_modal_key(&mut m, key(KeyCode::Char(' ')));
        assert!(expanded_of(&m));
        assert_eq!(
            handle_input_modal_key(&mut m, key(KeyCode::Esc)),
            InputKeyOutcome::Handled
        );
        assert!(!expanded_of(&m));
    }

    #[test]
    fn enter_with_no_matching_project_reopens_dropdown_instead_of_submitting() {
        let mut m = session_modal(Some(project_fixture(&["alpha", "beta"], 0)), None);
        // Filter to nothing so the picker has no selectable project.
        match &mut m {
            Modal::Input {
                project_picker: Some(p),
                ..
            } => {
                p.filter = "zzz".to_string();
                p.apply_filter();
                assert!(p.selected_id().is_none());
            }
            _ => panic!("not an Input modal with project picker"),
        }
        // Enter must not submit; it reopens the Project dropdown so the empty
        // result is visible.
        assert_eq!(
            handle_input_modal_key(&mut m, key(KeyCode::Enter)),
            InputKeyOutcome::Handled
        );
        assert_eq!(focus_of(&m), InputFocus::Project);
        assert!(expanded_of(&m));
    }

    #[test]
    fn enter_submits_from_a_collapsed_picker_row_with_a_valid_selection() {
        let mut m = session_modal(
            Some(project_fixture(&["alpha"], 0)),
            Some(program_fixture(&["claude"], 0)),
        );
        // Name → Project → Program, all collapsed, valid selections.
        handle_input_modal_key(&mut m, key(KeyCode::Down));
        handle_input_modal_key(&mut m, key(KeyCode::Down));
        assert_eq!(focus_of(&m), InputFocus::Program);
        assert!(!expanded_of(&m));
        assert_eq!(
            handle_input_modal_key(&mut m, key(KeyCode::Enter)),
            InputKeyOutcome::Submit
        );
    }

    #[test]
    fn program_only_modal_skips_project_and_selects() {
        let mut m = session_modal(None, Some(program_fixture(&["claude", "codex"], 0)));
        handle_input_modal_key(&mut m, key(KeyCode::Down)); // Name → Program (no project)
        assert_eq!(focus_of(&m), InputFocus::Program);
        handle_input_modal_key(&mut m, key(KeyCode::Char(' '))); // open
        assert!(expanded_of(&m));
        handle_input_modal_key(&mut m, key(KeyCode::Down)); // select index 1
        handle_input_modal_key(&mut m, key(KeyCode::Enter)); // confirm
        assert!(!expanded_of(&m));
        match &m {
            Modal::Input {
                program_picker: Some(p),
                ..
            } => assert_eq!(p.selected, 1),
            _ => panic!("not an Input modal with program picker"),
        }
    }
}
