//! Async PTY-based tmux session attachment
//!
//! Provides fully async terminal attachment that runs within the tokio runtime,
//! avoiding the need to drop and recreate the runtime for each attach operation.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use crate::error::Result;

/// Classification of a raw stdin burst by the local attach's keystroke
/// interception state machine. Pure — it performs no I/O and no side effects,
/// so it can be characterization-tested in isolation. The stdin task maps each
/// variant to the corresponding action (forward bytes / break with an
/// [`AttachResult`] / toggle voice / open the switcher).
///
/// The classification order is significant and mirrors the historical inline
/// branching exactly: popup passthrough first, then Ctrl+Q, Ctrl+\, Ctrl+Space,
/// voice, review, editor, and finally plain forwarding (with optional Ctrl+Z
/// stripping).
#[derive(Debug, PartialEq, Eq)]
enum InputAction {
    /// Forward these bytes to the PTY verbatim and keep looping. An empty
    /// vec means "swallow entirely, forward nothing".
    Forward(Vec<u8>),
    /// Ctrl+Space: open the in-session switcher popup, then forward the
    /// remaining bytes (the 0x00 stripped out; may be empty).
    OpenSwitcher(Vec<u8>),
    /// A voice trigger fired: toggle the mic, then forward the remaining bytes
    /// (trigger bytes stripped out; may be empty).
    ToggleVoice(Vec<u8>),
    /// Exit the attach loop with this result (Ctrl+Q, Ctrl+\, review, editor).
    Break(AttachResult),
}

/// Classify a raw stdin burst. See [`InputAction`] for the contract; this is the
/// single source of truth for the local attach's keystroke interception and is
/// covered by characterization tests.
fn classify_input(
    data: &[u8],
    popup_open: bool,
    voice_triggers: &[Vec<u8>],
    review_triggers: &[Vec<u8>],
    editor_triggers: &[Vec<u8>],
    intercept_ctrl_z: bool,
) -> InputAction {
    // While the in-session switcher popup is open, forward every byte to tmux
    // verbatim. tmux routes keystrokes to the popup (which has its own PTY), and
    // our hotkeys (Ctrl+Q etc.) shouldn't fire while the user is in the picker.
    if popup_open {
        return InputAction::Forward(data.to_vec());
    }

    // Ctrl+Q (0x11) anywhere → detach.
    if data.contains(&0x11) {
        return InputAction::Break(AttachResult::Detached);
    }

    // Ctrl+\ (0x1C) → toggle to the shell session.
    if data.contains(&0x1C) {
        return InputAction::Break(AttachResult::SwitchToShell);
    }

    // Ctrl+Space (0x00) → open the switcher popup; swallow the 0x00 byte and
    // forward the rest.
    if data.contains(&0x00) {
        let filtered: Vec<u8> = data.iter().copied().filter(|b| *b != 0x00).collect();
        return InputAction::OpenSwitcher(filtered);
    }

    // Voice-input toggle (Alt-V by default). Unlike the other triggers this does
    // NOT exit the attach: the bytes are swallowed and the rest forwarded. The
    // trigger is recognised whenever it is configured; whether an actual mic
    // toggle fires depends on a listener being wired in, decided by the caller.
    if !voice_triggers.is_empty()
        && voice_triggers
            .iter()
            .any(|pat| contains_subsequence(data, pat))
    {
        let filtered = remove_subsequences(data, voice_triggers);
        return InputAction::ToggleVoice(filtered);
    }

    // Review-toggle trigger (Alt-r by default). Empty `review_triggers` disables
    // it.
    if review_triggers
        .iter()
        .any(|pat| contains_subsequence(data, pat))
    {
        return InputAction::Break(AttachResult::SwitchToReview);
    }

    // User-configured editor trigger bytes. Empty `editor_triggers` (the
    // default) disables this feature entirely.
    if editor_triggers
        .iter()
        .any(|pat| contains_subsequence(data, pat))
    {
        return InputAction::Break(AttachResult::OpenEditor);
    }

    // Plain forwarding, with optional Ctrl+Z stripping for Claude sessions.
    let stripped = if intercept_ctrl_z {
        strip_ctrl_z(data)
    } else {
        None
    };
    InputAction::Forward(stripped.unwrap_or_else(|| data.to_vec()))
}

