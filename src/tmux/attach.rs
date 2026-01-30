//! Async PTY-based tmux session attachment
//!
//! Provides fully async terminal attachment that runs within the tokio runtime,
//! avoiding the need to drop and recreate the runtime for each attach operation.

use std::io::Write;
use std::os::unix::io::AsRawFd;

use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::Result;

/// Result of a session attachment attempt
#[derive(Debug)]
pub enum AttachResult {
    /// User detached with Ctrl+Q or tmux detach (Ctrl+B D)
    Detached,
    /// The session/process ended
    SessionEnded,
    /// An error occurred during attachment
    Error(String),
}

/// Async PTY attachment - runs entirely within tokio
///
/// Spawns `tmux attach-session` in a PTY and bridges stdin/stdout asynchronously.
/// Returns when the user detaches (Ctrl+Q or Ctrl+B D) or the session ends.
pub async fn attach_to_session(session_name: &str) -> Result<AttachResult> {
    // Get terminal size
    let (cols, rows) = terminal::size().unwrap_or((80, 24));

    // Open PTY (async)
    let pty = pty_process::Pty::new()?;
    pty.resize(pty_process::Size::new(rows, cols))?;

    // Get the raw fd for resize operations (before we split the pty)
    let pty_fd = pty.as_raw_fd();

    // Spawn tmux attach-session
    let mut cmd = pty_process::Command::new("tmux");
    cmd.args(["attach-session", "-t", session_name]);
    let mut child = cmd.spawn(&pty.pts()?)?;

    debug!("Spawned tmux attach-session for {}", session_name);

    // Enter raw mode
    enable_raw_mode()?;

    // Run the async I/O loop
    let result = run_async_loop(pty, pty_fd, &mut child).await;

    // Restore terminal
    let _ = disable_raw_mode();
    let _ = std::io::stdout().flush();

    // Ensure child is cleaned up
    let _ = child.wait().await;

    debug!("Attach result: {:?}", result);

    Ok(result)
}

/// Resize PTY using raw fd and ioctl
fn resize_pty(fd: i32, rows: u16, cols: u16) {
    use libc::{ioctl, winsize, TIOCSWINSZ};

    let ws = winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    unsafe {
        ioctl(fd, TIOCSWINSZ, &ws);
    }
}

async fn run_async_loop(
    pty: pty_process::Pty,
    pty_fd: i32,
    child: &mut tokio::process::Child,
) -> AttachResult {
    // Channel for shutdown signal
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<AttachResult>(1);

    // Split PTY into read and write halves for concurrent access
    let (mut pty_reader, mut pty_writer) = tokio::io::split(pty);

    // Task 1: PTY output -> stdout
    let stdout_shutdown = shutdown_tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        let mut buf = [0u8; 4096];

        loop {
            match pty_reader.read(&mut buf).await {
                Ok(0) => {
                    let _ = stdout_shutdown.send(AttachResult::SessionEnded).await;
                    break;
                }
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    let _ = stdout.flush().await;
                }
                Err(e) => {
                    // EIO is expected when PTY closes
                    if e.raw_os_error() != Some(5) {
                        warn!("PTY read error: {}", e);
                    }
                    let _ = stdout_shutdown.send(AttachResult::SessionEnded).await;
                    break;
                }
            }
        }
    });

    // Task 2: stdin (via crossterm events) -> PTY
    let stdin_shutdown = shutdown_tx.clone();
    let stdin_task = tokio::spawn(async move {
        let mut reader = EventStream::new();

        while let Some(event_result) = reader.next().await {
            match event_result {
                Ok(Event::Key(key_event)) => {
                    // Check for Ctrl+Q (our escape hatch)
                    if key_event.code == KeyCode::Char('q')
                        && key_event.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        debug!("Ctrl+Q detected, detaching");
                        let _ = stdin_shutdown.send(AttachResult::Detached).await;
                        break;
                    }

                    // Convert key event to bytes and send to PTY
                    if let Some(bytes) = key_event_to_bytes(&key_event) {
                        if pty_writer.write_all(&bytes).await.is_err() {
                            break;
                        }
                        // Flush after each key for responsiveness
                        let _ = pty_writer.flush().await;
                    }
                }
                Ok(Event::Resize(cols, rows)) => {
                    // Handle terminal resize via ioctl
                    resize_pty(pty_fd, rows, cols);
                }
                Ok(Event::Paste(text)) => {
                    // Handle paste events
                    if pty_writer.write_all(text.as_bytes()).await.is_err() {
                        break;
                    }
                    let _ = pty_writer.flush().await;
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("Event stream error: {}", e);
                    break;
                }
            }
        }
    });

    // Task 3: SIGWINCH handling (Unix only, as backup for resize events)
    #[cfg(unix)]
    let resize_task = tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};

        if let Ok(mut sigwinch) = signal(SignalKind::window_change()) {
            loop {
                sigwinch.recv().await;
                if let Ok((cols, rows)) = terminal::size() {
                    resize_pty(pty_fd, rows, cols);
                }
            }
        }
    });

    #[cfg(not(unix))]
    let resize_task = tokio::spawn(async {});

    // Wait for shutdown signal or child exit
    let result = tokio::select! {
        result = shutdown_rx.recv() => {
            result.unwrap_or(AttachResult::Detached)
        }
        status = child.wait() => {
            match status {
                Ok(s) if s.success() => AttachResult::Detached,
                Ok(_) => AttachResult::SessionEnded,
                Err(_) => AttachResult::SessionEnded,
            }
        }
    };

    // Abort spawned tasks
    stdout_task.abort();
    stdin_task.abort();
    resize_task.abort();

    result
}

