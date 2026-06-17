//! TUI side of conversation mode.
//!
//! The conversation's content (history, in-flight reply, status) lives in a
//! shared [`ConversationView`] behind an `Arc<Mutex<…>>`. The off-loop bridge
//! task updates that model directly from the session stream (and feeds the
//! speaker), so it advances whether or not the overlay is open and never
//! depends on the UI event loop. The overlay is a *pure view*: it locks the
//! model and renders it. This satisfies the "runs fully headless; the window is
//! just a log + input" requirement.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::*;
use crate::conversation::{ConversationEvent, ConversationSession, spawn_speaker};

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

/// The shared conversation model. Updated off-loop by the bridge task (and by
/// `submit`); read by the renderer. The single source of truth for the overlay.
#[derive(Default)]
pub struct ConversationView {
    /// Finalized turns.
    pub messages: Vec<ConvMessage>,
    /// In-progress assistant text (streaming).
    pub streaming: String,
    pub status: ConvStatus,
    pub session_id: Option<String>,
    /// When the last text delta arrived — drives the "working…" spinner when a
    /// reply stalls (the agent is running tools, producing no text).
    pub last_delta_at: Option<Instant>,
    /// User messages sent while a turn was still in progress. The session queues
    /// them and answers them in order; we defer *displaying* each until the
    /// preceding reply completes, so history stays correctly ordered.
    pub pending_user: std::collections::VecDeque<String>,
}

impl ConversationView {
    /// Apply a session event to the display model (audio is handled separately
    /// by the bridge). Pure + unit-tested.
    pub fn apply(&mut self, ev: &ConversationEvent) {
        match ev {
            ConversationEvent::Started { session_id } => {
                self.session_id = Some(session_id.clone());
                if self.status != ConvStatus::Thinking {
                    self.status = ConvStatus::Idle;
                }
            }
            ConversationEvent::Delta(text) => {
                self.last_delta_at = Some(Instant::now());
                self.streaming.push_str(text);
            }
            ConversationEvent::Break => {
                // A new text segment: separate it from the previous one, which
                // streamed with no trailing separator.
                if !self.streaming.is_empty() && !self.streaming.ends_with(char::is_whitespace) {
                    self.streaming.push_str("\n\n");
                }
            }
            ConversationEvent::TurnComplete => {
                self.finalize_streaming();
                // If the user queued a message while this reply streamed, show it
                // now (after the reply) and stay Thinking — it's answered next.
                if let Some(next) = self.pending_user.pop_front() {
                    self.messages.push(ConvMessage {
                        role: ConvRole::User,
                        text: next,
                    });
                    self.status = ConvStatus::Thinking;
                } else {
                    self.status = ConvStatus::Idle;
                }
            }
            ConversationEvent::Error(e) => {
                self.finalize_streaming();
                self.status = ConvStatus::Error(e.clone());
            }
            ConversationEvent::Exited => {
                self.finalize_streaming();
                self.status = ConvStatus::Error("session ended".to_string());
            }
        }
    }

    fn finalize_streaming(&mut self) {
        let text = std::mem::take(&mut self.streaming);
        let text = text.trim();
        if !text.is_empty() {
            self.messages.push(ConvMessage {
                role: ConvRole::Assistant,
                text: text.to_string(),
            });
        }
    }

    /// Record a user turn. Shown immediately when idle; when a reply is still in
    /// progress it's queued for display until that reply completes (the session
    /// has already received it and will answer it next).
    fn push_user(&mut self, text: String) {
        if self.status == ConvStatus::Thinking {
            self.pending_user.push_back(text);
        } else {
            self.messages.push(ConvMessage {
                role: ConvRole::User,
                text,
            });
            self.status = ConvStatus::Thinking;
            self.last_delta_at = None;
        }
    }

    fn set_error(&mut self, msg: impl Into<String>) {
        self.status = ConvStatus::Error(msg.into());
    }
}