/// Result of a session attachment attempt
#[derive(Debug, PartialEq, Eq)]
pub enum AttachResult {
    /// User detached with Ctrl+Q or tmux detach (Ctrl+B D)
    Detached,
    /// User pressed Ctrl+\ to toggle between Claude and shell sessions
    SwitchToShell,
    /// User pressed the review key (Alt+r) to switch to this session's diff
    SwitchToReview,
    /// User pressed Ctrl+. to open the editor for the session's worktree
    OpenEditor,
    /// The session/process ended
    SessionEnded,
    /// An error occurred during attachment
    Error(String),
}

/// Outcome of an attach. `final_session` is the tmux session the client
/// was attached to when the attach loop exited — usually the same as the
/// session passed in, but updated when the user picks a different
/// session via the in-session switcher (Ctrl+Space), which runs `tmux
/// switch-client` mid-attach.
#[derive(Debug)]
pub struct AttachOutcome {
    pub result: AttachResult,
    pub final_session: String,
}

/// Async PTY attachment - runs entirely within tokio
///
/// Spawns `tmux attach-session` in a PTY and bridges stdin/stdout asynchronously.
/// Returns when the user detaches (Ctrl+Q or Ctrl+B D) or the session ends.
///
/// `editor_triggers` is a list of byte patterns that, when seen on stdin,
/// cause the attach loop to exit with [`AttachResult::OpenEditor`]. Callers
/// compute these from the user's `OpenInEditor` keybindings — typically a
/// single control byte for `Ctrl-<letter>` bindings, or CSI-u/modifyOtherKeys
/// sequences for `Ctrl-<non-letter>` bindings. Bindings that cannot be
/// detected in raw stdin (e.g. a bare letter) should simply be omitted.
///
/// When `intercept_ctrl_z` is true, Ctrl+Z (`0x1A`) bytes are stripped from
/// stdin before reaching the pane. Use this for Claude sessions where SIGTSTP
/// would freeze the pane with no shell to recover from. Leave it false for
/// shell sessions, where Ctrl+Z is genuinely useful for job control.
pub async fn attach_to_session(
    session_name: &str,
    editor_triggers: Vec<Vec<u8>>,
    review_triggers: Vec<Vec<u8>>,
    voice_triggers: Vec<Vec<u8>>,
    voice_listener: Option<mpsc::UnboundedSender<crate::conversation::ListenerCommand>>,
    recording: Arc<AtomicBool>,
    intercept_ctrl_z: bool,
) -> Result<AttachOutcome> {
    // Get terminal size
    let (cols, rows) = terminal::size().unwrap_or((80, 24));

    // Spawn the transport-agnostic bridge: it opens+sizes the PTY and spawns
    // `tmux attach-session` in it. The local adapter below layers raw-mode,
    // SIGWINCH, and hotkey interception on top — none of which live in the
    // bridge, so the server can reuse the same spawn.
    let bridge = super::HeadlessAttach::spawn(session_name, cols, rows)?;
    let resize = bridge.resize_handle();
    let (pty_reader, pty_writer, _resize_handle, mut child_guard) = bridge.split();

    // Enter raw mode
    info!("Enabling raw mode for PTY session");
    enable_raw_mode()?;

    // Shared state for the in-session switcher: the popup task updates
    // `current_session` after a successful `tmux switch-client`, and the
    // attach outcome reports it back to the caller so subsequent state
    // (shell-toggle pair, editor open) uses the right session.
    let current_session = Arc::new(Mutex::new(session_name.to_string()));
    let popup_open = Arc::new(AtomicBool::new(false));
    // Shared mic state: the stdin task flips it on each Alt-V. Owned by the
    // caller (`ConversationRuntime.recording`) and shared in, so an external
    // IPC toggle and the post-attach UI observe the same flag — no syncing.
    let recording_flag = recording;

    // Run the async I/O loop
    info!("Starting async I/O loop");
    let result = run_async_loop(
        pty_reader,
        pty_writer,
        resize,
        &mut child_guard,
        editor_triggers,
        review_triggers,
        voice_triggers,
        voice_listener,
        recording_flag.clone(),
        intercept_ctrl_z,
        current_session.clone(),
        popup_open,
    )
    .await;
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
    let _ = child_guard.wait().await;
    info!("Child process finished");

    // Flush again after child exits
    info!("Flushing stdin with tcflush (after child wait)");
    flush_stdin();
    log_pending_stdin("after second tcflush");

    let final_session = current_session.lock().await.clone();
    info!(
        "Attach complete, result: {:?}, final session: {}, recording: {}",
        result,
        final_session,
        recording_flag.load(Ordering::Acquire)
    );

    Ok(AttachOutcome {
        result,
        final_session,
    })
}

