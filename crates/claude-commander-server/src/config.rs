//! Server configuration *policy*: environment layering and the no-auth bind guard.
//!
//! The persisted shape — the `[server]` table of `config.toml` — is
//! [`claude_commander_core::config::ServerConfig`], not a type of this crate's.
//! It moved into core so that `ConfigStore`, which persists by re-serialising the
//! whole `Config`, stops deleting a table it does not model, and so the TUI's
//! settings UI can edit it without depending on this crate. See that type's
//! module docs for the full reasoning.
//!
//! The dependency direction is unchanged (server → core, never the reverse), and
//! what stays here is everything that is about *serving*: the `CC_SERVER_*`
//! environment overrides layered on top of the file ([`resolve`]) and the refusal
//! to run unauthenticated on a routable address ([`check_no_auth_bind`]).

use std::net::IpAddr;

use claude_commander_core::config::ServerConfig;
use figment::{
    Figment,
    providers::{Env, Serialized},
};
use serde::{Deserialize, Serialize};

/// Layer `CC_SERVER_*` environment overrides over the `[server]` table core
/// loaded from `config.toml`.
///
/// Core's own loader is file-only (no env provider), so this is the one place
/// the environment gets a say. Both frontends call it — the standalone binary
/// before applying its CLI flags, and the TUI's embedded server — so an operator
/// who sets `CC_SERVER_PORT` sees the same effect either way.
pub fn resolve(base: ServerConfig) -> Result<ServerConfig, Box<figment::Error>> {
    // The env keys are namespaced under `server.` so they land inside the
    // wrapper's field (e.g. `CC_SERVER_TOKEN` → `server.token`), which is the
    // shape figment needs to merge them onto the struct.
    #[derive(Serialize, Deserialize)]
    struct Wrapper {
        #[serde(default)]
        server: ServerConfig,
    }

    let wrapper: Wrapper = Figment::from(Serialized::defaults(Wrapper { server: base }))
        .merge(Env::prefixed("CC_SERVER_").map(|k| format!("server.{k}").into()))
        .extract()
        .map_err(Box::new)?;

    Ok(wrapper.server)
}

/// Reject the dangerous `--allow-no-auth` on a non-loopback bind.
///
/// Disabling authentication is only ever safe on a loopback interface; on any
/// routable address it would expose an unauthenticated session-control API to
/// the network. Returns `Err` with an operator-facing message when
/// `allow_no_auth` is set against a non-loopback `bind`; `Ok` otherwise
/// (loopback + no-auth, or any bind with a token).
pub fn check_no_auth_bind(bind: IpAddr, allow_no_auth: bool) -> Result<(), String> {
    if allow_no_auth && !bind.is_loopback() {
        Err(format!(
            "--allow-no-auth refuses to run on non-loopback bind {bind}: an unauthenticated \
             API would be exposed to the network. Bind to a loopback address (127.0.0.1 / ::1) \
             or configure a token instead."
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn no_auth_on_loopback_is_ok() {
        assert!(check_no_auth_bind(IpAddr::V4(Ipv4Addr::LOCALHOST), true).is_ok());
        assert!(check_no_auth_bind(IpAddr::V6(Ipv6Addr::LOCALHOST), true).is_ok());
    }

    #[test]
    fn no_auth_on_non_loopback_is_rejected() {
        let err = check_no_auth_bind(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), true)
            .expect_err("0.0.0.0 + --allow-no-auth must be rejected");
        assert!(
            err.contains("loopback"),
            "message should explain why: {err}"
        );
    }

    #[test]
    fn token_on_non_loopback_is_ok() {
        // No `--allow-no-auth` → a token is in force, so any bind is allowed.
        assert!(check_no_auth_bind(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), false).is_ok());
    }

    /// With no `CC_SERVER_*` variables set, `resolve` is the identity. It is
    /// deliberately not tested *with* them set: `Env` reads the real process
    /// environment, which is global to the test binary, so a mutating test would
    /// make its neighbours order-dependent.
    #[test]
    fn resolve_passes_the_file_values_through_untouched() {
        let base = ServerConfig {
            auto_start: true,
            port: 9999,
            token: Some("sekret".into()),
            ..Default::default()
        };
        let resolved = resolve(base.clone()).unwrap();
        assert_eq!(resolved, base);
    }
}
