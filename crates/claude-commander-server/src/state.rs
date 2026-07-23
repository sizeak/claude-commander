//! Shared application state, cloned into every handler.

use std::sync::Arc;

use claude_commander_core::api::CommanderService;

use crate::auth::AuthConfig;
use crate::slack::handler::SlackApi;

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
    /// The Slack Web API client, present only when the Slack bridge is running
    /// (`[slack]` enabled). `None` means Slack is unavailable, which the notify
    /// route surfaces as a 503. Shared with the bridge so the server holds a
    /// single Slack client.
    pub slack: Option<Arc<dyn SlackApi>>,
}

impl AppState {
    /// Build state with no CORS allowlist (same-origin only) and no Slack
    /// client. Tests and the default path use this; the server overrides via
    /// [`Self::with_cors`] / [`Self::with_slack`].
    pub fn new(service: CommanderService, auth: AuthConfig) -> Self {
        Self {
            service,
            auth: Arc::new(auth),
            cors_allowed_origins: Arc::new(Vec::new()),
            slack: None,
        }
    }

    /// Set the CORS allowlist of permitted origins. Builder-style so the
    /// 2-arg `new` stays the common case for tests.
    pub fn with_cors(mut self, origins: Vec<String>) -> Self {
        self.cors_allowed_origins = Arc::new(origins);
        self
    }

    /// Attach the Slack Web API client (enables the notify route). Builder-style
    /// so the 2-arg `new` stays the common case for tests.
    pub fn with_slack(mut self, slack: Option<Arc<dyn SlackApi>>) -> Self {
        self.slack = slack;
        self
    }
}
