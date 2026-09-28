//! Reverse proxy for the `/api` surface → `claude-commander-server`.
//!
//! Every `/api/...` request is forwarded upstream verbatim except for the
//! `Authorization` header: in [`AuthMode::Bff`] the browser's Basic credential
//! (already validated by the auth layer) is replaced with the server's bearer
//! token; in [`AuthMode::PassThrough`] the browser's own bearer token is
//! forwarded unchanged. Bodies are buffered (the `/api` surface is small JSON +
//! the occasional blob), so this is not a streaming proxy.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tracing::warn;

use crate::config::{AppState, AuthMode};

/// Max buffered request body (guards against unbounded uploads). Blob fetches
/// are downloads (response side), so 32 MiB of *request* body is ample.
const MAX_BODY: usize = 32 * 1024 * 1024;

/// Hop-by-hop headers that must not be forwarded (RFC 7230 §6.1) plus
/// `content-length`, which the receiving stack recomputes from the buffered body.
fn is_hop_by_hop(name: &header::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    )
}

pub async fn proxy_api(State(state): State<AppState>, req: Request) -> Response {
    // Preserve the full path (`/api/...`) + query when building the upstream URL.
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_owned())
        .unwrap_or_else(|| req.uri().path().to_owned());
    let url = state.upstream_api_url(&path_and_query);

    let method = req.method().clone();
    let (parts, body) = req.into_parts();

    let body_bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response(),
    };

    // Forward request headers minus host/hop-by-hop; set the upstream auth.
    let mut headers = parts.headers.clone();
    headers.remove(header::HOST);
    headers.remove(header::AUTHORIZATION);
    if let AuthMode::Bff { token, .. } = state.auth.as_ref() {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(header::AUTHORIZATION, v);
        }
    } else if let Some(v) = parts.headers.get(header::AUTHORIZATION) {
        // PassThrough: forward the browser's own bearer token verbatim.
        headers.insert(header::AUTHORIZATION, v.clone());
    }

    let upstream = state
        .http
        .request(method, &url)
        .headers(headers)
        .body(body_bytes)
        .send()
        .await;

    let resp = match upstream {
        Ok(r) => r,
        Err(e) => {
            warn!("upstream /api request failed: {e}");
            return (StatusCode::BAD_GATEWAY, format!("upstream error: {e}")).into_response();
        }
    };

    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            warn!("reading upstream /api response failed: {e}");
            return (StatusCode::BAD_GATEWAY, "upstream read error").into_response();
        }
    };

    let mut builder = Response::builder().status(status);
    for (name, value) in resp_headers.iter() {
        if !is_hop_by_hop(name) {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
