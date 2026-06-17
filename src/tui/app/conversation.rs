//! TUI side of conversation mode: owns the long-lived headless `claude` session
//! and the streaming-TTS speaker, tracks the on-screen history, and renders the
//! full-screen overlay. The session lives on `App` (not the modal), so it keeps
//! running — and keeps speaking — while the overlay is closed.

use super::*;
use crate::conversation::{ConversationEvent, ConversationSession, SpeakerCommand, spawn_speaker};
use crate::tui::event::{AppEvent, StateUpdate};

/// Canonical project spinner frames (advanced every 3 render ticks).
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Preamble seeded into the conversation session's `CLAUDE.md` (`{name}` is
/// replaced with the configured assistant name). Followed by the generated
/// `claude-commander` CLI reference so the agent can inspect live session/
/// project state. Tuned for spoken replies.
const CONVERSATION_PRIME: &str = "\
# {name}

You are {name}, a voice assistant for Claude Commander (the `claude-commander`
CLI), which manages the user's Claude coding sessions across their projects. The
user is *talking* to you, and your replies are read aloud by text-to-speech.

## Tone
- Be concise and conversational — a sentence or two, not a wall of text.
- Don't read out code, long file paths, or long lists verbatim; summarise them.
- Avoid markdown tables and code blocks; they sound bad spoken.

## Checking current state
Don't guess about the user's sessions or projects — inspect the live state with
the CLI (it needs no approval). Good first commands:
- `claude-commander list` — all current sessions, their projects and status.
- `claude-commander status <name>` — detail on one session.
- `claude-commander log <name>` — recent output from a session.
Run `claude-commander list` early when the user asks anything about what's
going on. You can read anything on the filesystem the user can.

## Working directory
This directory is your own scratch space; nothing else in it matters.
";

/// Who authored a conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvRole {
    User,
    Assistant,
}

/// A finalized turn in the conversation history.
#[derive(Debug, Clone)]
pub struct ConvMessage {
    pub role: ConvRole,
    pub text: String,
}

/// Lifecycle status of the conversation, shown in the overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConvStatus {
    #[default]
    Idle,
    Thinking,
    Error(String),
}

/// Conversation runtime state held on `App`, independent of the overlay's
/// visibility.
#[derive(Default)]
pub struct ConversationRuntime {
    /// The headless `claude` subprocess; `None` until first opened.
    pub session: Option<ConversationSession>,
    /// Streaming-TTS speaker command channel; `None` if TTS is off/unavailable.
    pub speaker: Option<tokio::sync::mpsc::UnboundedSender<SpeakerCommand>>,
    /// Finalized turns.
    pub messages: Vec<ConvMessage>,
    /// In-progress assistant text (streaming).
    pub streaming: String,
    pub status: ConvStatus,
    pub session_id: Option<String>,
    /// User messages sent while a turn was still in progress. The session
    /// queues them and answers them in order; we defer *displaying* each until
    /// the preceding reply completes, so history stays correctly ordered.
    pub pending_user: std::collections::VecDeque<String>,
}

impl ConversationRuntime {
    fn speak(&self, cmd: SpeakerCommand) {
        if let Some(tx) = &self.speaker {
            let _ = tx.send(cmd);
        }
    }
}

impl App {
    /// Alt-c: open the overlay (starting the session on first use) or close it
    /// (leaving the session running). The single `conversation.enabled` setting
    /// gates the whole feature — when off, the overlay doesn't open at all.
    pub(super) async fn toggle_conversation_overlay(&mut self) {
        if matches!(self.ui_state.modal, Modal::Conversation { .. }) {
            self.ui_state.modal = Modal::None;
            return;
        }
        if !self.config.conversation.enabled {
            self.ui_state.status_message = Some((
                "Conversation mode is disabled — enable it in Settings ▸ Conversation".to_string(),
                std::time::Instant::now() + std::time::Duration::from_secs(4),
            ));
            return;
        }
        self.ensure_conversation_started().await;
        self.ui_state.modal = Modal::Conversation {
            input: Input::default(),
            scroll: 0,
        };
    }

