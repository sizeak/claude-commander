//! Mutual-TLS support for the web UI.
//!
//! Builds a [`rustls::ServerConfig`] that (a) presents the configured server
//! certificate and (b) *requires* every client to present a certificate signed
//! by the configured CA. This is the "mutual TLS" auth mode: the client
//! certificate is the identity, so there is no password, and the connection is
//! encrypted (unlike Basic-over-HTTP). All crypto goes through the pure-Rust
//! `ring` provider, consistent with the rest of the crate (no aws-lc / C deps).

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use rustls::ServerConfig;
use rustls::server::WebPkiClientVerifier;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::TlsAcceptor;
use tower::Service;
use tracing::debug;

use crate::config::Config;
use crate::error::WebError;

/// Resolve the three mutual-TLS file paths from config, erroring (with a
/// user-facing message) if any is missing. Returns `(cert, key, client_ca)`.
fn mtls_paths(config: &Config) -> Result<(&Path, &Path, &Path), WebError> {
    let cert = config.web_ui_tls_cert.as_deref().ok_or_else(|| {
        WebError::TlsConfig("mutual TLS needs `web_ui_tls_cert` (server certificate PEM)".into())
    })?;
    let key = config.web_ui_tls_key.as_deref().ok_or_else(|| {
        WebError::TlsConfig("mutual TLS needs `web_ui_tls_key` (server private-key PEM)".into())
    })?;
    let ca = config.web_ui_tls_client_ca.as_deref().ok_or_else(|| {
        WebError::TlsConfig(
            "mutual TLS needs `web_ui_tls_client_ca` (CA that client certs must be signed by)"
                .into(),
        )
    })?;
    Ok((cert, key, ca))
}

/// Read a PEM file into bytes, mapping IO errors to a clear [`WebError`].
fn read_pem(path: &Path, what: &str) -> Result<Vec<u8>, WebError> {
    std::fs::read(path).map_err(|e| {
        WebError::TlsConfig(format!("failed to read {what} at {}: {e}", path.display()))
    })
}

/// Parse a PEM file into a chain of certificates.
fn load_certs(path: &Path, what: &str) -> Result<Vec<CertificateDer<'static>>, WebError> {
    let bytes = read_pem(path, what)?;
    let certs: Result<Vec<_>, _> = rustls_pemfile::certs(&mut bytes.as_slice()).collect();
    let certs = certs.map_err(|e| WebError::TlsConfig(format!("invalid {what} PEM: {e}")))?;
    if certs.is_empty() {
        return Err(WebError::TlsConfig(format!(
            "{what} at {} contained no certificates",
            path.display()
        )));
    }
    Ok(certs)
}

/// Parse a PEM file into a single private key (PKCS#8, PKCS#1/RSA, or SEC1/EC).
fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>, WebError> {
    let bytes = read_pem(path, "server private key")?;
    rustls_pemfile::private_key(&mut bytes.as_slice())
        .map_err(|e| WebError::TlsConfig(format!("invalid private-key PEM: {e}")))?
        .ok_or_else(|| {
            WebError::TlsConfig(format!(
                "no private key found in {} (expected PKCS#8/PKCS#1/SEC1 PEM)",
                path.display()
            ))
        })
}

