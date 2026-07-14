//! Adapting `claude-commander-client`'s concrete WebSocket attach onto core's
//! transport-neutral attach seam.
//!
//! The client crate owns the WS pump, handshake, and shutdown logic and presents
//! it as a concrete [`AttachConnection`](claude_commander_client::AttachConnection).
//! This module wraps that into core's `AttachConnection`/`AttachStreams`/
//! `AttachTerminator` trait objects so a remote WebSocket and a local PTY look
//! identical to the TUI's interactive loop.

use async_trait::async_trait;
use claude_commander_client::{
    AttachConnection as ClientAttach, AttachEnd as ClientAttachEnd,
    AttachStreams as ClientAttachStreams, AttachTerminator as ClientTerminator,
};
use claude_commander_core::backend::{
    AttachConnection, AttachEnd, AttachResizer, AttachStreams, AttachTerminator,
};

/// Wraps a client [`ClientAttach`] as a core [`AttachConnection`].
pub(crate) struct RemoteAttachConnection(pub(crate) ClientAttach);

impl AttachConnection for RemoteAttachConnection {
    fn split(self: Box<Self>) -> AttachStreams {
        let ClientAttachStreams {
            reader,
            writer,
            resizer,
            terminator,
        } = self.0.split();
        AttachStreams {
            reader: Box::new(reader),
            writer: Box::new(writer),
            resizer: AttachResizer::new(move |cols, rows| resizer.resize(cols, rows)),
            terminator: Box::new(RemoteTerminator(terminator)),
        }
    }
}

/// Wraps the client's concrete terminator as core's [`AttachTerminator`],
/// mapping the client's [`ClientAttachEnd`] onto core's [`AttachEnd`].
struct RemoteTerminator(ClientTerminator);

#[async_trait]
impl AttachTerminator for RemoteTerminator {
    async fn detach(&mut self) {
        self.0.detach().await;
    }

    async fn wait(&mut self) -> AttachEnd {
        map_attach_end(self.0.wait().await)
    }
}

/// Map the client's [`ClientAttachEnd`] onto core's transport-neutral
/// [`AttachEnd`] (the two enums are structurally identical).
fn map_attach_end(end: ClientAttachEnd) -> AttachEnd {
    match end {
        ClientAttachEnd::SessionEnded => AttachEnd::SessionEnded,
        ClientAttachEnd::Detached => AttachEnd::Detached,
        ClientAttachEnd::Error(m) => AttachEnd::Error(m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_end_maps_one_for_one() {
        assert_eq!(
            map_attach_end(ClientAttachEnd::SessionEnded),
            AttachEnd::SessionEnded
        );
        assert_eq!(
            map_attach_end(ClientAttachEnd::Detached),
            AttachEnd::Detached
        );
        assert!(matches!(
            map_attach_end(ClientAttachEnd::Error("x".into())),
            AttachEnd::Error(m) if m == "x"
        ));
    }
}
