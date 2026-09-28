//! The browser UI, embedded into the binary at build time.
//!
//! `webui/` (the page, built from `web/` with xterm.js bundled in) is baked in
//! via [`rust_embed`], so every binary that links this crate — the standalone
//! server and the `claude-commander` TUI's embedded server alike — serves it
//! with no runtime file lookups. It is mounted as the router's fallback, outside the bearer
//! layer like `/health`: the page holds no data, and every call it makes goes
//! through `/api`, which still demands the token.
//!
//! In a debug build rust-embed reads the folder from disk on each request
//! (live edits without a rebuild); release builds — and this crate's tests,
//! via the `debug-embed` dev-dependency feature — embed it, so the tests check
//! what actually ships.
//!
//! That dev-dependency feature leaks further than the tests, though. Cargo
//! unifies dev-dependency features into every target built for a test or
//! all-targets run, so `cargo test -p claude-commander-server` and `cargo
//! clippy --all-targets` relink `target/debug/claude-commander-server` (and any
//! other debug binary built in the same run) *with* `debug-embed`. Until the
//! next plain `cargo build`/`cargo run` relinks it, that binary serves a frozen
//! copy of the page as it stood at that build and ignores edits to `webui/`.
//!
//! Every response here carries [`SECURITY_HEADERS`]; see [`CSP`] for why styles
//! allow `'unsafe-inline'` when scripts do not.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// The page's Content-Security-Policy.
///
/// Scripts are same-origin only (`app.js`; the page has no inline script and
/// the bundle no `eval`). Connections are same-origin plus WebSockets, for
/// `/ws/attach`. Styles must allow `'unsafe-inline'`: xterm.js 5.5 injects
/// `<style>` elements at runtime (its DOM renderer's theme and dimension rules,
/// `document.createElement("style")` in the bundled `app.js`) and sets a
/// `style` attribute via `setAttribute`, both of which a bare `style-src 'self'`
/// blocks. Inline style cannot run code, so the relaxation does not reopen
/// script injection. `frame-ancestors 'none'` is the CSP spelling of
/// `X-Frame-Options: DENY`, which is sent too for older browsers.
pub const CSP: &str = "default-src 'self'; script-src 'self'; \
     style-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss:; \
     object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

/// Headers on every response this module produces, the page's and its misses'.
const SECURITY_HEADERS: [(HeaderName, &str); 4] = [
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::X_FRAME_OPTIONS, "DENY"),
    (header::REFERRER_POLICY, "no-referrer"),
    (header::CONTENT_SECURITY_POLICY, CSP),
];

#[derive(RustEmbed)]
#[folder = "webui/"]
struct Assets;

/// Fallback handler: serve an embedded asset by path, defaulting to
/// `index.html` for the root and for unknown extension-less paths so a browser
/// reload of any client-side route still loads the app. A missing path whose
/// last segment has an extension is a missing *file* and gets a 404; a method
/// other than GET/HEAD gets a 405.
///
/// Responses carry `Cache-Control: no-cache` plus an `ETag` from the file's
/// content hash, so the browser revalidates every load (the files carry no
/// content hash in their names) but a matching `If-None-Match` is a cheap 304.
pub async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let mut resp = serve_inner(&method, &uri, &headers);
    let h = resp.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        h.insert(name, HeaderValue::from_static(value));
    }
    resp
}

fn serve_inner(method: &Method, uri: &Uri, headers: &HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    // Belt and braces: the router gives `/api` and `/ws` their own fallbacks,
    // but should a path under either ever reach here, it is an API miss, not
    // the page — a client expecting JSON must never get a 200 of HTML.
    if is_api_path(path) {
        return crate::error::error_response(StatusCode::NOT_FOUND, "route", "no such endpoint");
    }
    if method != Method::GET && method != Method::HEAD {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, HeaderValue::from_static("GET, HEAD"))],
            "method not allowed",
        )
            .into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };

    // Track which file was actually served so Content-Type reflects *that*
    // file, not the (possibly extension-less) requested path.
    let found = match Assets::get(path) {
        Some(a) => Some((path, a)),
        None if names_a_file(path) => None,
        None => Assets::get("index.html").map(|a| ("index.html", a)),
    };
    let Some((served_path, asset)) = found else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let etag = etag_for(&asset.metadata.sha256_hash());
    let cache = [(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))];

    if if_none_match_hits(headers, &etag) {
        return (StatusCode::NOT_MODIFIED, cache, [(header::ETAG, etag)]).into_response();
    }

    let mime = mime_guess::from_path(served_path).first_or_octet_stream();
    let content_type =
        HeaderValue::from_str(mime.as_ref()).expect("mime_guess yields a valid header value");
    (
        cache,
        [(header::CONTENT_TYPE, content_type), (header::ETAG, etag)],
        Body::from(asset.data.into_owned()),
    )
        .into_response()
}

/// Whether a (leading-slash-stripped) path names a file rather than a
/// client-side route: its last segment has an extension (`app.js.map`,
/// `vendor/xterm.js`). A leading dot alone (`.well-known`) is not one.
fn names_a_file(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or_default();
    last.char_indices()
        .any(|(i, c)| c == '.' && i > 0 && i + 1 < last.len())
}

/// Whether a (leading-slash-stripped) path belongs to the `/api` or `/ws`
/// surface.
fn is_api_path(path: &str) -> bool {
    ["api", "ws"]
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{p}/")))
}

/// A strong ETag (quoted hex of the sha256 rust-embed computes at build time).
fn etag_for(sha256: &[u8; 32]) -> HeaderValue {
    let hex: String = sha256.iter().map(|b| format!("{b:02x}")).collect();
    HeaderValue::from_str(&format!("\"{hex}\"")).expect("hex is a valid header value")
}

/// Whether `If-None-Match` names `etag` (or is `*`). Weak-comparison per
/// RFC 9110 §13.1.2: a `W/` prefix on the client's tag is ignored.
fn if_none_match_hits(headers: &HeaderMap, etag: &HeaderValue) -> bool {
    let Some(inm) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let ours = etag.to_str().unwrap_or_default();
    inm.split(',')
        .map(str::trim)
        .any(|tag| tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == ours)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_none_match_accepts_lists_weak_tags_and_star() {
        let etag = HeaderValue::from_static("\"abc\"");
        let hit = |v: &'static str| {
            let mut h = HeaderMap::new();
            h.insert(header::IF_NONE_MATCH, HeaderValue::from_static(v));
            if_none_match_hits(&h, &etag)
        };
        assert!(hit("\"abc\""));
        assert!(hit("\"x\", W/\"abc\""));
        assert!(hit("*"));
        assert!(!hit("\"abd\""));
        assert!(!if_none_match_hits(&HeaderMap::new(), &etag));
    }

    #[test]
    fn a_path_names_a_file_when_its_last_segment_has_an_extension() {
        for p in ["app.js", "app.js.map", "vendor/xterm.js", "a/b.c"] {
            assert!(names_a_file(p), "{p}");
        }
        for p in [
            "",
            "sessions/abc",
            "v1.2/route",
            ".well-known",
            "x/trailing.",
        ] {
            assert!(!names_a_file(p), "{p}");
        }
    }

    #[test]
    fn api_and_ws_paths_are_never_the_page() {
        for p in ["api", "api/", "api/x", "ws", "ws/", "ws/attach/x"] {
            assert!(is_api_path(p), "{p}");
        }
        for p in ["", "index.html", "apix", "wsx/y", "style.css"] {
            assert!(!is_api_path(p), "{p}");
        }
    }
}
