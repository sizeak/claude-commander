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

use crate::backend::{AttachEnd, AttachResizer, AttachStreams, AttachTerminator};
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
    /// Ctrl+V during a remote attach: capture the local clipboard image and
    /// upload it. Carries the original burst verbatim (the `0x16` is *not*
    /// stripped here) so the effectful handler can forward it as a fallback when
    /// the clipboard holds no image or the upload fails.
    PasteImage(Vec<u8>),
    /// Exit the attach loop with this result (Ctrl+Q, Ctrl+\, review, editor).
    Break(AttachResult),
}

/// Sink for a clipboard image captured during a remote attach. The attach loop
/// reads the operator's *local* clipboard (the remote agent can't) and hands the
/// encoded PNG bytes here; the implementation ships them to wherever the agent
/// can read them (the server) and returns once the image's path has been
/// injected into the pane. Kept transport-agnostic (a `String` error) so the
/// attach loop doesn't depend on the backend error type.
#[async_trait::async_trait]
pub trait ImagePasteSink: Send + Sync {
    async fn upload(&self, png: Vec<u8>) -> std::result::Result<(), String>;
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
    image_paste_enabled: bool,
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

    // Ctrl+V (0x16) during a REMOTE attach: capture the operator's local
    // clipboard image and upload it, because the remote agent reads the *server's*
    // clipboard (empty) on paste. Only when enabled — a local attach leaves this
    // false so Ctrl+V is forwarded and the co-located agent reads the clipboard
    // itself. The original burst rides along so the effectful handler can forward
    // it verbatim when there's no clipboard image (fallback to normal Ctrl+V).
    //
    // This matches the standard control-byte encoding. Under an enhanced keyboard
    // protocol (kitty/CSI-u or xterm modifyOtherKeys) the remote agent may have
    // enabled, Ctrl+V can instead arrive as an escape sequence (e.g. `\x1b[118;5u`)
    // and won't match here — paste then falls through to plain forwarding, same as
    // a local attach. Placed after the configurable editor/voice/review triggers
    // so an explicit user binding on 0x16 still wins.
    if image_paste_enabled && data.contains(&0x16) {
        return InputAction::PasteImage(data.to_vec());
    }

    // Plain forwarding, with optional Ctrl+Z stripping for Claude sessions.
    let stripped = if intercept_ctrl_z {
        strip_ctrl_z(data)
    } else {
        None
    };
    InputAction::Forward(stripped.unwrap_or_else(|| data.to_vec()))
}

/// What to do with a Ctrl+V paste burst, given the clipboard capture result.
/// Pure so the four outcomes are unit-testable without a real clipboard/network;
/// the effectful [`handle_image_paste`] maps it to spawn-upload + forward.
#[derive(Debug, PartialEq, Eq)]
enum PasteDecision {
    /// Forward these bytes to the pane verbatim; do not upload. Covers no
    /// clipboard image, a capture error, and an over-limit image — in every case
    /// Ctrl+V behaves as it would on a local attach.
    Forward(Vec<u8>),
    /// Upload this PNG (fire-and-forget) and forward these bytes — the original
    /// burst with `0x16` stripped (the Ctrl+V is swallowed; the server injects
    /// the path, which appears in the pane via the output stream).
    Upload { png: Vec<u8>, forward: Vec<u8> },
}

/// Pure paste decision from a clipboard-capture result. An image within the size
/// cap → upload + strip `0x16`; no image, a capture error, or an over-limit
/// image → forward the burst verbatim.
fn paste_decision(
    capture: std::result::Result<Option<Vec<u8>>, String>,
    orig: &[u8],
) -> PasteDecision {
    match capture {
        Ok(Some(png)) if png.len() <= crate::paste_image::MAX_IMAGE_BYTES => {
            PasteDecision::Upload {
                png,
                forward: orig.iter().copied().filter(|b| *b != 0x16).collect(),
            }
        }
        _ => PasteDecision::Forward(orig.to_vec()),
    }
}