/// Conversation runtime held on `App`. The model is shared with the bridge; the
/// session handle stays here (only `submit` writes to its stdin).
#[derive(Default)]
pub struct ConversationRuntime {
    /// The headless `claude` subprocess; `None` until first opened / after exit.
    pub session: Option<ConversationSession>,
    /// Shared display model — updated off-loop, read by the renderer.
    pub view: Arc<Mutex<ConversationView>>,
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
                Instant::now() + std::time::Duration::from_secs(4),
            ));
            return;
        }
        self.ensure_conversation_started().await;
        self.ui_state.modal = Modal::Conversation {
            input: Input::default(),
            scroll: 0,
        };
    }

    /// Spawn the headless streaming `claude` session, the TTS speaker, and the
    /// off-loop bridge that updates the shared model + feeds the speaker. Idempotent.
    async fn ensure_conversation_started(&mut self) {
        if self.conversation.session.is_some() {
            return;
        }
        let view = self.conversation.view.clone();
        let dir = match Config::data_dir() {
            Ok(d) => d.join("conversation"),
            Err(e) => {
                view.lock().unwrap().set_error(format!("no data dir: {e}"));
                return;
            }
        };
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            view.lock().unwrap().set_error(format!("mkdir failed: {e}"));
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
        if let Err(e) = tokio::fs::write(dir.join("CLAUDE.md"), claude_md).await {
            warn!(target: "conversation", "failed to write CLAUDE.md: {e}");
        }

        // Streaming-TTS speaker (fed directly by the bridge, off the UI loop).
        // Failure (e.g. no audio device) is non-fatal: chat still works, silent.
        let speaker = if self.config.conversation.enabled {
            match spawn_speaker(self.config.conversation.clone()) {
                Ok(tx) => Some(tx),
                Err(e) => {
                    warn!(target: "conversation", "TTS unavailable: {e}");
                    None
                }
            }
        } else {
            None
        };

        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<ConversationEvent>();
        let command = self.config.conversation.command.clone();
        let permission_mode = self.config.conversation.permission_mode.clone();
        match ConversationSession::spawn(&command, &permission_mode, &dir, ev_tx) {
            Ok(session) => self.conversation.session = Some(session),
            Err(e) => {
                view.lock().unwrap().set_error(e.to_string());
                return;
            }
        }

        // Bridge: entirely off the UI loop. Feeds the speaker AND updates the
        // shared model, so audio and the conversation log both advance while the
        // overlay is closed / the main loop is busy. The renderer just reads the
        // model when visible.
        let bridge_view = view.clone();
        tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                if matches!(ev, ConversationEvent::Delta(_)) {
                    tracing::trace!(target: "conversation", "bridge delta");
                } else {
                    tracing::debug!(target: "conversation", "bridge event: {ev:?}");
                }
                if let Some(sp) = &speaker
                    && let Some(cmd) = crate::conversation::speaker_command_for(&ev)
                {
                    let _ = sp.send(cmd);
                }
                bridge_view.lock().unwrap().apply(&ev);
            }
            tracing::debug!(target: "conversation", "bridge task ended");
        });
        view.lock().unwrap().status = ConvStatus::Idle;
    }

    /// Send a typed user turn to the session. The session serializes turns, so a
    /// message sent mid-reply is queued and answered after — we never interrupt.
    /// Self-heals a dead session by respawning and retrying once.
    pub(super) async fn submit_conversation_input(&mut self, text: String) {
        self.ensure_conversation_started().await;
        let mut result = self.send_to_session(&text).await;
        if result.is_err() {
            // Session likely exited (idle timeout / crash) — respawn and retry.
            self.conversation.session = None;
            self.ensure_conversation_started().await;
            result = self.send_to_session(&text).await;
        }
        match result {
            Ok(()) => self.conversation.view.lock().unwrap().push_user(text),
            Err(e) => {
                tracing::warn!(target: "conversation", "send failed: {e}");
                self.conversation.view.lock().unwrap().set_error(e);
            }
        }
    }

    async fn send_to_session(&mut self, text: &str) -> std::result::Result<(), String> {
        match self.conversation.session.as_mut() {
            Some(session) => session
                .send_user_message(text)
                .await
                .map_err(|e| e.to_string()),
            None => Err("session not running".to_string()),
        }
    }

    /// Render the full-screen conversation overlay (a pure view of the model).
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
        // Inset by one column so the text lines up with the title (which renders
        // two cells in: corner + line char), plus a top/bottom gap.
        let inner = block.inner(area).inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        frame.render_widget(block, area);

        // Layout: history (fills), then the input row bracketed by rules.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(inner);

        // Build the history lines from the shared model.
        let width = chunks[0].width.max(1) as usize;
        let mut lines: Vec<Line> = Vec::new();
        {
            let view = self.conversation.view.lock().unwrap();
            for msg in &view.messages {
                self.push_message_lines(&mut lines, msg.role, &msg.text, width);
            }
            if !view.streaming.is_empty() {
                // The reply is arriving — the text itself is the progress indicator.
                self.push_message_lines(&mut lines, ConvRole::Assistant, &view.streaming, width);
            }
            // Progress / error indicator below the (partial) reply.
            match &view.status {
                ConvStatus::Thinking => {
                    // While tokens actively stream the text is the indicator;
                    // once it stalls (agent thinking / running a tool) show a
                    // spinner so the silence doesn't read as a hang.
                    let streaming_now = view
                        .last_delta_at
                        .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(700));
                    if !streaming_now {
                        let frame_glyph = SPINNER_FRAMES
                            [(self.ui_state.tick_count as usize / 3) % SPINNER_FRAMES.len()];
                        let label = if view.streaming.is_empty() {
                            "thinking…"
                        } else {
                            "working…"
                        };
                        lines.push(Line::from(Span::styled(
                            format!("{frame_glyph} {label}"),
                            Style::default().fg(self.theme.conversation_accent),
                        )));
                    }
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

        // Input: a single prompt line bracketed by top/bottom rules.
        let input_block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_type(self.border_type())
            .border_style(Style::default().fg(self.theme.conversation_accent));
        let input_inner = input_block.inner(chunks[1]);
        frame.render_widget(input_block, chunks[1]);

        const PROMPT: &str = "› ";
        let prompt_w = PROMPT.chars().count() as u16;
        frame.render_widget(
            Paragraph::new(PROMPT).style(Style::default().fg(self.theme.text_secondary)),
            Rect {
                width: prompt_w.min(input_inner.width),
                ..input_inner
            },
        );
        let text_area = Rect {
            x: input_inner.x + prompt_w,
            width: input_inner.width.saturating_sub(prompt_w),
            ..input_inner
        };
        let text_width = text_area.width.max(1);
        let view_scroll = input.visual_scroll(text_width as usize);
        if input.value().is_empty() {
            frame.render_widget(
                Paragraph::new("Type a message…")
                    .style(Style::default().fg(self.theme.text_secondary)),
                text_area,
            );
        } else {
            frame.render_widget(
                Paragraph::new(input.value())
                    .scroll((0, view_scroll as u16))
                    .style(Style::default().fg(self.theme.text_primary)),
                text_area,
            );
        }
        // Real cursor in the text field.
        let col = (input.visual_cursor().saturating_sub(view_scroll)) as u16;
        frame.set_cursor_position((
            text_area.x + col.min(text_width.saturating_sub(1)),
            text_area.y,
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
                self.theme.conversation_accent,
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
            // Esc, Alt-c again, or Ctrl-q close the overlay (session keeps running).
            KeyCode::Esc => {
                self.ui_state.modal = Modal::None;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.ui_state.modal = Modal::None;
            }
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
    use super::*;

    #[test]
    fn wrap_basic() {
        assert_eq!(wrap_text("hello world foo", 11), vec!["hello world", "foo"]);
    }

    #[test]
    fn wrap_long_word_hard_splits() {
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_short_fits_on_one_line() {
        assert_eq!(wrap_text("hi there", 80), vec!["hi there"]);
    }

    #[test]
    fn view_streams_and_finalizes_a_turn() {
        let mut v = ConversationView::default();
        v.push_user("hi".into());
        assert_eq!(v.status, ConvStatus::Thinking);
        assert_eq!(v.messages.len(), 1); // the user message
        v.apply(&ConversationEvent::Started {
            session_id: "s".into(),
        });
        v.apply(&ConversationEvent::Delta("Hello ".into()));
        v.apply(&ConversationEvent::Delta("there.".into()));
        assert_eq!(v.streaming, "Hello there.");
        v.apply(&ConversationEvent::TurnComplete);
        assert_eq!(v.status, ConvStatus::Idle);
        assert_eq!(v.streaming, "");
        assert_eq!(v.messages.last().unwrap().role, ConvRole::Assistant);
        assert_eq!(v.messages.last().unwrap().text, "Hello there.");
    }

    #[test]
    fn view_queues_a_message_sent_mid_reply() {
        let mut v = ConversationView::default();
        v.push_user("first".into()); // Thinking
        v.apply(&ConversationEvent::Delta("answering".into()));
        // A second message while Thinking is queued, not shown.
        v.push_user("second".into());
        assert_eq!(v.pending_user.len(), 1);
        let user_msgs = v
            .messages
            .iter()
            .filter(|m| m.role == ConvRole::User)
            .count();
        assert_eq!(user_msgs, 1, "second message is deferred");
        // When the reply completes, the queued message is shown and we stay Thinking.
        v.apply(&ConversationEvent::TurnComplete);
        assert!(v.pending_user.is_empty());
        assert_eq!(v.status, ConvStatus::Thinking);
        assert_eq!(v.messages.last().unwrap().text, "second");
    }

    #[test]
    fn view_break_inserts_separator() {
        let mut v = ConversationView::default();
        v.apply(&ConversationEvent::Delta("First part.".into()));
        v.apply(&ConversationEvent::Break);
        v.apply(&ConversationEvent::Delta("Second part.".into()));
        assert_eq!(v.streaming, "First part.\n\nSecond part.");
    }
}
