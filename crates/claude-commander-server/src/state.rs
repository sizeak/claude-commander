//! Shared application state, cloned into every handler.

use std::sync::Arc;

use claude_commander_core::api::CommanderService;

use crate::auth::AuthConfig;

/// State shared across all handlers. `CommanderService` is already
/// `Arc`-backed and cheap to clone; `auth` is shared behind an `Arc`.
#[derive(Clone)]
pub struct AppState {
    pub service: CommanderService,
    pub auth: Arc<AuthConfig>,
    /// CORS allowlist of permitted origins (from `ServerConfig`). Empty means
    /// no cross-origin access (same-origin only). Consumed by `build_router`
    /// when assembling the `/api` CORS layer.
    pub cors_allowed_origins: Arc<Vec<String>>,
}

impl AppState {
    /// Build state with no CORS allowlist (same-origin only). Tests and the
    /// default path use this; the server overrides the allowlist via
    /// [`Self::with_cors`].
    pub fn new(service: CommanderService, auth: AuthConfig) -> Self {
        Self {
            service,
            auth: Arc::new(auth),
            cors_allowed_origins: Arc::new(Vec::new()),
        }
    }

    /// Set the CORS allowlist of permitted origins. Builder-style so the
    /// 2-arg `new` stays the common case for tests.
    pub fn with_cors(mut self, origins: Vec<String>) -> Self {
        self.cors_allowed_origins = Arc::new(origins);
        self
    }
}
