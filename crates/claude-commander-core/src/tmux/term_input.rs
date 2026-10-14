//! A cancellable reader for the operator's terminal.
//!
//! The local attach forwards keystrokes from the terminal to the tmux client,
//! and it used to read them through `tokio::io::stdin()`. That reader is a
//! blocking `read(0)` on tokio's blocking pool, and **aborting the task that
//! awaits it does not cancel the syscall** — tokio's own docs: "`stdin` is
//! implemented by using an ordinary blocking read on a separate thread, and it
//! is impossible to cancel that read" (`tokio-1.52.3/src/io/stdin.rs:16-19`).
//! The thread stays inside `read()` until the next burst arrives, consumes it,
//! and throws it away. So every attach that ended left one orphaned read armed
//! on the terminal, and whoever read next — crossterm, once the TUI restarted
//! its event reader — got only what the orphan left. A burst split that way is
//! how an SGR mouse report (`ESC [ < 35;120;30 M`) turns into the keystrokes
//! `<`, `3`, `5`, …
//!
//! [`TtyReader`] reads a **non-blocking** descriptor through tokio's
//! [`AsyncFd`], so cancelling the read is dropping a future: nothing is in
//! flight in the kernel, and the next reader sees every byte. It opens the
//! terminal *afresh* rather than setting `O_NONBLOCK` on fd 0: the flag lives on
//! the open file description, not the descriptor (`fcntl(2)`, F_SETFL), and a
//! shell hands a program fds 0, 1 and 2 as three `dup`s of one description — so
//! flagging fd 0 would flag stdout too, and `write()` would start failing with
//! `EAGAIN` under a full terminal buffer. A fresh `open()` is its own
//! description; the flag stays put.
//!
//! The path opened is the terminal's *name* (`ttyname(0)`, e.g. `/dev/pts/2`),
//! not the `/dev/tty` alias. Both reach the same terminal on Linux, but macOS's
//! kqueue refuses to poll `/dev/tty` (`EINVAL`; mio #1377, crossterm #500, and
//! why libuv's `uv_tty_init` reopens by `ttyname_r`), so `/dev/tty` is only the
//! fallback for when no std stream is a terminal.
//!
//! The terminal itself is still one line discipline. Raw mode set through fd 0
//! applies here too, `tcflush` on fd 0 discards bytes this reader would have
//! seen, and two live readers on it still race — this type makes the handoff
//! *clean*, it does not make concurrent readers safe.

use std::fs::File;
use std::io::{self, IsTerminal, Read};
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, ReadBuf};
use tracing::{debug, warn};

/// Set `O_NONBLOCK` on `fd`'s open file description (see the module docs for
/// why that must be a description nothing else shares).
fn set_nonblocking(fd: &impl AsFd) -> io::Result<()> {
    let flags = fcntl(fd, FcntlArg::F_GETFL)?;
    let flags = OFlag::from_bits_retain(flags) | OFlag::O_NONBLOCK;
    fcntl(fd, FcntlArg::F_SETFL(flags))?;
    Ok(())
}

/// The path to open for the controlling terminal: the name of whichever std
/// stream is a terminal, else the `/dev/tty` alias.
fn terminal_path() -> PathBuf {
    let stdin = io::stdin();
    let stdout = io::stdout();
    [stdin.as_fd(), stdout.as_fd()]
        .into_iter()
        .find_map(|fd| nix::unistd::ttyname(fd).ok())
        .unwrap_or_else(|| PathBuf::from("/dev/tty"))
}

/// Non-blocking, cancellable reads from a terminal descriptor.
pub struct TtyReader {
    fd: AsyncFd<File>,
}

impl TtyReader {
    /// Open the controlling terminal for reading.
    ///
    /// Fails when the process has no controlling terminal (a headless run, or
    /// a test) — callers fall back to stdin, see [`terminal_input`].
    pub fn open() -> io::Result<Self> {
        let file = File::open(terminal_path())?;
        Self::from_fd(file.into())
    }

    /// Wrap an already-open terminal descriptor, switching it to non-blocking.
    ///
    /// The flag is set on `fd`'s open file description, so hand this a
    /// descriptor that is not shared with anything expecting blocking I/O.
    pub fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        set_nonblocking(&fd)?;
        Ok(Self {
            fd: AsyncFd::new(File::from(fd))?,
        })
    }
}

impl AsyncRead for TtyReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            let mut guard = match this.fd.poll_read_ready(cx) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };
            // Read straight into the caller's buffer. `try_io` clears readiness
            // itself when the read comes back `WouldBlock`
            // (`tokio-1.52.3/src/io/async_fd.rs:1158-1159`), so the next
            // `poll_read_ready` waits for a fresh edge.
            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|inner| inner.get_ref().read(unfilled)) {
                Ok(Ok(n)) => {
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(e)) => return Poll::Ready(Err(e)),
                Err(_would_block) => continue,
            }
        }
    }
}