/// Handle a Ctrl+V image-paste burst during a remote attach: read the local
/// clipboard image and, if present and within the size cap, upload it via
/// `sink`. Returns the bytes the stdin loop should forward to the pane (see
/// [`PasteDecision`]).
///
/// The clipboard read runs on a blocking thread and is awaited (a fast local
/// round-trip), keeping the no-image fallback in-order. The **upload** is
/// spawned fire-and-forget: a large image over a slow link must never freeze
/// keystroke forwarding — this is the only stdin path. An upload failure is
/// therefore surfaced only as a `warn!` log (there is no reliable on-screen
/// channel: the remote pane's tmux is not the operator's local tmux, so a
/// `display-message` would target a name that isn't here). Success needs no
/// notification — the injected path appears in the prompt.
///
/// Accepted trade-off: because the upload is spawned, keystrokes typed
/// immediately after Ctrl+V travel the WS while the path injection races over
/// HTTP, so very fast typing within the round-trip can land before the path
/// (splitting a word around it — the surrounding spaces keep them from merging,
/// and the user is watching). This is the deliberate price of not blocking
/// stdin during the upload; do not "fix" it by awaiting the upload inline.
async fn handle_image_paste(orig: &[u8], sink: Option<&Arc<dyn ImagePasteSink>>) -> Vec<u8> {
    // `handle_image_paste` is only reached when `classify_input` returned
    // `PasteImage`, which requires `image_paste_enabled == sink.is_some()`; so
    // `sink` is `Some` here. Defensive fallback keeps the contract explicit.
    let Some(sink) = sink else {
        return orig.to_vec();
    };
    let capture = capture_clipboard_png().await;
    match &capture {
        Err(e) => warn!("clipboard image read failed: {e}"),
        Ok(None) => debug!("Ctrl+V with no clipboard image; forwarding verbatim"),
        Ok(Some(png)) if png.len() > crate::paste_image::MAX_IMAGE_BYTES => warn!(
            "clipboard image {} bytes exceeds {} limit; not uploading",
            png.len(),
            crate::paste_image::MAX_IMAGE_BYTES
        ),
        Ok(Some(_)) => {}
    }
    match paste_decision(capture, orig) {
        PasteDecision::Forward(bytes) => bytes,
        PasteDecision::Upload { png, forward } => {
            let sink = sink.clone();
            tokio::spawn(async move {
                match sink.upload(png).await {
                    Ok(()) => debug!("pasted clipboard image to remote session"),
                    Err(e) => warn!("image paste upload failed: {e}"),
                }
            });
            forward
        }
    }
}

