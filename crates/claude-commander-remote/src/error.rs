//! Mapping the client crate's [`ClientError`] onto core's [`BackendError`].
//!
//! The HTTP/transport classification itself lives in
//! `claude-commander-client`'s `error` module; the client's categories are
//! carved to line up 1:1 with core's, so this adapter is a flat rename. Both
//! types are foreign to this crate, so the conversion can't be a `From` impl
//! (orphan rule) — it's a free function every trait method threads through
//! `.map_err(into_backend_error)`.
//!
//! **Token safety:** the client never puts a bearer token in a `ClientError`
//! (its reasons come from the HTTP status or the server's own error body), and
//! this conversion only moves those strings across — so a token still can't
//! reach a `BackendError`'s `Display`/`Debug`.

use claude_commander_client::ClientError;
use claude_commander_core::backend::BackendError;

/// Convert a [`ClientError`] into the matching [`BackendError`] category. The
/// mapping is 1:1 — the client's categories were defined to mirror core's.
pub(crate) fn into_backend_error(err: ClientError) -> BackendError {
    match err {
        ClientError::Auth => BackendError::Auth,
        ClientError::NotFound => BackendError::NotFound,
        ClientError::InvalidRequest(m) => BackendError::InvalidRequest(m),
        ClientError::Unavailable { reason } => BackendError::Unavailable { reason },
        ClientError::Server(m) => BackendError::Server(m),
        ClientError::Protocol(m) => BackendError::Protocol(m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_category_one_for_one() {
        assert!(matches!(
            into_backend_error(ClientError::Auth),
            BackendError::Auth
        ));
        assert!(matches!(
            into_backend_error(ClientError::NotFound),
            BackendError::NotFound
        ));
        assert!(matches!(
            into_backend_error(ClientError::InvalidRequest("bad".into())),
            BackendError::InvalidRequest(m) if m == "bad"
        ));
        assert!(matches!(
            into_backend_error(ClientError::Unavailable { reason: "down".into() }),
            BackendError::Unavailable { reason } if reason == "down"
        ));
        assert!(matches!(
            into_backend_error(ClientError::Server("boom".into())),
            BackendError::Server(m) if m == "boom"
        ));
        assert!(matches!(
            into_backend_error(ClientError::Protocol("garble".into())),
            BackendError::Protocol(m) if m == "garble"
        ));
    }
}