/// Return true if `haystack` contains `needle` as a contiguous subsequence.
fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Remove every occurrence of any `needle` from `data`. Used to strip an
/// intercepted hotkey's bytes (e.g. the `ESC v` Alt-V burst) so they're never
/// forwarded to the attached pane while we keep the attach running.
fn remove_subsequences(data: &[u8], needles: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    'outer: while i < data.len() {
        for n in needles {
            if !n.is_empty() && data[i..].starts_with(n) {
                i += n.len();
                continue 'outer;
            }
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

/// Strip Ctrl+Z (0x1A) bytes from `data`. Returns `Some(filtered)` when any
/// were removed, `None` otherwise so callers can keep using the original
/// borrow without an allocation.
///
/// Ctrl+Z reaches the foreground process inside the tmux pane as SIGTSTP and
/// suspends it. Since tmux launches Claude directly with no shell wrapper,
/// there's no `fg` to recover with — the pane just freezes. Users hit it by
/// accident; Claude doesn't read it.
fn strip_ctrl_z(data: &[u8]) -> Option<Vec<u8>> {
    data.contains(&0x1A)
        .then(|| data.iter().copied().filter(|b| *b != 0x1A).collect())
}

/// Log any pending bytes in stdin for debugging
fn log_pending_stdin(context: &str) {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::io::Read;
    use std::os::unix::io::{AsFd, AsRawFd};

    let stdin = std::io::stdin();
    let fd = stdin.as_fd();
    let mut poll_fds = [PollFd::new(fd, PollFlags::POLLIN)];

    // Check if there's data available (non-blocking)
    match poll(&mut poll_fds, PollTimeout::ZERO) {
        Ok(n) if n > 0 => {
            // There's data - try to read it
            let flags = unsafe { nix::libc::fcntl(stdin.as_raw_fd(), nix::libc::F_GETFL) };
            unsafe {
                nix::libc::fcntl(
                    stdin.as_raw_fd(),
                    nix::libc::F_SETFL,
                    flags | nix::libc::O_NONBLOCK,
                )
            };

            let mut buf = [0u8; 256];
            let mut stdin_lock = stdin.lock();
            match stdin_lock.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let bytes = &buf[..n];
                    let as_str: String = bytes
                        .iter()
                        .map(|b| {
                            if *b >= 32 && *b < 127 {
                                format!("{}", *b as char)
                            } else {
                                format!("\\x{:02x}", b)
                            }
                        })
                        .collect();
                    warn!("STDIN {} - JUNK FOUND ({} bytes): {}", context, n, as_str);
                }
                Ok(_) => info!(
                    "STDIN {} - empty (poll said data but read got none)",
                    context
                ),
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
pub fn flush_stdin() {
    use nix::sys::termios::{FlushArg, tcflush};

    let _ = tcflush(std::io::stdin(), FlushArg::TCIFLUSH);
}

// Internal plumbing for the attach I/O loop; the arguments are all distinct
// channels/handles/policies with no natural grouping worth a struct here.
#[allow(clippy::too_many_arguments)]
async fn run_async_loop(
    mut pty_reader: tokio::io::ReadHalf<pty_process::Pty>,
    mut pty_writer: tokio::io::WriteHalf<pty_process::Pty>,
    resize: super::ResizeHandle,
    child: &mut super::ChildGuard,
    editor_triggers: Vec<Vec<u8>>,
    review_triggers: Vec<Vec<u8>>,
    voice_triggers: Vec<Vec<u8>>,
    voice_listener: Option<mpsc::UnboundedSender<crate::conversation::ListenerCommand>>,
    recording_flag: Arc<AtomicBool>,
    intercept_ctrl_z: bool,
    current_session: Arc<Mutex<String>>,
    popup_open: Arc<AtomicBool>,
) -> AttachResult {
    // Channel for shutdown signal
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<AttachResult>(1);

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

                    // Classify the burst with the pure interception state
                    // machine; perform the matching side effect here. The order
                    // of checks (popup passthrough → Ctrl+Q → Ctrl+\ →
                    // Ctrl+Space → voice → review → editor → forward) lives in
                    // `classify_input` and is characterization-tested.
                    match classify_input(
                        data,
                        popup_open.load(Ordering::Acquire),
                        &voice_triggers,
                        &review_triggers,
                        &editor_triggers,
                        intercept_ctrl_z,
                    ) {
                        InputAction::Break(result) => {
                            match &result {
                                AttachResult::Detached => debug!("Ctrl+Q detected, detaching"),
                                AttachResult::SwitchToShell => {
                                    debug!("Ctrl+\\ detected, switching to shell")
                                }
                                AttachResult::SwitchToReview => {
                                    debug!("Review trigger detected, switching to review")
                                }
                                AttachResult::OpenEditor => {
                                    debug!("Editor trigger detected, opening editor")
                                }
                                _ => {}
                            }
                            let _ = stdin_shutdown.send(result).await;
                            break;
                        }
                        InputAction::OpenSwitcher(filtered) => {
                            // Open the switcher popup over the attached pane and
                            // spawn the task that runs `tmux display-popup`
                            // followed by `tmux switch-client` on selection. The
                            // attach loop keeps running so the user stays "in"
                            // the pane the whole time.
                            if popup_open
                                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                            {
                                debug!("Ctrl+Space detected, spawning switcher popup");
                                let popup_open = popup_open.clone();
                                let current_session = current_session.clone();
                                tokio::spawn(async move {
                                    run_switcher_popup(current_session, popup_open).await;
                                });
                            }
                            if filtered.is_empty() {
                                continue;
                            }
                            if pty_writer.write_all(&filtered).await.is_err() {
                                break;
                            }
                            let _ = pty_writer.flush().await;
                        }
                        InputAction::ToggleVoice(filtered) => {
                            // Toggle the mic via the listener channel and stay in
                            // the pane. A `tmux display-message` gives feedback
                            // since the TUI status bar isn't visible here.
                            if let Some(listener) = &voice_listener {
                                let now_recording = crate::conversation::apply_listen_action(
                                    listener,
                                    &recording_flag,
                                    crate::conversation::ListenAction::Toggle,
                                );
                                let msg = if now_recording {
                                    "🎙 Recording… (Alt-V to send)"
                                } else {
                                    "Transcribing…"
                                };
                                let target = current_session.lock().await.clone();
                                tokio::spawn(async move {
                                    let _ = tokio::process::Command::new("tmux")
                                        .args(["display-message", "-t", &target, msg])
                                        .status()
                                        .await;
                                });
                            }
                            if filtered.is_empty() {
                                continue;
                            }
                            if pty_writer.write_all(&filtered).await.is_err() {
                                break;
                            }
                            let _ = pty_writer.flush().await;
                        }
                        InputAction::Forward(out) => {
                            if intercept_ctrl_z && out.len() != data.len() {
                                debug!("Ctrl+Z stripped from input");
                            }
                            if out.is_empty() {
                                continue;
                            }
                            if pty_writer.write_all(&out).await.is_err() {
                                break;
                            }
                            let _ = pty_writer.flush().await;
                        }
                    }
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
        use tokio::signal::unix::{SignalKind, signal};

        if let Ok(mut sigwinch) = signal(SignalKind::window_change()) {
            loop {
                sigwinch.recv().await;
                if let Ok((cols, rows)) = terminal::size() {
                    resize.resize(cols, rows);
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

/// Single-quote shell-escape `s` for embedding in a tmux `display-popup`
/// shell command. Wraps in `'…'` and escapes any embedded single quotes
/// as `'\''` (close-quote, literal quote, re-open-quote).
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Run the in-session switcher: spawn a `tmux display-popup` showing
/// the picker subcommand, then on a non-empty result `tmux
/// switch-client` to the chosen session and record it in
/// `current_session`. Always clears `popup_open` before returning.
async fn run_switcher_popup(current_session: Arc<Mutex<String>>, popup_open: Arc<AtomicBool>) {
    let current_name = current_session.lock().await.clone();
    let new_session = run_switcher_popup_inner(&current_name).await;
    if let Some(name) = new_session {
        info!("Switcher picked session: {}", name);
        let switch_status = tokio::process::Command::new("tmux")
            .args(["switch-client", "-t", &name])
            .status()
            .await;
        match switch_status {
            Ok(s) if s.success() => {
                *current_session.lock().await = name;
            }
            Ok(s) => warn!("tmux switch-client exited with {:?}", s.code()),
            Err(e) => warn!("Failed to spawn tmux switch-client: {}", e),
        }
    }
    popup_open.store(false, Ordering::Release);
}

/// Spawn the popup and return the chosen session name, if any.
async fn run_switcher_popup_inner(current_session: &str) -> Option<String> {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "claude-commander".to_string());
    let tmp = std::env::temp_dir().join(format!(
        "cc-pick-{}-{}.txt",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));

    let popup_cmd = format!(
        "{} pick-session --out {} --current {}",
        shell_quote(&exe),
        shell_quote(&tmp.to_string_lossy()),
        shell_quote(current_session),
    );

    let status = tokio::process::Command::new("tmux")
        .args(["display-popup", "-E", "-h", "70%", "-w", "70%", &popup_cmd])
        .status()
        .await;

    match status {
        Ok(s) if !s.success() => {
            warn!("tmux display-popup exited with {:?}", s.code());
        }
        Err(e) => {
            warn!("Failed to spawn tmux display-popup: {}", e);
        }
        _ => {}
    }

    let pick = tokio::fs::read_to_string(&tmp).await.ok();
    let _ = tokio::fs::remove_file(&tmp).await;
    pick.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_subsequence_finds_needle() {
        assert!(contains_subsequence(b"hello world", b"world"));
        assert!(contains_subsequence(b"\x1b[46;5u", b"\x1b[46;5u"));
        assert!(contains_subsequence(b"abc\x1b[46;5udef", b"\x1b[46;5u"));
    }

    #[test]
    fn test_contains_subsequence_rejects_missing() {
        assert!(!contains_subsequence(b"hello", b"world"));
        assert!(!contains_subsequence(b"\x1b[45;5u", b"\x1b[46;5u"));
    }

    #[test]
    fn test_contains_subsequence_empty_cases() {
        assert!(!contains_subsequence(b"", b"x"));
        assert!(!contains_subsequence(b"x", b""));
        assert!(!contains_subsequence(b"ab", b"abc"));
    }

    #[test]
    fn test_remove_subsequences_strips_voice_trigger() {
        let triggers = vec![vec![0x1b, b'v']];
        // A lone Alt-V burst is swallowed entirely (nothing forwarded).
        assert!(remove_subsequences(b"\x1bv", &triggers).is_empty());
        // Surrounding bytes are preserved; only the trigger is removed.
        assert_eq!(
            remove_subsequences(b"ab\x1bvcd", &triggers),
            b"abcd".to_vec()
        );
        // Input without the trigger is untouched.
        assert_eq!(remove_subsequences(b"hello", &triggers), b"hello".to_vec());
    }

    #[test]
    fn test_strip_ctrl_z_removes_byte() {
        assert_eq!(strip_ctrl_z(b"\x1a"), Some(vec![]));
        assert_eq!(strip_ctrl_z(b"a\x1ab"), Some(b"ab".to_vec()));
        assert_eq!(strip_ctrl_z(b"\x1a\x1a\x1a"), Some(vec![]));
        assert_eq!(strip_ctrl_z(b"hi\x1a"), Some(b"hi".to_vec()));
    }

    #[test]
    fn test_strip_ctrl_z_passthrough_when_absent() {
        assert_eq!(strip_ctrl_z(b""), None);
        assert_eq!(strip_ctrl_z(b"hello"), None);
        // Other control bytes must not be stripped.
        assert_eq!(strip_ctrl_z(b"\x03\x11\x1c"), None);
    }

    // -- classify_input characterization tests --
    //
    // These pin down the keystroke-interception state machine that the local
    // attach relied on inline before the bridge refactor. The defaults below
    // mirror the real triggers: Alt-V (`ESC v`) for voice, Alt-r (`ESC r`) for
    // review, Ctrl-e (`0x05`) for an example editor binding.

    fn voice() -> Vec<Vec<u8>> {
        vec![vec![0x1b, b'v']]
    }
    fn review() -> Vec<Vec<u8>> {
        vec![vec![0x1b, b'r']]
    }
    fn editor() -> Vec<Vec<u8>> {
        vec![vec![0x05]]
    }

    /// Classify with the standard trigger set and Ctrl+Z interception on.
    fn classify(data: &[u8], popup_open: bool) -> InputAction {
        classify_input(data, popup_open, &voice(), &review(), &editor(), true)
    }

    #[test]
    fn classify_plain_text_forwards_verbatim() {
        assert_eq!(
            classify(b"hello", false),
            InputAction::Forward(b"hello".to_vec())
        );
    }

    #[test]
    fn classify_ctrl_q_detaches() {
        assert_eq!(
            classify(b"\x11", false),
            InputAction::Break(AttachResult::Detached)
        );
        // Anywhere in the burst, mixed with other bytes.
        assert_eq!(
            classify(b"ab\x11cd", false),
            InputAction::Break(AttachResult::Detached)
        );
    }

    #[test]
    fn classify_ctrl_backslash_switches_to_shell() {
        assert_eq!(
            classify(b"\x1c", false),
            InputAction::Break(AttachResult::SwitchToShell)
        );
    }

    #[test]
    fn classify_ctrl_q_precedes_ctrl_backslash() {
        // Ctrl+Q is checked first, so a burst containing both detaches.
        assert_eq!(
            classify(b"\x11\x1c", false),
            InputAction::Break(AttachResult::Detached)
        );
    }

    #[test]
    fn classify_ctrl_space_opens_switcher_and_strips_nul() {
        assert_eq!(classify(b"\x00", false), InputAction::OpenSwitcher(vec![]));
        // Surrounding bytes survive; only the 0x00 is stripped.
        assert_eq!(
            classify(b"a\x00b", false),
            InputAction::OpenSwitcher(b"ab".to_vec())
        );
    }

    #[test]
    fn classify_voice_trigger_toggles_and_strips() {
        // Lone Alt-V burst toggles voice and forwards nothing.
        assert_eq!(classify(b"\x1bv", false), InputAction::ToggleVoice(vec![]));
        // Trigger embedded in a burst: stripped, the rest forwarded.
        assert_eq!(
            classify(b"x\x1bvy", false),
            InputAction::ToggleVoice(b"xy".to_vec())
        );
    }

    #[test]
    fn classify_review_trigger_breaks() {
        assert_eq!(
            classify(b"\x1br", false),
            InputAction::Break(AttachResult::SwitchToReview)
        );
    }

    #[test]
    fn classify_editor_trigger_breaks() {
        assert_eq!(
            classify(b"\x05", false),
            InputAction::Break(AttachResult::OpenEditor)
        );
    }

    #[test]
    fn classify_popup_open_forwards_everything_verbatim() {
        // With the popup open, even hotkeys are passed through untouched and
        // Ctrl+Z is NOT stripped.
        assert_eq!(
            classify(b"\x11\x1c\x00\x1a", true),
            InputAction::Forward(b"\x11\x1c\x00\x1a".to_vec())
        );
    }

    #[test]
    fn classify_strips_ctrl_z_on_plain_forward_when_enabled() {
        assert_eq!(
            classify(b"a\x1ab", false),
            InputAction::Forward(b"ab".to_vec())
        );
        // A lone Ctrl+Z becomes an empty forward (swallowed).
        assert_eq!(classify(b"\x1a", false), InputAction::Forward(vec![]));
    }

    #[test]
    fn classify_keeps_ctrl_z_when_interception_disabled() {
        let action = classify_input(b"a\x1ab", false, &voice(), &review(), &editor(), false);
        assert_eq!(action, InputAction::Forward(b"a\x1ab".to_vec()));
    }

    #[test]
    fn classify_empty_triggers_disable_review_and_editor() {
        // With no review/editor triggers configured, those bytes are forwarded
        // as ordinary input rather than intercepted.
        let action = classify_input(b"\x1br", false, &voice(), &[], &[], true);
        assert_eq!(action, InputAction::Forward(b"\x1br".to_vec()));
        let action = classify_input(b"\x05", false, &voice(), &[], &[], true);
        assert_eq!(action, InputAction::Forward(b"\x05".to_vec()));
    }

    #[test]
    fn classify_voice_precedes_review_when_both_match() {
        // Ordering: voice is checked before review. A burst containing both
        // triggers toggles voice (and strips it) rather than breaking to review.
        let action = classify_input(b"\x1bv\x1br", false, &voice(), &review(), &editor(), true);
        assert_eq!(action, InputAction::ToggleVoice(b"\x1br".to_vec()));
    }
}