/// Read an image from the operator's local OS clipboard and encode it as PNG.
/// `Ok(None)` means the clipboard holds no image (so the caller forwards Ctrl+V
/// verbatim). The blocking `arboard` read runs on a blocking thread so it never
/// stalls the async stdin loop.
#[cfg(feature = "clipboard")]
async fn capture_clipboard_png() -> std::result::Result<Option<Vec<u8>>, String> {
    tokio::task::spawn_blocking(|| {
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        match clipboard.get_image() {
            Ok(img) => {
                let png = crate::paste_image::encode_rgba_png(
                    img.width as u32,
                    img.height as u32,
                    img.bytes.into_owned(),
                )
                .map_err(|e| e.to_string())?;
                Ok(Some(png))
            }
            // No image on the clipboard (text, or empty) — not an error.
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    })
    .await
    .map_err(|e| format!("clipboard task panicked: {e}"))?
}

/// Clipboard support compiled out (`--no-default-features`): there is no local
/// clipboard to read, so paste always falls back to forwarding Ctrl+V.
#[cfg(not(feature = "clipboard"))]
async fn capture_clipboard_png() -> std::result::Result<Option<Vec<u8>>, String> {
    Ok(None)
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

/// Configuration for one interactive attach, driven by [`run_attach`]. Bundles
/// the keystroke-interception policy and the local-only affordances (switcher
/// popup, voice) so the transport-agnostic loop keeps a single signature.
///
/// `editor_triggers` is a list of byte patterns that, when seen on stdin, cause
/// the attach loop to exit with [`AttachResult::OpenEditor`]. Callers compute
/// these from the user's `OpenInEditor` keybindings — typically a single
/// control byte for `Ctrl-<letter>` bindings, or CSI-u/modifyOtherKeys
/// sequences for `Ctrl-<non-letter>` bindings. Bindings that cannot be detected
/// in raw stdin (e.g. a bare letter) should simply be omitted.
///
/// Hook the attach loop calls with the switcher's picked tmux session name
/// before running `tmux switch-client`: revives the session when its tmux
/// session is missing or its pane died, returning the (primary) name to
/// switch to. Supplied by frontends that own a `CommanderService` (see
/// `CommanderService::switcher_revive_hook`) so the revive runs in the
/// process that owns the state store; the attach loop itself stays
/// transport-agnostic and holds no service handle.
pub type SwitcherRevive = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send>>
        + Send
        + Sync,
>;

/// When `intercept_ctrl_z` is true, Ctrl+Z (`0x1A`) bytes are stripped from
/// stdin before reaching the pane. Use this for Claude sessions where SIGTSTP
/// would freeze the pane with no shell to recover from. Leave it false for
/// shell sessions, where Ctrl+Z is genuinely useful for job control.
pub struct AttachConfig {
    pub editor_triggers: Vec<Vec<u8>>,
    pub review_triggers: Vec<Vec<u8>>,
    pub voice_triggers: Vec<Vec<u8>>,
    pub voice_listener: Option<crate::conversation::ListenerHandle>,
    pub recording: Arc<AtomicBool>,
    pub intercept_ctrl_z: bool,
    /// Whether the in-session Ctrl+Space switcher popup is available. It's a
    /// local capability (runs `tmux display-popup`/`switch-client` against the
    /// operator's own server), so a remote attach sets this false and Ctrl+Space
    /// is forwarded to the pane verbatim.
    pub switcher_enabled: bool,
    /// The tmux session name currently attached, for the switcher popup and the
    /// voice feedback `tmux display-message`. The TUI sets this for remote
    /// attaches too (the session's tmux name rides in on the wire), so it's
    /// normally `Some`. The switcher is gated separately by `switcher_enabled`
    /// (off for remote), and the voice `display-message` is best-effort — it
    /// runs against the operator's local tmux, so for a remote session it may
    /// simply target a name that isn't there. `None` disables both.
    pub session_name: Option<String>,
    /// Revives the switcher's pick before `tmux switch-client` when its tmux
    /// session died (e.g. after a reboot) — the same revive-on-attach the
    /// tree view gets. `None` switches to the raw name, which fails for a
    /// dead session.
    pub switcher_revive: Option<SwitcherRevive>,
    /// Sink for clipboard-image paste, set only for a **remote** attach (the
    /// backend's `client_side_image_paste` capability). When `Some`, Ctrl+V is
    /// intercepted: the operator's local clipboard image is captured, encoded,
    /// and uploaded via this sink instead of being forwarded to the remote pane.
    /// `None` (local attach) forwards Ctrl+V so the co-located agent reads the
    /// clipboard directly, exactly as before.
    pub image_paste: Option<Arc<dyn ImagePasteSink>>,
}

/// Async PTY attachment by tmux session name — the CLI/local entry point.
///
/// Spawns `tmux attach-session` in a PTY, wraps it as [`AttachStreams`], and
/// drives it through [`run_attach`]. Returns when the user detaches (Ctrl+Q or
/// Ctrl+B D) or the session ends.
#[allow(clippy::too_many_arguments)]
pub async fn attach_to_session(
    session_name: &str,
    editor_triggers: Vec<Vec<u8>>,
    review_triggers: Vec<Vec<u8>>,
    voice_triggers: Vec<Vec<u8>>,
    voice_listener: Option<crate::conversation::ListenerHandle>,
    recording: Arc<AtomicBool>,
    intercept_ctrl_z: bool,
    switcher_revive: Option<SwitcherRevive>,
) -> Result<AttachOutcome> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    // Local TUI/CLI attach talks to the user's own tmux server, so no socket-dir
    // isolation (unlike the server's WS attach, which honours `tmux_tmpdir`).
    let streams = super::HeadlessAttach::spawn(session_name, cols, rows, None)?.into_streams();
    let cfg = AttachConfig {
        editor_triggers,
        review_triggers,
        voice_triggers,
        voice_listener,
        recording,
        intercept_ctrl_z,
        switcher_enabled: true,
        session_name: Some(session_name.to_string()),
        switcher_revive,
        // The CLI/local attach runs the agent on this machine, so it reads the
        // local clipboard itself — no client-side capture.
        image_paste: None,
    };
    run_attach(streams, cfg).await
}

/// Clipboard-image sink that forwards a captured PNG through a
/// [`CommanderBackend`](crate::backend::CommanderBackend)'s `paste_image` route.
/// Shared by any frontend attaching to a *remote* session (the TUI has its own
/// copy inline; the CLI's remote attach uses this one).
struct BackendImagePaste {
    backend: Arc<dyn crate::backend::CommanderBackend>,
    id: crate::session::SessionId,
}

#[async_trait::async_trait]
impl ImagePasteSink for BackendImagePaste {
    async fn upload(&self, png: Vec<u8>) -> std::result::Result<(), String> {
        self.backend
            .paste_image(self.id, png)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Attach to a session on a `backend` — the remote entry point mirroring
/// [`attach_to_session`]'s local PTY path. Resolves `query` to a session via the
/// backend, opens an attach connection (a WebSocket for a remote backend), and
/// drives it through [`run_attach`] with the CLI-appropriate policy: the
/// in-session switcher and voice input are TUI-only affordances so they stay
/// off, while clipboard-image paste is enabled when the backend advertises
/// `client_side_image_paste`.
///
/// Lives in core (not `main.rs`) so the config assembly is unit-testable and the
/// CLI stays thin.
pub async fn attach_backend_session(
    backend: Arc<dyn crate::backend::CommanderBackend>,
    query: &str,
    editor_triggers: Vec<Vec<u8>>,
) -> crate::backend::BResult<AttachOutcome> {
    use crate::backend::{AttachKind, BackendError};

    let detail = backend
        .session_detail(query, None)
        .await?
        .ok_or(BackendError::NotFound)?;
    let id = detail.info.session_id;

    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let conn = backend.attach(id, cols, rows, AttachKind::Agent).await?;
    let streams = conn.split();

    let image_paste: Option<Arc<dyn ImagePasteSink>> = backend
        .capabilities()
        .client_side_image_paste
        .then(|| {
            Arc::new(BackendImagePaste {
                backend: backend.clone(),
                id,
            }) as Arc<dyn ImagePasteSink>
        });

    let cfg = AttachConfig {
        editor_triggers,
        review_triggers: Vec::new(),
        voice_triggers: Vec::new(),
        voice_listener: None,
        recording: Arc::new(AtomicBool::new(false)),
        intercept_ctrl_z: true,
        // Local-only affordance: the switcher popup runs `tmux display-popup`
        // against the operator's own server, where the remote session isn't.
        switcher_enabled: false,
        session_name: None,
        switcher_revive: None,
        image_paste,
    };

    Ok(run_attach(streams, cfg).await?)
}

/// Drive one interactive attach over transport-agnostic [`AttachStreams`]
/// (a local PTY, or a remote WebSocket via the backend). Enters raw mode, pumps
/// stdin/stdout through the [`classify_input`] interception state machine,
/// forwards SIGWINCH resizes to the [`AttachResizer`], and tears the attach down
/// (via [`AttachTerminator::detach`]) on exit. The TUI supplies streams from
/// `backend.attach(...)`; the CLI wraps a local PTY via [`attach_to_session`].
pub async fn run_attach(streams: AttachStreams, cfg: AttachConfig) -> Result<AttachOutcome> {
    let AttachStreams {
        reader,
        writer,
        resizer,
        mut terminator,
    } = streams;

    info!("Enabling raw mode for attach session");
    enable_raw_mode()?;

    // Shared state for the in-session switcher: the popup task updates
    // `current_session` after a successful `tmux switch-client`, and the attach
    // outcome reports it back to the caller so subsequent state (shell-toggle
    // pair, editor open) uses the right session.
    let current_session = Arc::new(Mutex::new(cfg.session_name.clone().unwrap_or_default()));
    let popup_open = Arc::new(AtomicBool::new(false));

    info!("Starting async I/O loop");
    let result = run_async_loop(
        reader,
        writer,
        resizer,
        &mut terminator,
        &cfg,
        current_session.clone(),
        popup_open,
    )
    .await;
    info!("Async I/O loop ended with result: {:?}", result);

    info!("Disabling raw mode");
    let _ = disable_raw_mode();
    let _ = std::io::stdout().flush();

    // Flush any leftover input at the kernel level before teardown.
    flush_stdin();
    log_pending_stdin("after first tcflush");

    // Deterministic teardown: kill the attach client (idempotent if it already
    // exited). Detaches the client; the tmux session + program keep running.
    info!("Detaching attach transport");
    terminator.detach().await;

    // Flush again after teardown to discard stale input.
    flush_stdin();
    log_pending_stdin("after second tcflush");

    let final_session = current_session.lock().await.clone();
    info!(
        "Attach complete, result: {:?}, final session: {}, recording: {}",
        result,
        final_session,
        cfg.recording.load(Ordering::Acquire)
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

// Internal plumbing for the attach I/O loop. Generic over the transport: the
// byte streams are boxed trait objects (a local PTY or a remote socket), and
// termination is observed via the [`AttachTerminator`] rather than a concrete
// child handle.
async fn run_async_loop(
    mut reader: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    mut writer: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    resizer: AttachResizer,
    terminator: &mut Box<dyn AttachTerminator>,
    cfg: &AttachConfig,
    current_session: Arc<Mutex<String>>,
    popup_open: Arc<AtomicBool>,
) -> AttachResult {
    // Clone the interception policy out of `cfg` so the spawned tasks can own it.
    let editor_triggers = cfg.editor_triggers.clone();
    let review_triggers = cfg.review_triggers.clone();
    let voice_triggers = cfg.voice_triggers.clone();
    let voice_listener = cfg.voice_listener.clone();
    let recording_flag = cfg.recording.clone();
    let intercept_ctrl_z = cfg.intercept_ctrl_z;
    let switcher_enabled = cfg.switcher_enabled;
    let switcher_revive = cfg.switcher_revive.clone();
    let image_paste = cfg.image_paste.clone();

    // Channel for shutdown signal
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<AttachResult>(1);

    // Task 1: transport output -> stdout
    let stdout_shutdown = shutdown_tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        let mut buf = [0u8; 4096];

        loop {
            match reader.read(&mut buf).await {
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
                        image_paste.is_some(),
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
                            // The in-session switcher is a local capability
                            // (`tmux display-popup`/`switch-client` on the
                            // operator's own server). When disabled (e.g. a
                            // remote attach) Ctrl+Space is just forwarded.
                            if switcher_enabled
                                && popup_open
                                    .compare_exchange(
                                        false,
                                        true,
                                        Ordering::AcqRel,
                                        Ordering::Acquire,
                                    )
                                    .is_ok()
                            {
                                // Open the switcher popup over the attached pane
                                // and spawn the task that runs `tmux
                                // display-popup` then `tmux switch-client` on
                                // selection. The attach loop keeps running so
                                // the user stays "in" the pane the whole time.
                                debug!("Ctrl+Space detected, spawning switcher popup");
                                let popup_open = popup_open.clone();
                                let current_session = current_session.clone();
                                let revive = switcher_revive.clone();
                                tokio::spawn(async move {
                                    run_switcher_popup(current_session, popup_open, revive).await;
                                });
                            }
                            if filtered.is_empty() {
                                continue;
                            }
                            if writer.write_all(&filtered).await.is_err() {
                                break;
                            }
                            let _ = writer.flush().await;
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
                            if writer.write_all(&filtered).await.is_err() {
                                break;
                            }
                            let _ = writer.flush().await;
                        }
                        InputAction::PasteImage(orig) => {
                            // Capture the local clipboard image and upload it.
                            // Returns the bytes to forward: empty/rest on success
                            // (Ctrl+V swallowed), or the original burst as a
                            // fallback when there's no image or the upload fails.
                            let to_forward = handle_image_paste(&orig, image_paste.as_ref()).await;
                            if to_forward.is_empty() {
                                continue;
                            }
                            if writer.write_all(&to_forward).await.is_err() {
                                break;
                            }
                            let _ = writer.flush().await;
                        }
                        InputAction::Forward(out) => {
                            if intercept_ctrl_z && out.len() != data.len() {
                                debug!("Ctrl+Z stripped from input");
                            }
                            if out.is_empty() {
                                continue;
                            }
                            if writer.write_all(&out).await.is_err() {
                                break;
                            }
                            let _ = writer.flush().await;
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
                    resizer.resize(cols, rows);
                }
            }
        }
    });

    #[cfg(not(unix))]
    let resize_task = tokio::spawn(async move {
        // Keep `resizer` owned on non-unix so the signature matches.
        let _ = resizer;
    });

    // Wait for a shutdown signal (an intercepted hotkey) or the transport
    // ending on its own (PTY EOF / detach key / socket close).
    let result = tokio::select! {
        result = shutdown_rx.recv() => {
            result.unwrap_or(AttachResult::Detached)
        }
        end = terminator.wait() => {
            match end {
                AttachEnd::Detached => AttachResult::Detached,
                AttachEnd::SessionEnded => AttachResult::SessionEnded,
                AttachEnd::Error(e) => AttachResult::Error(e),
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
/// the picker subcommand, then on a non-empty result revive the chosen
/// session if its tmux session died, `tmux switch-client` to it, and
/// record it in `current_session`. Always clears `popup_open` before
/// returning.
async fn run_switcher_popup(
    current_session: Arc<Mutex<String>>,
    popup_open: Arc<AtomicBool>,
    revive: Option<SwitcherRevive>,
) {
    let current_name = current_session.lock().await.clone();
    let new_session = run_switcher_popup_inner(&current_name).await;
    if let Some(name) = new_session {
        info!("Switcher picked session: {}", name);
        // Revive the pick before switching — its tmux session may have died
        // (e.g. after a reboot), and `switch-client` can't create sessions.
        // On a revive error fall back to the raw name: the pick may exist in
        // tmux without being in commander state.
        let target = match &revive {
            Some(revive) => match revive(name.clone()).await {
                Ok(target) => target,
                Err(e) => {
                    warn!("Failed to revive picked session {}: {}", name, e);
                    name
                }
            },
            None => name,
        };
        let switch_status = tokio::process::Command::new("tmux")
            .args(["switch-client", "-t", &target])
            .status()
            .await;
        match switch_status {
            Ok(s) if s.success() => {
                *current_session.lock().await = target;
            }
            Ok(s) => {
                warn!("tmux switch-client exited with {:?}", s.code());
                // Surface the failure in the pane the user is still on;
                // without this a dead pick looks like the popup did nothing.
                let _ = tokio::process::Command::new("tmux")
                    .args([
                        "display-message",
                        "-t",
                        &current_name,
                        &format!("Could not switch to session {target}"),
                    ])
                    .status()
                    .await;
            }
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

    #[tokio::test]
    async fn attach_backend_session_unknown_query_is_not_found() {
        // A query that resolves to no session must fail with NotFound *before*
        // the loop touches the terminal — the mock's `session_detail` returns
        // `None`, so the early return fires and raw mode is never entered.
        use crate::backend::mock::MockBackend;
        use crate::backend::{BackendError, empty_snapshot};

        let backend: Arc<dyn crate::backend::CommanderBackend> =
            Arc::new(MockBackend::new("test", empty_snapshot()));
        let err = attach_backend_session(backend, "does-not-exist", Vec::new())
            .await
            .expect_err("unknown session query must not attach");
        assert!(
            matches!(err, BackendError::NotFound),
            "expected NotFound, got {err:?}"
        );
    }

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

    /// Classify with the standard trigger set and Ctrl+Z interception on. Image
    /// paste is off (the local-attach default), matching the historical calls.
    fn classify(data: &[u8], popup_open: bool) -> InputAction {
        classify_input(
            data,
            popup_open,
            &voice(),
            &review(),
            &editor(),
            true,
            false,
        )
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
        let action = classify_input(
            b"a\x1ab",
            false,
            &voice(),
            &review(),
            &editor(),
            false,
            false,
        );
        assert_eq!(action, InputAction::Forward(b"a\x1ab".to_vec()));
    }

    #[test]
    fn classify_empty_triggers_disable_review_and_editor() {
        // With no review/editor triggers configured, those bytes are forwarded
        // as ordinary input rather than intercepted.
        let action = classify_input(b"\x1br", false, &voice(), &[], &[], true, false);
        assert_eq!(action, InputAction::Forward(b"\x1br".to_vec()));
        let action = classify_input(b"\x05", false, &voice(), &[], &[], true, false);
        assert_eq!(action, InputAction::Forward(b"\x05".to_vec()));
    }

    #[test]
    fn classify_voice_precedes_review_when_both_match() {
        // Ordering: voice is checked before review. A burst containing both
        // triggers toggles voice (and strips it) rather than breaking to review.
        let action = classify_input(
            b"\x1bv\x1br",
            false,
            &voice(),
            &review(),
            &editor(),
            true,
            false,
        );
        assert_eq!(action, InputAction::ToggleVoice(b"\x1br".to_vec()));
    }

    #[test]
    fn classify_ctrl_v_pastes_image_when_enabled() {
        // With image paste enabled (remote attach), Ctrl+V (0x16) is intercepted
        // and the original burst carried through for the effectful handler.
        let action = classify_input(b"\x16", false, &voice(), &review(), &editor(), true, true);
        assert_eq!(action, InputAction::PasteImage(b"\x16".to_vec()));
        // Mixed with other bytes, the whole burst rides along (0x16 not stripped
        // here — the handler decides).
        let action = classify_input(b"a\x16b", false, &voice(), &review(), &editor(), true, true);
        assert_eq!(action, InputAction::PasteImage(b"a\x16b".to_vec()));
    }

    #[test]
    fn classify_ctrl_v_forwards_when_disabled() {
        // Local attach (image paste off): Ctrl+V is forwarded verbatim so the
        // co-located agent reads the clipboard itself — unchanged behaviour.
        let action = classify_input(b"\x16", false, &voice(), &review(), &editor(), true, false);
        assert_eq!(action, InputAction::Forward(b"\x16".to_vec()));
    }

    #[test]
    fn classify_ctrl_v_does_not_shadow_detach() {
        // Ctrl+Q still detaches even in a burst that also contains Ctrl+V, since
        // the detach check precedes the paste check.
        let action = classify_input(
            b"\x16\x11",
            false,
            &voice(),
            &review(),
            &editor(),
            true,
            true,
        );
        assert_eq!(action, InputAction::Break(AttachResult::Detached));
    }

    #[test]
    fn classify_ctrl_v_forwarded_verbatim_when_popup_open() {
        // With the switcher popup open, every byte (including 0x16) is forwarded
        // untouched — image paste must not fire while the picker owns input.
        let action = classify_input(b"\x16", true, &voice(), &review(), &editor(), true, true);
        assert_eq!(action, InputAction::Forward(b"\x16".to_vec()));
    }

    // -- paste_decision: the strip/forward contract for a Ctrl+V burst --

    #[test]
    fn paste_decision_uploads_and_strips_on_capture() {
        // An image within the cap → upload it and forward the burst with 0x16
        // removed (order otherwise preserved).
        let png = vec![0u8; 16];
        assert_eq!(
            paste_decision(Ok(Some(png.clone())), b"a\x16b"),
            PasteDecision::Upload {
                png: png.clone(),
                forward: b"ab".to_vec(),
            }
        );
        // A lone Ctrl+V uploads and forwards nothing.
        assert_eq!(
            paste_decision(Ok(Some(png.clone())), b"\x16"),
            PasteDecision::Upload {
                png,
                forward: vec![],
            }
        );
    }

    #[test]
    fn paste_decision_forwards_verbatim_when_no_image() {
        // Empty clipboard → forward Ctrl+V unchanged (local-attach behaviour).
        assert_eq!(
            paste_decision(Ok(None), b"\x16"),
            PasteDecision::Forward(b"\x16".to_vec())
        );
    }

    #[test]
    fn paste_decision_forwards_verbatim_on_capture_error() {
        // Clipboard read failed → forward Ctrl+V unchanged, never swallow it.
        assert_eq!(
            paste_decision(Err("x11 unavailable".into()), b"x\x16y"),
            PasteDecision::Forward(b"x\x16y".to_vec())
        );
    }

    #[test]
    fn paste_decision_forwards_verbatim_when_over_size_cap() {
        // An over-limit image is not uploaded (the doomed transfer is skipped);
        // Ctrl+V is forwarded verbatim.
        let too_big = vec![0u8; crate::paste_image::MAX_IMAGE_BYTES + 1];
        assert_eq!(
            paste_decision(Ok(Some(too_big)), b"\x16"),
            PasteDecision::Forward(b"\x16".to_vec())
        );
    }
}
