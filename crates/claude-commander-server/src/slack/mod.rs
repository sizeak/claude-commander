//! Slack Socket Mode bridge.
//!
//! A config-gated background task (started from `main` when
//! [`SlackConfig::is_enabled`](claude_commander_core::config::SlackConfig::is_enabled))
//! that turns `@commander` mentions and DMs into short-lived headless commander
//! turns and replies in-thread.
//!
//! The crate keeps the network-facing `slack-morphism` wiring
//! ([`listener`], [`client`]) deliberately thin; all the behaviour worth testing
//! — event classification, dedup, key derivation, mention stripping, prompt
//! assembly, reply accumulation, and the react → ask → reply flow — lives in the
//! pure [`decision`] module and the fake-testable [`handler`] flow.

pub mod client;
pub mod decision;
pub mod handler;
pub mod listener;

pub use listener::spawn_bridge;
