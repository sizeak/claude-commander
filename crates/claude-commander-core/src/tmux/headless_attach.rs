//! Transport-agnostic tmux attach bridge.
//!
//! Spawns `tmux attach-session -t <name>` inside a PTY and exposes the **raw**
//! PTY reader/writer halves, a [`ResizeHandle`], and a [`ChildGuard`] that reaps
//! the `tmux attach-session` child when dropped (or killed explicitly). It knows
//! nothing about *where* bytes flow: no stdin/stdout, no SIGWINCH, no hotkeys,
//! and no intermediate channel on the data path.
//!
//! Two adapters consume this bridge:
//! - the local TUI/CLI ([`super::attach_to_session`]), which pumps the PTY
//!   halves directly to the process stdin/stdout with raw-mode + hotkey
//!   interception layered on top;
//! - the remote server's WebSocket handler, which bridges socket frames to the
//!   PTY halves.
//!
//! Keeping the spawn/resize/reaping in one place means both transports share the
//! same lifecycle and there is exactly one copy of the PTY plumbing.

use std::os::unix::io::{AsRawFd, RawFd};
use std::path::Path;

use tokio::io::{ReadHalf, WriteHalf};
use tracing::{info, warn};

use crate::error::Result;
use crate::tmux::isolation::TmuxTmpdir;

/// A live `tmux attach-session` running inside a PTY.
///
/// Holds the PTY (for I/O + resize) and the child process. Use [`Self::split`]
/// to break it into the independently-ownable halves an async I/O loop needs.
pub struct HeadlessAttach {
    pty: pty_process::Pty,
    child: tokio::process::Child,
}

impl HeadlessAttach {
    /// Spawn `tmux attach-session -t <session_name>` in a fresh PTY sized to
    /// `cols`×`rows`.
    ///
    /// `tmux_tmpdir` isolates the attach client onto a throwaway socket dir (for
    /// hermetic tests/e2e — see [`Config::tmux_tmpdir`](crate::config::Config::tmux_tmpdir));
    /// pass `None` for normal use, which leaves the environment untouched. It
    /// must match the socket dir the target session was created on, or the
    /// client attaches to the wrong server.
    pub fn spawn(
        session_name: &str,
        cols: u16,
        rows: u16,
        tmux_tmpdir: Option<&Path>,
    ) -> Result<Self> {
        let (pty, pts) = pty_process::open()?;
        pty.resize(pty_process::Size::new(rows, cols))?;

        let cmd = pty_process::Command::new("tmux")
            .args(["attach-session", "-t", session_name])
            .with_tmux_tmpdir(tmux_tmpdir);
        let child = cmd.spawn(pts)?;

        info!("Spawned tmux attach-session for {}", session_name);

        Ok(Self { pty, child })
    }

    /// A handle that can resize the PTY without owning it. Cloneable; safe to
    /// move into a separate task (e.g. one driven by SIGWINCH or a `resize`
    /// control frame).
    pub fn resize_handle(&self) -> ResizeHandle {
        ResizeHandle {
            fd: self.pty.as_raw_fd(),
        }
    }

    /// Break the bridge into its independently-ownable parts:
    /// - the raw PTY reader half (PTY output → transport),
    /// - the raw PTY writer half (transport → PTY input),
    /// - a [`ResizeHandle`] for out-of-band resizes,
    /// - a [`ChildGuard`] that reaps the `tmux attach-session` child.
    ///
    /// No channel or extra copy sits between the halves and the PTY, so a
    /// consumer that pumps `reader.read() → out` and `in → writer` has the same
    /// latency/throughput as touching the PTY directly.
    pub fn split(
        self,
    ) -> (
        ReadHalf<pty_process::Pty>,
        WriteHalf<pty_process::Pty>,
        ResizeHandle,
        ChildGuard,
    ) {
        let resize = self.resize_handle();
        let (reader, writer) = tokio::io::split(self.pty);
        let guard = ChildGuard { child: self.child };
        (reader, writer, resize, guard)
    }
}

/// Resizes a PTY by raw fd via the `TIOCSWINSZ` ioctl. Holds only the fd, so it
/// is cheap to copy and can live in a different task than the PTY halves.
#[derive(Debug, Clone, Copy)]
pub struct ResizeHandle {
    fd: RawFd,
}

impl ResizeHandle {
    /// Resize the PTY to `cols`×`rows`.
    pub fn resize(&self, cols: u16, rows: u16) {
        use nix::libc::{TIOCSWINSZ, ioctl, winsize};

        let ws = winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // SAFETY: fd comes from a live PTY (`Pty::as_raw_fd`) that outlives this
        // handle in practice; `ws` is a valid stack pointer for the call.
        let rc = unsafe { ioctl(self.fd, TIOCSWINSZ, &ws) };
        if rc != 0 {
            // A resize failure is non-fatal (the PTY keeps its previous size),
            // but worth surfacing rather than silently swallowing.
            warn!(
                "PTY resize ioctl(TIOCSWINSZ) to {cols}x{rows} failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

/// Owns the `tmux attach-session` child and reaps it.
///
/// Dropping the guard kills the child (best-effort, non-blocking) so a consumer
/// that simply drops the bridge — e.g. a closed browser tab — never leaks an
/// attach process. **This detaches, not kills**: only the `tmux attach-session`
/// client process dies; the tmux *session* and the program inside it keep
/// running, exactly like pressing the tmux detach key. For deterministic
/// teardown in an async context, prefer [`Self::kill`] before dropping.
pub struct ChildGuard {
    child: tokio::process::Child,
}

impl ChildGuard {
    /// Kill the `tmux attach-session` child and await its exit. Detaches the
    /// client; leaves the tmux session + its program running.
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }

    /// Wait for the child to exit on its own (e.g. the user pressed tmux's
    /// detach key, or the session ended) and return its exit status. Idempotent
    /// once the child has exited.
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Best-effort synchronous reap. `start_kill` only signals; the child is
        // reaped by tokio's background machinery. This is the safety net for
        // ungraceful drops — graceful paths should call `kill().await`.
        let _ = self.child.start_kill();
    }
}
