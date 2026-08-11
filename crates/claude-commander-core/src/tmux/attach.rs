//! Async PTY-based tmux session attachment
//!
//! Provides fully async terminal attachment that runs within the tokio runtime,
//! avoiding the need to drop and recreate the runtime for each attach operation.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, Notify, mpsc};
use tracing::{debug, info, warn};

use crate::backend::{AttachEnd, AttachRefresher, AttachResizer, AttachStreams, AttachTerminator};
use crate::error::Result;

/// Classification of a raw stdin burst by the local attach's keystroke
/// interception state machine. Pure — it performs no I/O and no side effects,
/// so it can be characterization-tested in isolation. The stdin task maps each
/// variant to the corresponding action (forward bytes / break with an
/// [`AttachResult`] / toggle voice / open the switcher).
///
/// The classification order is significant: Ctrl+Q, Ctrl+\, Ctrl+Space, voice,
/// review, editor, and finally plain forwarding (with optional Ctrl+Z
/// stripping).
#[derive(Debug, PartialEq, Eq)]
enum InputAction {
    /// Forward these bytes to the PTY verbatim and keep looping. An empty
    /// vec means "swallow entirely, forward nothing".
    Forward(Vec<u8>),
    /// Ctrl+Space: suspend the attach so the frontend can draw the switcher
    /// over the pane, then forward the remaining bytes (the 0x00 stripped out;
    /// may be empty).
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
    switcher_enabled: bool,
    voice_triggers: &[Vec<u8>],
    review_triggers: &[Vec<u8>],
    editor_triggers: &[Vec<u8>],
    intercept_ctrl_z: bool,
    image_paste_enabled: bool,
) -> InputAction {
    // Ctrl+Q (0x11) anywhere → detach.
    if data.contains(&0x11) {
        return InputAction::Break(AttachResult::Detached);
    }

    // Ctrl+\ (0x1C) → toggle to the shell session.
    if data.contains(&0x1C) {
        return InputAction::Break(AttachResult::SwitchToShell);
    }

    // Ctrl+Space (0x00) → suspend so the frontend can draw the switcher over
    // the pane; swallow the 0x00 byte and forward the rest.
    //
    // Gated here rather than at the effect site so that a frontend with no
    // switcher to show (the bare `attach` CLI) forwards the NUL to the pane
    // *verbatim*, which is what its config has always claimed to do.
    if switcher_enabled && data.contains(&0x00) {
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
        Ok(Some(png)) if png.len() <= claude_commander_protocol::paste::MAX_IMAGE_BYTES => {
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
        Ok(Some(png)) if png.len() > claude_commander_protocol::paste::MAX_IMAGE_BYTES => warn!(
            "clipboard image {} bytes exceeds {} limit; not uploading",
            png.len(),
            claude_commander_protocol::paste::MAX_IMAGE_BYTES
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
    /// The in-session switcher (Ctrl+Space). Only ever produced when
    /// [`AttachConfig::switcher_enabled`] is set, and it means two different
    /// things depending on where it is read:
    ///
    /// - from [`AttachSession::run`], the attach is **suspended, not ended** —
    ///   the I/O pumps are parked and the frontend owns the terminal until it
    ///   calls [`AttachSession::resume`];
    /// - on an [`AttachOutcome`], the attach **is** over, and the switcher is
    ///   why: the frontend ended it to act on the user's pick (a session on
    ///   another backend, which `tmux switch-client` cannot reach) or on a
    ///   command that needs the full UI back.
    OpenSwitcher,
    /// The session/process ended
    SessionEnded,
    /// An error occurred during attachment
    Error(String),
}

/// Outcome of an attach. `final_session` is the tmux session the client was
/// attached to when the attach loop exited — usually the session passed in, but
/// updated when the user picks a different one from the in-session switcher
/// (Ctrl+Space), which reaches a local session by running `tmux switch-client`
/// mid-attach rather than re-attaching.
#[derive(Debug)]
pub struct AttachOutcome {
    pub result: AttachResult,
    pub final_session: String,
}

/// Configuration for one interactive attach, driven by [`run_attach`] or
/// [`AttachSession`]. Bundles the keystroke-interception policy and the optional
/// affordances (the in-session switcher, voice) so the transport-agnostic loop
/// keeps a single signature.
///
/// `editor_triggers` is a list of byte patterns that, when seen on stdin, cause
/// the attach loop to exit with [`AttachResult::OpenEditor`]. Callers compute
/// these from the user's `OpenInEditor` keybindings — typically a single
/// control byte for `Ctrl-<letter>` bindings, or CSI-u/modifyOtherKeys
/// sequences for `Ctrl-<non-letter>` bindings. Bindings that cannot be detected
/// in raw stdin (e.g. a bare letter) should simply be omitted.
///
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
    /// Whether Ctrl+Space suspends the attach so the frontend can draw the
    /// switcher over the pane ([`AttachResult::OpenSwitcher`]). Requires a
    /// frontend with a session list to show, so the TUI sets it and the bare
    /// `attach` CLI does not — with it off, Ctrl+Space reaches the pane as a
    /// plain NUL. Unlike the `tmux display-popup` switcher this replaced, it is
    /// **not** a local-only capability: the frontend renders it, so it works
    /// over a remote attach too.
    pub switcher_enabled: bool,
    /// The tmux session name currently attached, for the switcher and the voice
    /// feedback `tmux display-message`. The TUI sets this for remote attaches
    /// too (the session's tmux name rides in on the wire), so it's normally
    /// `Some`. The voice `display-message` is best-effort — it runs against the
    /// operator's local tmux, so for a remote session it may simply target a
    /// name that isn't there.
    pub session_name: Option<String>,
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
        // The bare CLI attach has no session list to draw over the pane, so
        // Ctrl+Space belongs to the agent running there.
        switcher_enabled: false,
        session_name: Some(session_name.to_string()),
        // The CLI/local attach runs the agent on this machine, so it reads the
        // local clipboard itself — no client-side capture.
        image_paste: None,
    };
    run_attach(streams, cfg).await
}

/// Clipboard-image sink that forwards a captured PNG through a
/// [`CommanderBackend`](crate::backend::CommanderBackend)'s `paste_image` route.
/// The canonical [`ImagePasteSink`] used by every frontend attaching to a
/// *remote* session (the TUI and the CLI both construct it via [`Self::sink`]).
pub struct BackendImagePaste {
    backend: Arc<dyn crate::backend::CommanderBackend>,
    id: crate::session::SessionId,
}

impl BackendImagePaste {
    /// Build a sink that uploads captured clipboard images to `backend` for the
    /// session `id`, boxed as the [`ImagePasteSink`] trait object the attach
    /// loop's `image_paste` slot expects. Named `sink` rather than `new` because
    /// it returns the trait object, not `Self`.
    pub fn sink(
        backend: Arc<dyn crate::backend::CommanderBackend>,
        id: crate::session::SessionId,
    ) -> Arc<dyn ImagePasteSink> {
        Arc::new(Self { backend, id })
    }
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
        .then(|| BackendImagePaste::sink(backend.clone(), id));

    let cfg = AttachConfig {
        editor_triggers,
        review_triggers: Vec::new(),
        voice_triggers: Vec::new(),
        voice_listener: None,
        recording: Arc::new(AtomicBool::new(false)),
        intercept_ctrl_z: true,
        // Same as the local CLI attach: no session list to draw, so Ctrl+Space
        // is the remote agent's.
        switcher_enabled: false,
        session_name: None,
        image_paste,
    };

    Ok(run_attach(streams, cfg).await?)
}

/// Drive one interactive attach to completion over transport-agnostic
/// [`AttachStreams`] (a local PTY, or a remote WebSocket via the backend).
///
/// The straight-through path for frontends that cannot suspend an attach: it
/// starts an [`AttachSession`], runs it until something ends it, and tears it
/// down. Callers that set [`AttachConfig::switcher_enabled`] must drive
/// [`AttachSession`] themselves instead, since they have to *do* something when
/// it yields [`AttachResult::OpenSwitcher`].
pub async fn run_attach(streams: AttachStreams, cfg: AttachConfig) -> Result<AttachOutcome> {
    debug_assert!(
        !cfg.switcher_enabled,
        "run_attach cannot service OpenSwitcher; drive AttachSession directly"
    );
    let mut session = AttachSession::start(streams, cfg)?;
    let result = session.run().await;
    Ok(session.finish(result).await)
}

/// One interactive attach, which the frontend can **suspend and resume**.
///
/// The in-session switcher draws the real palette over the live pane, so the
/// attach has to hand the terminal back without dying: tearing the PTY down and
/// re-attaching would cost a full detach/reattach round trip on what should be
/// an Alt+Tab-speed keystroke, and would lose the ability to switch with `tmux
/// switch-client`.
///
/// So [`run`](Self::run) returns [`AttachResult::OpenSwitcher`] with everything
/// still alive and the I/O pumps parked. While parked:
///
/// - the **stdin pump stops reading** — it is the whole point, because crossterm
///   reads the terminal through a *separate* fd from the pump's `stdin`: its Unix
///   event source polls `tty_fd()`, which opens `/dev/tty`
///   (`crossterm-0.29.0/src/terminal/sys/file_descriptor.rs:123-150`, used by
///   `event/source/unix/mio.rs:37`). Two readers on one terminal would race for
///   keystrokes;
/// - the **stdout pump keeps draining the transport but discards** what it
///   reads, so the pane looks frozen under the palette rather than painting over
///   it. It must not stop draining, or a blocked pipe would wedge the tmux
///   client.
///
/// [`resume`](Self::resume) unparks both and repaints via the [`AttachRefresher`],
/// which is what restores the region the palette covered.
pub struct AttachSession {
    cfg: AttachConfig,
    terminator: Box<dyn AttachTerminator>,
    refresher: AttachRefresher,
    /// The tmux session the client is on *now*. The frontend updates this after
    /// a `tmux switch-client`, and [`AttachOutcome::final_session`] reports it
    /// back so the caller's later state (shell-toggle pair, editor open) follows
    /// the user rather than where they started.
    current_session: Arc<Mutex<String>>,
    paused: Arc<AtomicBool>,
    resume: Arc<Notify>,
    shutdown_rx: mpsc::Receiver<AttachResult>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    local_client_tty: Option<String>,
}

impl AttachSession {
    /// Enter raw mode and start the I/O pumps. The attach is live from here
    /// until [`Self::finish`].
    pub fn start(streams: AttachStreams, cfg: AttachConfig) -> Result<Self> {
        let AttachStreams {
            reader,
            writer,
            resizer,
            refresher,
            terminator,
            local_client_tty,
        } = streams;

        info!("Enabling raw mode for attach session");
        enable_raw_mode()?;

        let current_session = Arc::new(Mutex::new(cfg.session_name.clone().unwrap_or_default()));
        let paused = Arc::new(AtomicBool::new(false));
        let resume = Arc::new(Notify::new());

        info!("Starting async I/O pumps");
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<AttachResult>(1);
        let tasks = spawn_pumps(
            reader,
            writer,
            resizer,
            &cfg,
            shutdown_tx,
            paused.clone(),
            resume.clone(),
            current_session.clone(),
            tokio::io::stdin(),
            tokio::io::stdout(),
        );

        Ok(Self {
            cfg,
            terminator,
            refresher,
            current_session,
            paused,
            resume,
            shutdown_rx,
            tasks,
            local_client_tty,
        })
    }

    /// Run until an intercepted hotkey fires or the transport ends. Returns
    /// [`AttachResult::OpenSwitcher`] with the attach merely *suspended*; every
    /// other variant means it is over and [`Self::finish`] should follow.
    pub async fn run(&mut self) -> AttachResult {
        tokio::select! {
            result = self.shutdown_rx.recv() => result.unwrap_or(AttachResult::Detached),
            end = self.terminator.wait() => match end {
                AttachEnd::Detached => AttachResult::Detached,
                AttachEnd::SessionEnded => AttachResult::SessionEnded,
                AttachEnd::Error(e) => AttachResult::Error(e),
            },
        }
    }

    /// Un-park the pumps after a suspension and repaint the screen, restoring
    /// whatever the frontend drew over. Safe to call when not suspended.
    pub async fn resume(&mut self) {
        // Discard anything typed at the overlay that the terminal driver still
        // holds, so stray keys don't land in the pane the moment it wakes up.
        flush_stdin();
        self.paused.store(false, Ordering::Release);
        // `notify_one` (not `notify_waiters`) because the stdin pump may not have
        // reached its await yet; this leaves a permit rather than dropping the
        // wake-up.
        self.resume.notify_one();
        self.refresher.refresh().await;
    }

    /// The tty of the operator's own tmux client backing this attach, if any.
    ///
    /// `Some` exactly when the attach is a local PTY one, so this doubles as the
    /// switcher's permission to move the user with `tmux switch-client` — and as
    /// the client to name when it does. See [`AttachStreams::local_client_tty`].
    pub fn local_client_tty(&self) -> Option<&str> {
        self.local_client_tty.as_deref()
    }

    /// The tmux session the client is on right now.
    pub async fn current_session(&self) -> String {
        self.current_session.lock().await.clone()
    }

    /// Record that the client has moved to `name` (after a `tmux switch-client`).
    pub async fn set_current_session(&self, name: impl Into<String>) {
        *self.current_session.lock().await = name.into();
    }

    /// End the attach: stop the pumps, leave raw mode, and detach the transport.
    pub async fn finish(mut self, result: AttachResult) -> AttachOutcome {
        info!("Attach ending with result: {:?}", result);
        for task in self.tasks.drain(..) {
            task.abort();
        }

        info!("Disabling raw mode");
        let _ = disable_raw_mode();
        let _ = std::io::stdout().flush();

        // Flush any leftover input at the kernel level before teardown.
        flush_stdin();
        log_pending_stdin("after first tcflush");

        // Deterministic teardown: kill the attach client (idempotent if it already
        // exited). Detaches the client; the tmux session + program keep running.
        info!("Detaching attach transport");
        self.terminator.detach().await;

        // Flush again after teardown to discard stale input.
        flush_stdin();
        log_pending_stdin("after second tcflush");

        let final_session = self.current_session.lock().await.clone();
        info!(
            "Attach complete, result: {:?}, final session: {}, recording: {}",
            result,
            final_session,
            self.cfg.recording.load(Ordering::Acquire)
        );

        AttachOutcome {
            result,
            final_session,
        }
    }
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
// termination is observed by the owning [`AttachSession`] via its
// [`AttachTerminator`] rather than a concrete child handle.
//
// Returns the spawned pump tasks; the session aborts them on teardown.
//
// `term_in`/`term_out` are the *local* terminal's halves — process stdin/stdout
// in production. They're parameters rather than `tokio::io::stdin()`/`stdout()`
// baked into the body so the suspend/resume behaviour (the one piece of this
// module with real concurrency in it) can be driven over in-memory pipes in a
// test, with no tty and no tmux.
#[allow(clippy::too_many_arguments)]
fn spawn_pumps<R, W>(
    mut reader: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    mut writer: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    resizer: AttachResizer,
    cfg: &AttachConfig,
    shutdown_tx: mpsc::Sender<AttachResult>,
    paused: Arc<AtomicBool>,
    resume: Arc<Notify>,
    current_session: Arc<Mutex<String>>,
    mut term_in: R,
    mut term_out: W,
) -> Vec<tokio::task::JoinHandle<()>>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    // Clone the interception policy out of `cfg` so the spawned tasks can own it.
    let editor_triggers = cfg.editor_triggers.clone();
    let review_triggers = cfg.review_triggers.clone();
    let voice_triggers = cfg.voice_triggers.clone();
    let voice_listener = cfg.voice_listener.clone();
    let recording_flag = cfg.recording.clone();
    let intercept_ctrl_z = cfg.intercept_ctrl_z;
    let switcher_enabled = cfg.switcher_enabled;
    let image_paste = cfg.image_paste.clone();

    // Task 1: transport output -> stdout
    let stdout_shutdown = shutdown_tx.clone();
    let stdout_paused = paused.clone();
    let stdout_task = tokio::spawn(async move {
        let mut buf = [0u8; 4096];

        loop {
            match reader.read(&mut buf).await {
                Ok(0) => {
                    let _ = stdout_shutdown.send(AttachResult::SessionEnded).await;
                    break;
                }
                // While suspended, keep draining the transport but throw the
                // bytes away: the frontend owns the screen, and `resume`'s
                // repaint restores whatever we skipped. Draining (rather than
                // parking) is deliberate — a full pipe would wedge the tmux
                // client behind it.
                Ok(_) if stdout_paused.load(Ordering::Acquire) => continue,
                Ok(n) => {
                    if term_out.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    let _ = term_out.flush().await;
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
        let mut buf = [0u8; 1024];

        loop {
            match term_in.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = &buf[..n];

                    // Classify the burst with the pure interception state
                    // machine; perform the matching side effect here. The order
                    // of checks (Ctrl+Q → Ctrl+\ → Ctrl+Space → voice → review →
                    // editor → forward) lives in `classify_input` and is
                    // characterization-tested.
                    match classify_input(
                        data,
                        switcher_enabled,
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
                            // Forward whatever else was in the burst *before*
                            // parking, so those keystrokes reach the pane rather
                            // than being stranded behind the suspension.
                            if !filtered.is_empty() {
                                if writer.write_all(&filtered).await.is_err() {
                                    break;
                                }
                                let _ = writer.flush().await;
                            }

                            // Suspend: stop reading the terminal so the frontend's
                            // own reader (crossterm, on /dev/tty) has it to itself,
                            // and tell the stdout pump to start discarding. Both
                            // must be in place before the frontend draws.
                            debug!("Ctrl+Space detected, suspending attach for the switcher");
                            paused.store(true, Ordering::Release);
                            if stdin_shutdown
                                .send(AttachResult::OpenSwitcher)
                                .await
                                .is_err()
                            {
                                break;
                            }
                            resume.notified().await;
                            debug!("Attach resumed after the switcher");
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

    vec![stdout_task, stdin_task, resize_task]
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Suspend/resume: the pumps' behaviour while the switcher is up --
    //
    // Driven over in-memory duplex pipes standing in for the terminal and the
    // transport, so there is no tty, no PTY and no tmux involved.

    /// Minimal config for the pump tests: no triggers, switcher on (the TUI's
    /// policy, and the only one where Ctrl+Space suspends).
    fn pump_cfg() -> AttachConfig {
        AttachConfig {
            editor_triggers: Vec::new(),
            review_triggers: Vec::new(),
            voice_triggers: Vec::new(),
            voice_listener: None,
            recording: Arc::new(AtomicBool::new(false)),
            intercept_ctrl_z: false,
            switcher_enabled: true,
            session_name: Some("cc-test".to_string()),
            image_paste: None,
        }
    }

    struct PumpHarness {
        /// Writes here act as the attached pane's output.
        transport_in: tokio::io::DuplexStream,
        /// Writes here act as the user's keystrokes.
        term_in: tokio::io::DuplexStream,
        /// Reads here are what would have been painted on the user's terminal.
        term_out: tokio::io::DuplexStream,
        /// Reads here are the keystrokes forwarded on to the pane.
        transport_out: tokio::io::DuplexStream,
        shutdown_rx: mpsc::Receiver<AttachResult>,
        paused: Arc<AtomicBool>,
        resume: Arc<Notify>,
        refreshes: Arc<std::sync::atomic::AtomicUsize>,
        refresher: AttachRefresher,
        _tasks: Vec<tokio::task::JoinHandle<()>>,
    }

    fn start_pumps() -> PumpHarness {
        let (transport_in, transport_far) = tokio::io::duplex(4096);
        let (term_in, term_far) = tokio::io::duplex(4096);
        let (term_out_far, term_out) = tokio::io::duplex(4096);
        let (transport_out_far, transport_out) = tokio::io::duplex(4096);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let paused = Arc::new(AtomicBool::new(false));
        let resume = Arc::new(Notify::new());
        let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = refreshes.clone();
        let refresher = AttachRefresher::new(move || {
            counter.fetch_add(1, Ordering::Release);
            std::future::ready(())
        });

        let cfg = pump_cfg();
        let tasks = spawn_pumps(
            Box::new(transport_far),
            Box::new(transport_out_far),
            AttachResizer::new(|_, _| {}),
            &cfg,
            shutdown_tx,
            paused.clone(),
            resume.clone(),
            Arc::new(Mutex::new("cc-test".to_string())),
            term_far,
            term_out_far,
        );

        PumpHarness {
            transport_in,
            term_in,
            term_out,
            transport_out,
            shutdown_rx,
            paused,
            resume,
            refreshes,
            refresher,
            _tasks: tasks,
        }
    }

    /// Read whatever lands on a pipe within `ms`, or nothing.
    async fn read_term(term_out: &mut tokio::io::DuplexStream, ms: u64) -> Vec<u8> {
        let mut buf = [0u8; 1024];
        match tokio::time::timeout(
            std::time::Duration::from_millis(ms),
            term_out.read(&mut buf),
        )
        .await
        {
            Ok(Ok(n)) => buf[..n].to_vec(),
            _ => Vec::new(),
        }
    }

    #[tokio::test]
    async fn ctrl_space_suspends_after_forwarding_the_rest_of_the_burst() {
        let mut h = start_pumps();
        // Ctrl+Space arriving mid-burst: the other keystrokes are the user's and
        // belong to the pane, so they must go on ahead rather than being
        // stranded behind the suspension.
        h.term_in.write_all(b"a\x00b").await.unwrap();

        let result =
            tokio::time::timeout(std::time::Duration::from_millis(500), h.shutdown_rx.recv())
                .await
                .expect("Ctrl+Space should signal promptly");
        assert_eq!(result, Some(AttachResult::OpenSwitcher));
        assert!(
            h.paused.load(Ordering::Acquire),
            "the pumps must be parked before the frontend draws"
        );
        assert_eq!(
            read_term(&mut h.transport_out, 500).await,
            b"ab",
            "the rest of the burst reaches the pane, with the NUL swallowed"
        );
    }

    #[tokio::test]
    async fn suspended_pane_output_is_discarded_then_flows_again_on_resume() {
        let mut h = start_pumps();

        // Baseline: while running, pane output reaches the terminal.
        h.transport_in.write_all(b"before").await.unwrap();
        assert_eq!(read_term(&mut h.term_out, 500).await, b"before");

        // Suspend, exactly as the stdin pump does on Ctrl+Space.
        h.paused.store(true, Ordering::Release);
        h.transport_in.write_all(b"hidden").await.unwrap();
        assert!(
            read_term(&mut h.term_out, 200).await.is_empty(),
            "output must not paint over the switcher while it is up"
        );

        // Resume: the transport keeps flowing again. (The bytes swallowed while
        // suspended are not replayed — `refresh-client` repaints instead.)
        h.paused.store(false, Ordering::Release);
        h.resume.notify_one();
        h.transport_in.write_all(b"after").await.unwrap();
        assert_eq!(read_term(&mut h.term_out, 500).await, b"after");
    }

    #[tokio::test]
    async fn suspended_stdin_is_left_for_the_frontends_reader() {
        let mut h = start_pumps();

        // Suspend via the real path so the stdin pump parks itself.
        h.term_in.write_all(b"\x00").await.unwrap();
        assert_eq!(h.shutdown_rx.recv().await, Some(AttachResult::OpenSwitcher));

        // Keystrokes typed at the switcher must be left in the terminal for
        // crossterm to read; the pump consuming them here is exactly the race
        // the suspension exists to prevent.
        h.term_in.write_all(b"query").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let mut peek = [0u8; 16];
        let n = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            h.term_in.read(&mut peek),
        )
        .await
        .map(|r| r.unwrap_or(0))
        .unwrap_or(0);
        assert_eq!(n, 0, "the parked pump must not consume terminal input");
    }

    #[tokio::test]
    async fn each_refresh_runs_the_transport_hook_once() {
        // Covers the refresher plumbing only. `AttachSession::resume` — which is
        // what calls this in anger — can't be exercised here: constructing a
        // session needs `enable_raw_mode`, and so a real tty.
        let h = start_pumps();
        assert_eq!(h.refreshes.load(Ordering::Acquire), 0);
        h.refresher.refresh().await;
        h.refresher.refresh().await;
        assert_eq!(h.refreshes.load(Ordering::Acquire), 2);
    }

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

    /// Classify with the standard trigger set, Ctrl+Z interception on, and the
    /// switcher enabled (the TUI's policy). Image paste is off (the local-attach
    /// default), matching the historical calls.
    fn classify(data: &[u8]) -> InputAction {
        classify_input(data, true, &voice(), &review(), &editor(), true, false)
    }

    /// As [`classify`], but with the switcher off — the bare `attach` CLI, which
    /// has no session list to draw and so leaves Ctrl+Space to the pane.
    fn classify_no_switcher(data: &[u8]) -> InputAction {
        classify_input(data, false, &voice(), &review(), &editor(), true, false)
    }

    #[test]
    fn classify_plain_text_forwards_verbatim() {
        assert_eq!(classify(b"hello"), InputAction::Forward(b"hello".to_vec()));
    }

    #[test]
    fn classify_ctrl_q_detaches() {
        assert_eq!(
            classify(b"\x11"),
            InputAction::Break(AttachResult::Detached)
        );
        // Anywhere in the burst, mixed with other bytes.
        assert_eq!(
            classify(b"ab\x11cd"),
            InputAction::Break(AttachResult::Detached)
        );
    }

    #[test]
    fn classify_ctrl_backslash_switches_to_shell() {
        assert_eq!(
            classify(b"\x1c"),
            InputAction::Break(AttachResult::SwitchToShell)
        );
    }

    #[test]
    fn classify_ctrl_q_precedes_ctrl_backslash() {
        // Ctrl+Q is checked first, so a burst containing both detaches.
        assert_eq!(
            classify(b"\x11\x1c"),
            InputAction::Break(AttachResult::Detached)
        );
    }

    #[test]
    fn classify_ctrl_space_opens_switcher_and_strips_nul() {
        assert_eq!(classify(b"\x00"), InputAction::OpenSwitcher(vec![]));
        // Surrounding bytes survive; only the 0x00 is stripped.
        assert_eq!(
            classify(b"a\x00b"),
            InputAction::OpenSwitcher(b"ab".to_vec())
        );
    }

    #[test]
    fn classify_ctrl_space_reaches_the_pane_when_the_switcher_is_off() {
        // A frontend with no switcher to draw must forward the NUL *verbatim*,
        // which is what `AttachConfig::switcher_enabled`'s docs have always
        // promised. Gating at the effect site instead — as this did before —
        // strips the byte and forwards only the remainder, silently eating the
        // keystroke on every CLI and remote attach.
        assert_eq!(
            classify_no_switcher(b"\x00"),
            InputAction::Forward(b"\x00".to_vec())
        );
        assert_eq!(
            classify_no_switcher(b"a\x00b"),
            InputAction::Forward(b"a\x00b".to_vec())
        );
    }

    #[test]
    fn classify_voice_trigger_toggles_and_strips() {
        // Lone Alt-V burst toggles voice and forwards nothing.
        assert_eq!(classify(b"\x1bv"), InputAction::ToggleVoice(vec![]));
        // Trigger embedded in a burst: stripped, the rest forwarded.
        assert_eq!(
            classify(b"x\x1bvy"),
            InputAction::ToggleVoice(b"xy".to_vec())
        );
    }

    #[test]
    fn classify_review_trigger_breaks() {
        assert_eq!(
            classify(b"\x1br"),
            InputAction::Break(AttachResult::SwitchToReview)
        );
    }

    #[test]
    fn classify_editor_trigger_breaks() {
        assert_eq!(
            classify(b"\x05"),
            InputAction::Break(AttachResult::OpenEditor)
        );
    }

    #[test]
    fn classify_strips_ctrl_z_on_plain_forward_when_enabled() {
        assert_eq!(classify(b"a\x1ab"), InputAction::Forward(b"ab".to_vec()));
        // A lone Ctrl+Z becomes an empty forward (swallowed).
        assert_eq!(classify(b"\x1a"), InputAction::Forward(vec![]));
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
    fn classify_ctrl_v_is_unaffected_by_the_switcher_policy() {
        // The switcher no longer has an "open" state the classifier can see —
        // while it is up the stdin pump is parked and classifies nothing — so
        // Ctrl+V behaves the same either way.
        for switcher_enabled in [true, false] {
            let action = classify_input(
                b"\x16",
                switcher_enabled,
                &voice(),
                &review(),
                &editor(),
                true,
                true,
            );
            assert_eq!(action, InputAction::PasteImage(b"\x16".to_vec()));
        }
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
        let too_big = vec![0u8; claude_commander_protocol::paste::MAX_IMAGE_BYTES + 1];
        assert_eq!(
            paste_decision(Ok(Some(too_big)), b"\x16"),
            PasteDecision::Forward(b"\x16".to_vec())
        );
    }
}
