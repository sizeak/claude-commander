//! Application router: the SPA (fallback), the `/api` reverse proxy, the
//! `/ws/attach` WS bridge, and a `/webui/config` hint telling the SPA which auth
//! mode it's running under. In BFF mode a Basic-auth layer wraps everything.

use axum::routing::{any, get};
use axum::{Json, Router, extract::State, middleware::from_fn_with_state};
use serde_json::json;

use crate::config::{AppState, AuthMode};
use crate::{assets, auth, proxy, ws_proxy};

/// Build the full application router for the given state.
pub fn build_router(state: AppState) -> Router {
    let bff = matches!(state.auth.as_ref(), AuthMode::Bff { .. });

    let router = Router::new()
        .route("/webui/config", get(webui_config))
        .route("/ws/attach", get(ws_proxy::attach))
        .route("/api/{*path}", any(proxy::proxy_api))
        .fallback(assets::serve_asset)
        .with_state(state.clone());

    if bff {
        // Browsers replay cached Basic credentials on same-origin requests
        // (including the WS upgrade), so one layer covers SPA + api + ws.
        router.layer(from_fn_with_state(state, auth::require_basic_auth))
    } else {
        router
    }
}

/// Tell the SPA how it should authenticate: `bff` → it's behind a password and
/// the token is injected server-side (no connect screen); `direct` → the SPA
/// must collect the commander URL + token itself.
async fn webui_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "mode": state.auth.label() }))
}
