//! Static frontend assets, embedded into the binary at build time.
//!
//! The `web/dist` directory (the SPA + vendored xterm.js) is baked in via
//! [`rust_embed`], so the binary is self-contained. Requests that don't match a
//! proxy/WS route fall through to [`serve_asset`], which serves the matching
//! file or `index.html` for the SPA root and unknown client-side routes.

use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

/// Fallback handler: serve an embedded asset by path, defaulting to `index.html`
/// for the root and unknown paths so a browser reload of any client-side route
/// still loads the app.
pub async fn serve_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // Resolve the asset, falling back to index.html for unknown paths. Track
    // which file we actually served so Content-Type reflects *that* file, not
    // the (possibly extension-less) requested path.
    let (served_path, asset) = match Assets::get(path) {
        Some(a) => (path, a),
        None => match Assets::get("index.html") {
            Some(a) => ("index.html", a),
            None => return (StatusCode::NOT_FOUND, "not found").into_response(),
        },
    };

    let mime = mime_guess::from_path(served_path).first_or_octet_stream();
    (
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            // The SPA changes frequently and carries no content hashing, so never
            // let the browser serve a stale bundle (a plain reload would).
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from(asset.data.into_owned()),
    )
        .into_response()
}
