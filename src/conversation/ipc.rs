//! Cross-platform local IPC for triggering voice input from outside the TUI.
//!
//! A terminal app can't capture global hotkeys itself (terminal key events need
//! window focus; Wayland has no client-side global-shortcut API). The portable
//! route is a desktop-level global shortcut that runs a command which signals
//! the already-running TUI. The signal travels over a Unix-domain socket — the
//! one IPC primitive shared by Linux and macOS. (On Linux we additionally serve
//! the same toggle over D-Bus; see [`dbus`](super::dbus).)
//!
//! The server ([`serve`]) feeds [`apply_listen_action`] with the shared listener
//! channel + recording flag — the exact same core the in-app Alt-V key path and
//! the in-attach byte interceptor use — so an external toggle behaves
//! identically and works even while the main loop is parked in a tmux attach.
//! The client ([`send_command`] / [`send_default`]) backs the
//! `claude-commander listen-toggle` subcommand.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use crate::conversation::{ListenAction, ListenerCommand, apply_listen_action};

/// Default per-user socket path. Prefers `$XDG_RUNTIME_DIR` (per-user on Linux),
/// falling back to the OS temp dir (`$TMPDIR`, per-user on macOS). Both the TUI
/// server and the `listen-toggle` client derive the path the same way.
pub fn default_socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("claude-commander.sock")
    } else {
        std::env::temp_dir().join("claude-commander.sock")
    }
}

/// Map a one-line wire command to a [`ListenAction`].
fn parse_action(line: &str) -> Option<ListenAction> {
    match line.trim() {
        "toggle" => Some(ListenAction::Toggle),
        "start" => Some(ListenAction::Start),
        "stop" => Some(ListenAction::Stop),
        _ => None,
    }
}

/// The wire word for an action (client side).
fn action_word(action: ListenAction) -> &'static str {
    match action {
        ListenAction::Toggle => "toggle",
        ListenAction::Start => "start",
        ListenAction::Stop => "stop",
    }
}

/// Bind the socket (cleaning up a stale one from a crashed instance) and spawn
/// the accept loop. Returns the bound path. An `AddrInUse` error with a *live*
/// peer means another instance owns the socket — surfaced as an error so the
/// caller can log and skip rather than stealing it.
pub fn serve(
    path: PathBuf,
    listener: UnboundedSender<ListenerCommand>,
    recording: Arc<AtomicBool>,
) -> std::io::Result<PathBuf> {
    let l = bind_with_cleanup(&path)?;
    info!(target: "conversation", "voice IPC listening on {}", path.display());
    tokio::spawn(run_accept_loop(l, listener, recording));
    Ok(path)
}

/// Bind synchronously, removing a stale socket file if no peer is listening.
fn bind_with_cleanup(path: &Path) -> std::io::Result<UnixListener> {
    use std::os::unix::net::{UnixListener as StdListener, UnixStream as StdStream};

    let into_tokio = |std_l: StdListener| -> std::io::Result<UnixListener> {
        std_l.set_nonblocking(true)?;
        UnixListener::from_std(std_l)
    };

    match StdListener::bind(path) {
        Ok(std_l) => into_tokio(std_l),
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            // A leftover file blocks bind. If nothing answers a connect, it's
            // stale (previous instance crashed) — remove it and rebind. If a
            // peer answers, a live instance owns it: don't steal.
            if StdStream::connect(path).is_err() {
                debug!(target: "conversation", "removing stale voice IPC socket {}", path.display());
                std::fs::remove_file(path)?;
                into_tokio(StdListener::bind(path)?)
            } else {
                Err(e)
            }
        }
        Err(e) => Err(e),
    }
}

async fn run_accept_loop(
    l: UnixListener,
    listener: UnboundedSender<ListenerCommand>,
    recording: Arc<AtomicBool>,
) {
    loop {
        match l.accept().await {
            Ok((stream, _addr)) => {
                let listener = listener.clone();
                let recording = recording.clone();
                tokio::spawn(handle_conn(stream, listener, recording));
            }
            Err(e) => {
                warn!(target: "conversation", "voice IPC accept failed: {e}");
                break;
            }
        }
    }
}

/// Read one command line, apply it, and write back the resulting state.
async fn handle_conn(
    stream: UnixStream,
    listener: UnboundedSender<ListenerCommand>,
    recording: Arc<AtomicBool>,
) {
    let (read_half, mut write_half) = stream.into_split();
    let mut line = String::new();
    if BufReader::new(read_half)
        .read_line(&mut line)
        .await
        .is_err()
    {
        return;
    }
    let reply = match parse_action(&line) {
        Some(action) => {
            if apply_listen_action(&listener, &recording, action) {
                "recording\n"
            } else {
                "stopped\n"
            }
        }
        None => "error: unknown command\n",
    };
    let _ = write_half.write_all(reply.as_bytes()).await;
}

/// Send a command to the default socket and return the one-line status reply.
pub async fn send_default(action: ListenAction) -> std::io::Result<String> {
    send_command(&default_socket_path(), action).await
}

/// Connect to the TUI's IPC socket, send `action`, and return its status reply.
pub async fn send_command(path: &Path, action: ListenAction) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(path).await?;
    stream
        .write_all(format!("{}\n", action_word(action)).as_bytes())
        .await?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await?;
    Ok(reply.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn parse_action_known_and_unknown() {
        assert_eq!(parse_action("toggle\n"), Some(ListenAction::Toggle));
        assert_eq!(parse_action(" start "), Some(ListenAction::Start));
        assert_eq!(parse_action("stop"), Some(ListenAction::Stop));
        assert_eq!(parse_action("frobnicate"), None);
    }

    #[tokio::test]
    async fn socket_round_trip_toggles_recording() {
        // Isolated temp socket — never touches a real runtime dir.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cc-test.sock");
        let (tx, mut rx) = unbounded_channel();
        let recording = Arc::new(AtomicBool::new(false));

        // Bind happens synchronously inside serve(), so there's no accept race.
        serve(path.clone(), tx, recording.clone()).expect("bind");

        let reply = send_command(&path, ListenAction::Toggle)
            .await
            .expect("send");
        assert_eq!(reply, "recording");
        assert!(recording.load(Ordering::Acquire));
        assert!(matches!(rx.try_recv(), Ok(ListenerCommand::Start)));

        let reply = send_command(&path, ListenAction::Toggle)
            .await
            .expect("send");
        assert_eq!(reply, "stopped");
        assert!(!recording.load(Ordering::Acquire));
        assert!(matches!(rx.try_recv(), Ok(ListenerCommand::Stop)));
    }

    #[tokio::test]
    async fn stale_socket_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cc-stale.sock");
        // Leave a leftover file with no listener behind it.
        std::fs::write(&path, b"").unwrap();

        let (tx, _rx) = unbounded_channel();
        let recording = Arc::new(AtomicBool::new(false));
        // Should remove the stale file and bind successfully.
        serve(path.clone(), tx, recording).expect("reclaim stale socket");

        let reply = send_command(&path, ListenAction::Start)
            .await
            .expect("send");
        assert_eq!(reply, "recording");
    }
}