/// Build a rustls [`ServerConfig`] for mutual TLS from the configured paths.
///
/// The returned config presents `web_ui_tls_cert`/`web_ui_tls_key` and rejects
/// any client that does not present a certificate chaining to
/// `web_ui_tls_client_ca`. Returns a [`WebError::TlsConfig`] for any missing or
/// malformed input so the caller can decline to start with a clear message.
pub fn build_server_config(config: &Config) -> Result<Arc<ServerConfig>, WebError> {
    // rustls 0.23 with only the `ring` provider compiled in has no automatic
    // process-default crypto provider — installing it explicitly (idempotent;
    // ignore the error if another component already set one) avoids a runtime
    // panic inside `ServerConfig::builder()`.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (cert_path, key_path, ca_path) = mtls_paths(config)?;

    let server_certs = load_certs(cert_path, "server certificate")?;
    let server_key = load_key(key_path)?;

    // Trust anchors for verifying client certificates.
    let ca_certs = load_certs(ca_path, "client CA certificate")?;
    let mut roots = rustls::RootCertStore::empty();
    for ca in ca_certs {
        roots.add(ca).map_err(|e| {
            WebError::TlsConfig(format!("client CA is not a valid trust anchor: {e}"))
        })?;
    }

    // Require (not just request) a client cert chaining to our roots.
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|e| WebError::TlsConfig(format!("failed to build client-cert verifier: {e}")))?;

    let server_config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(server_certs, server_key)
        .map_err(|e| WebError::TlsConfig(format!("invalid server certificate/key: {e}")))?;

    Ok(Arc::new(server_config))
}

/// Accept TLS connections on `listener` and serve `app` over each, until the
/// task is cancelled. Each accepted TCP stream is wrapped in a rustls handshake
/// (which *rejects* clients without a CA-signed certificate before any HTTP is
/// served), then handed to hyper to drive the axum router. Per-connection
/// errors (failed handshake — i.e. an untrusted/absent client cert — or a
/// dropped connection) are logged and skipped; they never bring the loop down.
pub async fn serve_tls(
    listener: tokio::net::TcpListener,
    server_config: Arc<ServerConfig>,
    app: Router,
) -> Result<(), WebError> {
    let acceptor = TlsAcceptor::from(server_config);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                // A transient accept error shouldn't kill the server.
                debug!("web TLS accept error: {e}");
                continue;
            }
        };

        let acceptor = acceptor.clone();
        let app = app.clone();

        // Handle each connection on its own task so a slow/blocked handshake
        // doesn't stall new connections.
        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(s) => s,
                Err(e) => {
                    // The most common case here is a client with no cert or a
                    // cert not signed by our CA — exactly what mTLS should
                    // reject. Log at debug so it isn't noisy.
                    debug!("web TLS handshake rejected for {peer}: {e}");
                    return;
                }
            };

            let io = TokioIo::new(tls_stream);
            // Bridge hyper's request type to axum's tower::Service.
            let service = hyper::service::service_fn(move |req| {
                let mut app = app.clone();
                async move { app.call(req).await }
            });

            if let Err(e) = ConnBuilder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(io, service)
                .await
            {
                debug!("web TLS connection error for {peer}: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(cert: Option<&str>, key: Option<&str>, ca: Option<&str>) -> Config {
        Config {
            web_ui_tls_cert: cert.map(Into::into),
            web_ui_tls_key: key.map(Into::into),
            web_ui_tls_client_ca: ca.map(Into::into),
            ..Config::default()
        }
    }

    #[test]
    fn mtls_paths_errors_name_the_missing_file() {
        let err = mtls_paths(&cfg(None, Some("k"), Some("ca"))).unwrap_err();
        assert!(err.to_string().contains("web_ui_tls_cert"));

        let err = mtls_paths(&cfg(Some("c"), None, Some("ca"))).unwrap_err();
        assert!(err.to_string().contains("web_ui_tls_key"));

        let err = mtls_paths(&cfg(Some("c"), Some("k"), None)).unwrap_err();
        assert!(err.to_string().contains("web_ui_tls_client_ca"));
    }

    #[test]
    fn mtls_paths_ok_when_all_present() {
        assert!(mtls_paths(&cfg(Some("c"), Some("k"), Some("ca"))).is_ok());
    }

    #[test]
    fn build_server_config_errors_on_missing_files() {
        // Paths set but files don't exist → a TlsConfig read error, not a panic.
        let config = cfg(
            Some("/nope/server.pem"),
            Some("/nope/server.key"),
            Some("/nope/ca.pem"),
        );
        let err = build_server_config(&config).unwrap_err();
        assert!(matches!(err, WebError::TlsConfig(_)));
        assert!(err.to_string().contains("failed to read"));
    }
}