/// The reader the local attach forwards keystrokes from.
///
/// - stdin is a terminal (the interactive case): a [`TtyReader`] on that
///   terminal, opened afresh so its read is cancellable.
/// - stdin is a pipe or file: `tokio::io::stdin()`, so `printf 'ls\n' |
///   claude-commander attach …` still forwards the piped bytes to the pane. No
///   crossterm reader shares a pipe, so the orphaned-read hazard is moot there.
/// - stdin is a terminal but it can't be opened afresh: `tokio::io::stdin()`
///   again, with a `warn!` naming the consequence, because that path *does*
///   reintroduce the hazard described in the module docs.
pub fn terminal_input() -> Box<dyn AsyncRead + Send + Unpin> {
    if !io::stdin().is_terminal() {
        debug!("stdin is not a terminal; the attach forwards it as-is");
        return Box::new(tokio::io::stdin());
    }
    match TtyReader::open() {
        Ok(reader) => Box::new(reader),
        Err(e) => {
            warn!(
                "could not reopen the terminal for the attach ({e}); falling back to stdin, \
                 whose read cannot be cancelled — the first keystrokes after each detach may be lost"
            );
            Box::new(tokio::io::stdin())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    use nix::pty::openpty;
    use nix::sys::termios::{SetArg, cfmakeraw, tcgetattr, tcsetattr};
    use tokio::io::AsyncReadExt;

    /// A raw-mode pty pair: writes to `master` arrive on `slave` byte-for-byte.
    /// The master stays a plain blocking `File` — a few bytes never block it.
    fn raw_pty() -> (File, OwnedFd) {
        let pty = openpty(None, None).expect("openpty");
        let mut termios = tcgetattr(&pty.slave).expect("tcgetattr");
        cfmakeraw(&mut termios);
        tcsetattr(&pty.slave, SetArg::TCSANOW, &termios).expect("tcsetattr");
        (File::from(pty.master), pty.slave)
    }

    fn is_nonblocking(fd: &impl AsFd) -> bool {
        let flags = fcntl(fd, FcntlArg::F_GETFL).expect("F_GETFL");
        OFlag::from_bits_retain(flags).contains(OFlag::O_NONBLOCK)
    }

    async fn read_some(reader: &mut TtyReader) -> Vec<u8> {
        let mut buf = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(2), reader.read(&mut buf))
            .await
            .expect("a byte should arrive")
            .expect("read");
        buf[..n].to_vec()
    }

    #[tokio::test]
    async fn bytes_written_to_the_terminal_are_read() {
        let (mut master, slave) = raw_pty();
        let mut reader = TtyReader::from_fd(slave).unwrap();
        master.write_all(b"hi").unwrap();
        assert_eq!(read_some(&mut reader).await, b"hi");
    }

    /// The regression this module exists for: a reader whose task was aborted
    /// while parked must not consume the bytes that arrive afterwards. With
    /// `tokio::io::stdin()` the orphaned blocking `read()` eats the next burst;
    /// here cancelling is dropping a future, so the next reader gets it all.
    #[tokio::test]
    async fn an_aborted_read_leaves_the_next_bytes_for_the_next_reader() {
        let (mut master, slave) = raw_pty();
        let shared = slave.try_clone().expect("dup the terminal fd");
        let mut parked = TtyReader::from_fd(shared).unwrap();
        let mut next = TtyReader::from_fd(slave).unwrap();

        let task = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let _ = parked.read(&mut buf).await;
        });
        // Let the task reach its await and park on readiness.
        tokio::time::sleep(Duration::from_millis(50)).await;
        task.abort();
        let _ = task.await;

        master.write_all(b"\x1b[<35;120;30M").unwrap();

        assert_eq!(
            read_some(&mut next).await,
            b"\x1b[<35;120;30M",
            "the whole report must reach the live reader intact"
        );
    }

    #[tokio::test]
    async fn from_fd_switches_the_descriptor_to_nonblocking() {
        let (_master, slave) = raw_pty();
        assert!(!is_nonblocking(&slave), "openpty hands out blocking fds");
        let reader = TtyReader::from_fd(slave).unwrap();
        assert!(is_nonblocking(reader.fd.get_ref()));
    }

    /// The flag must land on the reader's own open file description: a `dup` of
    /// the descriptor shares it (and so flips too), an independent `open` of the
    /// same terminal does not. This is the property that keeps fd 0/1/2 blocking
    /// when the attach reopens the terminal by name.
    #[tokio::test]
    async fn nonblocking_stays_on_the_readers_own_open_file_description() {
        let (_master, slave) = raw_pty();
        let name = nix::unistd::ttyname(&slave).expect("pty slave has a name");
        let independent = File::open(&name).expect("open the same terminal afresh");
        let dup = slave.try_clone().unwrap();

        let _reader = TtyReader::from_fd(slave).unwrap();

        assert!(is_nonblocking(&dup), "a dup shares the description");
        assert!(
            !is_nonblocking(&independent),
            "a separate open of the same terminal must stay blocking"
        );
    }
}
