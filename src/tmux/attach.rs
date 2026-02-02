//! Async PTY-based tmux session attachment
//!
//! Provides fully async terminal attachment that runs within the tokio runtime,
//! avoiding the need to drop and recreate the runtime for each attach operation.

use std::io::Write;
use std::os::unix::io::AsRawFd;

use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

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

    info!("Spawned tmux attach-session for {}", session_name);

    // Enter raw mode
    info!("Enabling raw mode for PTY session");
    enable_raw_mode()?;

    // Run the async I/O loop
    info!("Starting async I/O loop");
    let result = run_async_loop(pty, pty_fd, &mut child).await;
    info!("Async I/O loop ended with result: {:?}", result);

    // Restore terminal
    info!("Disabling raw mode");
    let _ = disable_raw_mode();
    let _ = std::io::stdout().flush();

    // Flush any leftover input at the kernel level BEFORE child cleanup
    info!("Flushing stdin with tcflush (before child wait)");
    flush_stdin();
    log_pending_stdin("after first tcflush");

    // Ensure child is cleaned up
    info!("Waiting for child process");
    let _ = child.wait().await;
    info!("Child process finished");

    // Flush again after child exits
    info!("Flushing stdin with tcflush (after child wait)");
    flush_stdin();
    log_pending_stdin("after second tcflush");

    info!("Attach complete, result: {:?}", result);

    Ok(result)
}

/// Log any pending bytes in stdin for debugging
fn log_pending_stdin(context: &str) {
    use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
    use std::os::unix::io::{AsFd, AsRawFd};
    use std::io::Read;

    let stdin = std::io::stdin();
    let fd = stdin.as_fd();
    let mut poll_fds = [PollFd::new(fd, PollFlags::POLLIN)];

    // Check if there's data available (non-blocking)
    match poll(&mut poll_fds, PollTimeout::ZERO) {
        Ok(n) if n > 0 => {
            // There's data - try to read it
            let flags = unsafe { nix::libc::fcntl(stdin.as_raw_fd(), nix::libc::F_GETFL) };
            unsafe { nix::libc::fcntl(stdin.as_raw_fd(), nix::libc::F_SETFL, flags | nix::libc::O_NONBLOCK) };

            let mut buf = [0u8; 256];
            let mut stdin_lock = stdin.lock();
            match stdin_lock.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let bytes = &buf[..n];
                    let as_str: String = bytes.iter().map(|b| {
                        if *b >= 32 && *b < 127 {
                            format!("{}", *b as char)
                        } else {
                            format!("\\x{:02x}", b)
                        }
                    }).collect();
                    warn!("STDIN {} - JUNK FOUND ({} bytes): {}", context, n, as_str);
                }
                Ok(_) => info!("STDIN {} - empty (poll said data but read got none)", context),
                Err(e) => info!("STDIN {} - read error: {}", context, e),
            }
            drop(stdin_lock);

            unsafe { nix::libc::fcntl(stdin.as_raw_fd(), nix::libc::F_SETFL, flags) };
        }
        Ok(_) => info!("STDIN {} - empty", context),
        Err(e) => info!("STDIN {} - poll error: {}", context, e),
    }
}

/// Flush any pending input from stdin at the kernel level
fn flush_stdin() {
    use nix::sys::termios::{tcflush, FlushArg};

    let _ = tcflush(std::io::stdin(), FlushArg::TCIFLUSH);
}

/// Resize PTY using ioctl
fn resize_pty(fd: i32, rows: u16, cols: u16) {
    use nix::libc::{ioctl, winsize, TIOCSWINSZ};

    let ws = winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: fd is valid (from pty.as_raw_fd()), ws is valid stack pointer
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

    // Task 2: stdin -> PTY (raw byte forwarding, no crossterm EventStream)
    // We use raw stdin to avoid conflicting with TUI's EventStream
    let stdin_shutdown = shutdown_tx.clone();
    let stdin_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 1024];

        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = &buf[..n];

                    // Check for Ctrl+Q (0x11) anywhere in the input
                    if data.contains(&0x11) {
                        debug!("Ctrl+Q detected, detaching");
                        let _ = stdin_shutdown.send(AttachResult::Detached).await;
                        break;
                    }

                    // Forward raw bytes to PTY
                    if pty_writer.write_all(data).await.is_err() {
                        break;
                    }
                    let _ = pty_writer.flush().await;
                }
                Err(e) => {
                    warn!("stdin read error: {}", e);
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
