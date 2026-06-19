//! Linux-only system-wide voice toggle via the XDG Desktop Portal
//! `GlobalShortcuts` interface (the Wayland-native route; mirrors the `dictator`
//! app's `src/shortcuts.rs`).
//!
//! Registers a `"toggle-voice"` shortcut with the portal **in-process**; the
//! compositor (KDE/GNOME/…) delivers an activation each time the user presses
//! the key they bound in System Settings ▸ Shortcuts. Each activation toggles
//! recording through the shared [`apply_listen_action`] core — the same one the
//! in-app Alt-V key path uses — so it behaves identically and works even while
//! the main loop is parked in a tmux attach. Unlike the portable Unix socket
//! ([`ipc`](super::ipc)), this needs no command, `qdbus`, or socket: the app
//! registers the action and the user just assigns it a key.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use crate::conversation::{ListenAction, ListenerCommand, apply_listen_action};

/// Stable id for our shortcut. Persisted by the portal across runs so the user's
/// chosen key sticks.
const SHORTCUT_ID: &str = "toggle-voice";

/// Trigger suggested to the portal on first registration (`Meta+Alt+V`). The
/// user can rebind it in System Settings; we never assume this stays fixed.
const DEFAULT_TRIGGER: &str = "LOGO+ALT+v";

/// Register the global shortcut and toggle recording on each activation. Spawns
/// a background task that owns the portal session for the process lifetime.
/// Best-effort: logs and returns if the portal is unavailable or rejects the
/// binding (e.g. no portal backend, or running headless).
pub fn spawn(listener: UnboundedSender<ListenerCommand>, recording: Arc<AtomicBool>) {
    tokio::spawn(async move {
        if let Err(e) = monitor(listener, recording).await {
            warn!(target: "conversation", "global voice shortcut unavailable: {e}");
        }
    });
}

async fn monitor(
    listener: UnboundedSender<ListenerCommand>,
    recording: Arc<AtomicBool>,
) -> ashpd::Result<()> {
    let shortcuts = GlobalShortcuts::new().await?;
    let session = shortcuts.create_session().await?;

    let shortcut = NewShortcut::new(SHORTCUT_ID, "Toggle voice input")
        .preferred_trigger(Some(DEFAULT_TRIGGER));
    shortcuts
        .bind_shortcuts(&session, &[shortcut], None)
        .await?
        .response()?;

    info!(
        target: "conversation",
        "global voice shortcut registered (default {DEFAULT_TRIGGER}); rebind in System Settings ▸ Shortcuts"
    );

    let mut activated = shortcuts.receive_activated().await?;
    while let Some(action) = activated.next().await {
        if action.shortcut_id() == SHORTCUT_ID {
            apply_listen_action(&listener, &recording, ListenAction::Toggle);
        }
    }
    Ok(())
}
