//! Writing text to the operator's local OS clipboard.
//!
//! Feature-gated on `clipboard` exactly like the image *read* in
//! `tmux::attach`: `arboard` links X11/Wayland on Linux, so only the desktop TUI
//! wants it — the server, the remote backend and the Flutter cdylib all take
//! core with `default-features = false`.
//!
//! The failure modes are all ordinary rather than exceptional (no display over a
//! bare SSH session, no compositor clipboard, the feature compiled out), so this
//! returns a message for a caller to show rather than treating them as faults.
//! A caller must therefore always have a fallback that puts the text on screen.

/// The process's clipboard handle, created on first use and then **kept alive**.
///
/// Not an implementation detail: on X11 the clipboard is an ownership protocol,
/// not a buffer, so whoever set the selection has to stay around to serve it.
/// Dropping the last `arboard::Clipboard` offers the data to a running clipboard
/// manager and then destroys the selection-owner window (receipt:
/// arboard-3.6.1 `src/platform/linux/x11.rs:1082-1112`, calling
/// `ask_clipboard_manager_to_request_our_data` at `x11.rs:711-790` and then
/// `destroy_window`). With no clipboard manager to hand off to — a plain X11
/// session, i3, a bare Xorg — the copied text would simply vanish while we told
/// the operator it was copied. Holding the handle for the life of the process
/// keeps us the selection owner instead.
///
/// Wayland does not need this (arboard forks a serving process,
/// `wayland.rs:126`), but one handle is correct on both, and lazy so a TUI that
/// never copies anything never opens a display connection.
///
/// Both claims are read from arboard's source, not observed on a live X11
/// session without a clipboard manager.
#[cfg(feature = "clipboard")]
static CLIPBOARD: std::sync::Mutex<Option<arboard::Clipboard>> = std::sync::Mutex::new(None);

/// Put `text` on the operator's clipboard.
///
/// The blocking `arboard` call runs on a blocking thread so it cannot stall an
/// async caller.
#[cfg(feature = "clipboard")]
pub async fn set_text(text: impl Into<String>) -> std::result::Result<(), String> {
    let text = text.into();
    tokio::task::spawn_blocking(move || {
        let mut guard = CLIPBOARD
            .lock()
            .map_err(|_| "clipboard lock poisoned".to_string())?;
        if guard.is_none() {
            *guard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        guard
            .as_mut()
            .expect("just initialised")
            .set_text(text)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("clipboard task panicked: {e}"))?
}

/// Clipboard support compiled out (`--no-default-features`): there is nothing to
/// write to, and the caller's fallback (telling the operator where to find the
/// value) is the whole behaviour.
#[cfg(not(feature = "clipboard"))]
pub async fn set_text(_text: impl Into<String>) -> std::result::Result<(), String> {
    Err("clipboard support is not compiled in".to_string())
}