/// Convert crossterm KeyEvent to raw bytes for PTY
fn key_event_to_bytes(event: &crossterm::event::KeyEvent) -> Option<Vec<u8>> {
    use crossterm::event::KeyCode;

    match event.code {
        KeyCode::Char(c) => {
            if event.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+A = 1, Ctrl+B = 2, etc.
                if c.is_ascii_lowercase() || c.is_ascii_uppercase() {
                    let ctrl_char = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a' - 1);
                    Some(vec![ctrl_char])
                } else {
                    // Ctrl with non-letter (like Ctrl+[)
                    match c {
                        '[' => Some(vec![0x1b]), // ESC
                        '\\' => Some(vec![0x1c]),
                        ']' => Some(vec![0x1d]),
                        '^' => Some(vec![0x1e]),
                        '_' => Some(vec![0x1f]),
                        _ => None,
                    }
                }
            } else if event.modifiers.contains(KeyModifiers::ALT) {
                // Alt+key sends ESC followed by the key
                let mut buf = vec![0x1b];
                let mut char_buf = [0u8; 4];
                let s = c.encode_utf8(&mut char_buf);
                buf.extend_from_slice(s.as_bytes());
                Some(buf)
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                Some(s.as_bytes().to_vec())
            }
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => {
            if event.modifiers.contains(KeyModifiers::SHIFT) {
                Some(b"\x1b[Z".to_vec()) // Shift+Tab
            } else {
                Some(vec![b'\t'])
            }
        }
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::F(n) => {
            // F1-F12 escape sequences
            let seq = match n {
                1 => b"\x1bOP".to_vec(),
                2 => b"\x1bOQ".to_vec(),
                3 => b"\x1bOR".to_vec(),
                4 => b"\x1bOS".to_vec(),
                5 => b"\x1b[15~".to_vec(),
                6 => b"\x1b[17~".to_vec(),
                7 => b"\x1b[18~".to_vec(),
                8 => b"\x1b[19~".to_vec(),
                9 => b"\x1b[20~".to_vec(),
                10 => b"\x1b[21~".to_vec(),
                11 => b"\x1b[23~".to_vec(),
                12 => b"\x1b[24~".to_vec(),
                _ => return None,
            };
            Some(seq)
        }
        KeyCode::Null => Some(vec![0]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn test_key_event_to_bytes_chars() {
        let event = make_key_event(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(key_event_to_bytes(&event), Some(vec![b'a']));

        let event = make_key_event(KeyCode::Char('Z'), KeyModifiers::NONE);
        assert_eq!(key_event_to_bytes(&event), Some(vec![b'Z']));
    }

    #[test]
    fn test_key_event_to_bytes_ctrl() {
        let event = make_key_event(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_bytes(&event), Some(vec![3])); // Ctrl+C

        let event = make_key_event(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_bytes(&event), Some(vec![1])); // Ctrl+A
    }

    #[test]
    fn test_key_event_to_bytes_special() {
        let event = make_key_event(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(key_event_to_bytes(&event), Some(vec![b'\r']));

        let event = make_key_event(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(key_event_to_bytes(&event), Some(vec![0x7f]));

        let event = make_key_event(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(key_event_to_bytes(&event), Some(vec![0x1b]));
    }

    #[test]
    fn test_key_event_to_bytes_arrows() {
        let event = make_key_event(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(key_event_to_bytes(&event), Some(b"\x1b[A".to_vec()));

        let event = make_key_event(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(key_event_to_bytes(&event), Some(b"\x1b[B".to_vec()));
    }

    #[test]
    fn test_key_event_to_bytes_function_keys() {
        let event = make_key_event(KeyCode::F(1), KeyModifiers::NONE);
        assert_eq!(key_event_to_bytes(&event), Some(b"\x1bOP".to_vec()));

        let event = make_key_event(KeyCode::F(5), KeyModifiers::NONE);
        assert_eq!(key_event_to_bytes(&event), Some(b"\x1b[15~".to_vec()));
    }
}
