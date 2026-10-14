//! Shared application state, cloned into every handler.

use std::sync::Arc;

use claude_commander_core::api::CommanderService;

use crate::auth::AuthConfig;
use claude_commander_chat::ChatHub;

/// State shared across all handlers. `CommanderService` is already
/// `Arc`-backed and cheap to clone; `auth` is shared behind an `Arc`.
#[derive(Clone)]
pub struct AppState {
    pub service: CommanderService,
    pub auth: Arc<AuthConfig>,
    /// CORS allowlist of permitted origins (from core's `ServerConfig`). Empty means
    /// no cross-origin access (same-origin only). Consumed by `build_router`
    /// when assembling the `/api` CORS layer.
    pub cors_allowed_origins: Arc<Vec<String>>,
    /// The chat backends this server runs (possibly none). The notify route
    /// delivers through it, sharing the backends' clients with their bridges.
    pub chat: Arc<ChatHub>,
}

impl AppState {
    /// Build state with no CORS allowlist (same-origin only) and no chat
    /// backends. Tests and the default path use this; the server overrides via
    /// [`Self::with_cors`] / [`Self::with_chat`].
    pub fn new(service: CommanderService, auth: AuthConfig) -> Self {
        Self {
            service,
            auth: Arc::new(auth),
            cors_allowed_origins: Arc::new(Vec::new()),
            chat: Arc::new(ChatHub::new(Vec::new(), None)),
        }
    }

    /// Set the CORS allowlist of permitted origins. Builder-style so the
    /// 2-arg `new` stays the common case for tests.
    pub fn with_cors(mut self, origins: Vec<String>) -> Self {
        self.cors_allowed_origins = Arc::new(origins);
        self
    }

    /// Attach the chat backends (enables the notify route). Builder-style so
    /// the 2-arg `new` stays the common case for tests.
    pub fn with_chat(mut self, chat: Arc<ChatHub>) -> Self {
        self.chat = chat;
        self
    }
}
