//! Binding and serving the web UI, shared by this crate's own binary and by
//! the `claude-commander` TUI, which serves it in-process beside its embedded
//! API server.
//!
//! One implementation of "validate the auth, bind a listener, serve the router"
//! means the two frontends cannot drift on the rules — most importantly that
//! BFF mode never runs with an empty password.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::config::{AppState, AuthMode};
use crate::router::build_router;

/// Why the web UI could not be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// BFF mode was requested with an empty (or whitespace) password, which
    /// would hand the commander token to anyone who asked.
    #[error("the web UI password must not be empty")]
    EmptyPassword,
    /// The listener could not be bound — most often the port is already taken.
    #[error("could not bind {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}

impl AuthMode {
    /// Build a BFF auth mode, refusing an empty password.
    ///
    /// The only constructor either frontend uses, so the "no empty password"
    /// rule lives in one place rather than at each call site.
    pub fn bff(
        username: impl Into<String>,
        password: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, StartError> {
        let password = password.into();
        if password.trim().is_empty() {
            return Err(StartError::EmptyPassword);
        }
        Ok(AuthMode::Bff {
            username: username.into(),
            password,
            token: token.into(),
        })
    }
}

/// A running web UI. Dropping it aborts the serving task, so an embedder cannot
/// leak the listener; the standalone binary [`join`](Self::join)s instead.
pub struct EmbeddedWeb {
    addr: SocketAddr,
    /// `Option` so [`join`](Self::join) can take it without `Drop` aborting it.
    task: Option<JoinHandle<()>>,
}

impl EmbeddedWeb {
    /// The address actually bound (a configured port of 0 resolves here).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A base URL for a browser on this machine. An unspecified bind renders as
    /// `localhost`, since `0.0.0.0` is not dialable.
    pub fn url(&self) -> String {
        if self.addr.ip().is_unspecified() {
            format!("http://localhost:{}", self.addr.port())
        } else {
            format!("http://{}", self.addr)
        }
    }

    /// Serve until the task ends.
    pub async fn join(mut self) {
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for EmbeddedWeb {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// Bind `bind:port` and serve the web UI on a background task, proxying to
/// `commander_url` (trailing slashes are trimmed).
pub async fn start(
    bind: IpAddr,
    port: u16,
    commander_url: &str,
    auth: AuthMode,
) -> Result<EmbeddedWeb, StartError> {
    let requested = SocketAddr::new(bind, port);
    let listener = tokio::net::TcpListener::bind(requested)
        .await
        .map_err(|source| StartError::Bind {
            addr: requested,
            source,
        })?;
    let addr = listener.local_addr().unwrap_or(requested);

    let state = AppState {
        http: reqwest::Client::new(),
        commander_url: Arc::from(commander_url.trim_end_matches('/')),
        auth: Arc::new(auth),
    };
    let app = build_router(state);
    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("web UI stopped: {e}");
        }
    });

    Ok(EmbeddedWeb {
        addr,
        task: Some(task),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn bff_refuses_an_empty_password() {
        for blank in ["", "  "] {
            assert!(matches!(
                AuthMode::bff("admin", blank, "tok"),
                Err(StartError::EmptyPassword)
            ));
        }
        assert!(matches!(
            AuthMode::bff("admin", "pw", "tok"),
            Ok(AuthMode::Bff { .. })
        ));
    }

    #[test]
    fn url_renders_an_unspecified_bind_as_localhost() {
        let web = EmbeddedWeb {
            addr: "0.0.0.0:8420".parse().unwrap(),
            task: None,
        };
        assert_eq!(web.url(), "http://localhost:8420");
        let web = EmbeddedWeb {
            addr: "127.0.0.1:8420".parse().unwrap(),
            task: None,
        };
        assert_eq!(web.url(), "http://127.0.0.1:8420");
    }

    /// End to end over a real socket: a BFF web UI comes up, gates the browser
    /// behind Basic auth, and goes away when its guard is dropped.
    #[tokio::test]
    async fn start_serves_behind_basic_auth_until_dropped() {
        let auth = AuthMode::bff("admin", "pw", "tok").unwrap();
        let web = start(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            0,
            "http://127.0.0.1:1/",
            auth,
        )
        .await
        .unwrap();
        let url = format!("{}/webui/config", web.url());
        let client = reqwest::Client::new();

        let anon = client.get(&url).send().await.unwrap();
        assert_eq!(anon.status(), 401, "no credentials must be refused");

        let authed = client
            .get(&url)
            .basic_auth("admin", Some("pw"))
            .send()
            .await
            .unwrap();
        assert_eq!(authed.status(), 200);
        let body: serde_json::Value =
            serde_json::from_slice(&authed.bytes().await.unwrap()).unwrap();
        assert_eq!(body["mode"], "bff");

        let addr = web.addr();
        drop(web);
        // The abort lands on the runtime's next poll of the task.
        tokio::task::yield_now().await;
        let mut refused = false;
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(addr).await.is_err() {
                refused = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(refused, "dropping the guard must stop the listener");
    }

    #[tokio::test]
    async fn a_taken_port_is_a_bind_error() {
        let held = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = held.local_addr().unwrap().port();
        let err = start(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            "http://127.0.0.1:1",
            AuthMode::PassThrough,
        )
        .await
        .err()
        .expect("the port is taken");
        assert!(matches!(err, StartError::Bind { .. }), "{err}");
    }
}
