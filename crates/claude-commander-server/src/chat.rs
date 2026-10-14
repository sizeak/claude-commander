//! Which chat backends this server runs.
//!
//! Each platform is a separate crate behind an opt-in cargo feature, so the
//! set of *available* backends is fixed at build time; `config.toml` decides
//! which of those actually run. A platform configured in a build without its
//! feature is logged once rather than silently ignored.

use std::sync::Arc;

use claude_commander_chat::{ChatBackend, ChatHub};
use claude_commander_core::Config;

/// The hub of every backend that is both compiled in and enabled in `config`.
pub fn hub_from_config(config: &Config) -> ChatHub {
    let backends: Vec<Arc<dyn ChatBackend>> = [slack(config)].into_iter().flatten().collect();
    ChatHub::new(backends, config.chat.default_notify.clone())
}

#[cfg(feature = "slack")]
fn slack(config: &Config) -> Option<Arc<dyn ChatBackend>> {
    claude_commander_chat_slack::SlackBackend::from_config(&config.slack)
        .map(|b| b as Arc<dyn ChatBackend>)
}

#[cfg(not(feature = "slack"))]
fn slack(config: &Config) -> Option<Arc<dyn ChatBackend>> {
    if config.slack.is_configured() {
        tracing::warn!(
            "[slack] is configured, but this build has no Slack support \
             (rebuild with `--features slack`)"
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slack_config() -> Config {
        let mut config = Config::default();
        config.slack.app_token = Some("xapp-1".into());
        config.slack.bot_token = Some("xoxb-1".into());
        config.slack.allowed_user_ids = vec!["U1".into()];
        config
    }

    #[test]
    fn nothing_runs_without_chat_config() {
        assert!(hub_from_config(&Config::default()).is_empty());
    }

    #[cfg(feature = "slack")]
    #[test]
    fn an_enabled_slack_table_runs_slack() {
        assert_eq!(
            hub_from_config(&slack_config()).backend_ids(),
            vec!["slack"]
        );
    }

    #[cfg(not(feature = "slack"))]
    #[test]
    fn a_build_without_the_slack_feature_runs_no_slack() {
        assert!(hub_from_config(&slack_config()).is_empty());
    }
}