    /// Spawn the headless streaming `claude` session (and the TTS speaker) once.
    async fn ensure_conversation_started(&mut self) {
        if self.conversation.session.is_some() {
            return;
        }
        let dir = match Config::data_dir() {
            Ok(d) => d.join("conversation"),
            Err(e) => {
                self.conversation.status = ConvStatus::Error(format!("no data dir: {e}"));
                return;
            }
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.conversation.status = ConvStatus::Error(format!("mkdir failed: {e}"));
            return;
        }

        // Seed CLAUDE.md so the agent knows it's a (spoken) Claude Commander
        // assistant and how to inspect live session/project state. Rewritten on
        // each (re)spawn so the embedded CLI reference stays current.
        let cli = crate::cli_args::cli_command();
        let prime = CONVERSATION_PRIME.replace("{name}", &self.config.conversation.name);
        let claude_md = format!(
            "{}\n{}",
            prime.trim_end(),
            crate::commander::generate_cli_reference(&cli)
        );
        if let Err(e) = std::fs::write(dir.join("CLAUDE.md"), claude_md) {
            warn!("conversation: failed to write CLAUDE.md: {e}");
        }

        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<ConversationEvent>();
        let command = self.config.conversation.command.clone();
        let permission_mode = self.config.conversation.permission_mode.clone();
        match ConversationSession::spawn(&command, &permission_mode, &dir, ev_tx) {
            Ok(session) => self.conversation.session = Some(session),
            Err(e) => {
                self.conversation.status = ConvStatus::Error(e.to_string());
                return;
            }
        }

        // Bridge the session's events onto the main AppEvent loop, so history
        // updates (and TTS) keep flowing even while the overlay is closed.
        let app_tx = self.event_loop.sender();
        tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                if app_tx
                    .send(AppEvent::StateUpdate(StateUpdate::Conversation(ev)))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Start the streaming-TTS speaker if enabled. Failure (e.g. no audio
        // device) is non-fatal: the conversation still works, just silent.
        if self.config.conversation.enabled {
            match spawn_speaker(self.config.conversation.clone()) {
                Ok(tx) => self.conversation.speaker = Some(tx),
                Err(e) => warn!("conversation TTS unavailable: {e}"),
            }
        }
        self.conversation.status = ConvStatus::Idle;
    }

    /// Handle one parsed conversation event (called from the StateUpdate loop,
    /// regardless of whether the overlay is open).
    pub(super) fn on_conversation_event(&mut self, ev: ConversationEvent) {
        let overlay_open = matches!(self.ui_state.modal, Modal::Conversation { .. });
        match ev {
            ConversationEvent::Started { session_id } => {
                tracing::debug!(%session_id, overlay_open, "conversation: started");
                self.conversation.session_id = Some(session_id);
                if self.conversation.status != ConvStatus::Thinking {
                    self.conversation.status = ConvStatus::Idle;
                }
            }
            ConversationEvent::Delta(text) => {
                self.conversation.streaming.push_str(&text);
                self.conversation.speak(SpeakerCommand::Chunk(text));
            }
            ConversationEvent::Break => {
                // A new text segment: separate it from the previous one (which
                // streamed with no trailing separator), and speak the pending
                // sentence before the gap.
                let s = &mut self.conversation.streaming;
                if !s.is_empty() && !s.ends_with(char::is_whitespace) {
                    s.push_str("\n\n");
                }
                self.conversation.speak(SpeakerCommand::Flush);
            }
            ConversationEvent::TurnComplete => {
                tracing::debug!(overlay_open, "conversation: turn complete");
                self.finalize_streaming();
                self.conversation.speak(SpeakerCommand::Flush);
                // If the user queued a message while this reply was streaming,
                // show it now (after the reply) and stay Thinking — the session
                // is already answering it next.
                if let Some(next) = self.conversation.pending_user.pop_front() {
                    self.conversation.messages.push(ConvMessage {
                        role: ConvRole::User,
                        text: next,
                    });
                    self.conversation.status = ConvStatus::Thinking;
                } else {
                    self.conversation.status = ConvStatus::Idle;
                }
            }
            ConversationEvent::Error(e) => {
                tracing::warn!(overlay_open, "conversation: turn error: {e}");
                self.finalize_streaming();
                self.conversation.speak(SpeakerCommand::Flush);
                self.conversation.status = ConvStatus::Error(e);
            }
            ConversationEvent::Exited => {
                tracing::debug!(overlay_open, "conversation: session exited");
                self.finalize_streaming();
                // Drop the dead handle; the next message respawns it.
                self.conversation.session = None;
                self.conversation.status = ConvStatus::Error("session ended".to_string());
            }
        }
    }

    fn finalize_streaming(&mut self) {
        let text = std::mem::take(&mut self.conversation.streaming);
        let text = text.trim();
        if !text.is_empty() {
            self.conversation.messages.push(ConvMessage {
                role: ConvRole::Assistant,
                text: text.to_string(),
            });
        }
    }

    /// Send a typed user turn to the session.
    ///
    /// The session serializes turns: a message sent while a reply is still
    /// streaming is queued and answered after. So we never interrupt or discard
    /// the in-flight reply — we send the new message (the session queues it) and,
    /// if a turn is in progress, defer displaying it until that reply completes.
    pub(super) async fn submit_conversation_input(&mut self, text: String) {
        // Self-heal: if the session exited (idle timeout / crash), bring it back.
        self.ensure_conversation_started().await;
        let Some(session) = self.conversation.session.as_mut() else {
            self.conversation.status = ConvStatus::Error("session not running".to_string());
            return;
        };
        if let Err(e) = session.send_user_message(&text).await {
            tracing::warn!("conversation: send failed: {e}");
            self.conversation.status = ConvStatus::Error(e.to_string());
            return;
        }

        if self.conversation.status == ConvStatus::Thinking {
            // A reply is still in progress — queue this for display until it ends.
            self.conversation.pending_user.push_back(text);
        } else {
            self.conversation.messages.push(ConvMessage {
                role: ConvRole::User,
                text,
            });
            self.conversation.status = ConvStatus::Thinking;
        }
    }

    /// Render the full-screen conversation overlay.
    pub(super) fn render_conversation_modal(
        &self,
        frame: &mut Frame,
        area: Rect,
        input: &Input,
        scroll: u16,
    ) {
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(" Conversation ")
            .borders(Borders::ALL)
            .border_type(self.border_type())
            .border_style(Style::default().fg(self.theme.modal_info));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Layout: history (fills), input box (3 rows). No top status line — the
        // feature is TTS by definition, and progress is shown inline at the
        // bottom where the reply appears.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(inner);

        // History: wrap every message to the inner width, bottom-anchored with
        // `scroll` lines paged up.
        let width = chunks[0].width.max(1) as usize;
        let mut lines: Vec<Line> = Vec::new();
        for msg in &self.conversation.messages {
            self.push_message_lines(&mut lines, msg.role, &msg.text, width);
        }
        if !self.conversation.streaming.is_empty() {
            // The reply is arriving — the text itself is the progress indicator.
            self.push_message_lines(
                &mut lines,
                ConvRole::Assistant,
                &self.conversation.streaming,
                width,
            );
        } else {
            // No text yet: show a spinner where the reply will appear, or an
            // error if the last turn failed.
            match &self.conversation.status {
                ConvStatus::Thinking => {
                    let frame_glyph = SPINNER_FRAMES
                        [(self.ui_state.tick_count as usize / 3) % SPINNER_FRAMES.len()];
                    lines.push(Line::from(Span::styled(
                        format!("{frame_glyph} thinking…"),
                        Style::default().fg(self.theme.status_running),
                    )));
                }
                ConvStatus::Error(e) => {
                    lines.push(Line::from(Span::styled(
                        format!("⚠ {e}"),
                        Style::default().fg(self.theme.modal_error),
                    )));
                }
                ConvStatus::Idle => {}
            }
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "Type a message and press Enter. Replies stream in and are spoken aloud.",
                Style::default().fg(self.theme.text_secondary),
            )));
        }
        let view_h = chunks[0].height as usize;
        let total = lines.len();
        let bottom_start = total.saturating_sub(view_h);
        let start = bottom_start.saturating_sub(scroll as usize);
        let end = (start + view_h).min(total);
        let visible: Vec<Line> = lines[start..end].to_vec();
        frame.render_widget(Paragraph::new(visible), chunks[0]);

        // Input box.
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_type(self.border_type())
            .border_style(Style::default().fg(self.theme.border_unfocused));
        let input_inner = input_block.inner(chunks[1]);
        frame.render_widget(input_block, chunks[1]);
        let text_width = input_inner.width.max(1);
        let view_scroll = input.visual_scroll(text_width as usize);
        frame.render_widget(
            Paragraph::new(input.value())
                .scroll((0, view_scroll as u16))
                .style(Style::default().fg(self.theme.text_primary)),
            input_inner,
        );
        // Real cursor in the input field.
        let col = (input.visual_cursor().saturating_sub(view_scroll)) as u16;
        frame.set_cursor_position((
            input_inner.x + col.min(text_width.saturating_sub(1)),
            input_inner.y,
        ));
    }

    /// Append a message's wrapped lines (role header + body) to `lines`.
    fn push_message_lines(
        &self,
        lines: &mut Vec<Line<'static>>,
        role: ConvRole,
        text: &str,
        width: usize,
    ) {
        let (label, color) = match role {
            ConvRole::User => ("You", self.theme.text_accent),
            ConvRole::Assistant => (
                self.config.conversation.name.as_str(),
                self.theme.status_running,
            ),
        };
        lines.push(Line::from(Span::styled(
            format!("{label}:"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));
        for raw in text.split('\n') {
            if raw.is_empty() {
                lines.push(Line::from(String::new()));
                continue;
            }
            for chunk in wrap_text(raw, width) {
                lines.push(Line::from(chunk));
            }
        }
        lines.push(Line::from(String::new())); // blank between messages
    }

    /// Conversation overlay key handling. Returns `true` if the key was consumed.
    pub(super) async fn handle_conversation_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};

        let Modal::Conversation { input, scroll } = &mut self.ui_state.modal else {
            return false;
        };
        match key.code {
            // Esc, or Alt-c again, closes the overlay (session keeps running).
            KeyCode::Esc => {
                self.ui_state.modal = Modal::None;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.ui_state.modal = Modal::None;
            }
            KeyCode::Enter => {
                let text = input.value().trim().to_string();
                *input = Input::default();
                *scroll = 0; // snap back to the live bottom
                if !text.is_empty() {
                    self.submit_conversation_input(text).await;
                }
            }
            KeyCode::PageUp => {
                *scroll = scroll.saturating_add(10);
            }
            KeyCode::PageDown => {
                *scroll = scroll.saturating_sub(10);
            }
            _ => {
                super::edit_text_input(input, key);
            }
        }
        true
    }
}

/// Greedy word-wrap to `width` columns (by char count). Always returns at least
/// one segment for non-empty input.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        // A single word longer than the width is hard-split across rows.
        if word.chars().count() > width {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            for ch in word.chars() {
                if line.chars().count() == width {
                    out.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
            continue;
        }
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::wrap_text;

    #[test]
    fn wrap_basic() {
        assert_eq!(wrap_text("hello world foo", 11), vec!["hello world", "foo"]);
    }

    #[test]
    fn wrap_long_word_hard_splits() {
        let out = wrap_text("abcdefghij", 4);
        assert_eq!(out, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_short_fits_on_one_line() {
        assert_eq!(wrap_text("hi there", 80), vec!["hi there"]);
    }
}
