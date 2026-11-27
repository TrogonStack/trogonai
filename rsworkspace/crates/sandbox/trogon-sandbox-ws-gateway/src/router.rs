use axum::Router;
use axum::routing::{any, get, post};
use tower_http::cors::{Any, CorsLayer};

use crate::{AppState, create_session, preview_path, preview_root, ws_upgrade};

/// The gateway's routes. `cors_origins` lists the browser origins allowed to call it; none allows no cross-origin
/// calls.
pub fn router(state: AppState, cors_origins: Vec<String>) -> Router {
    Router::new()
        .route("/v1/sessions", post(create_session))
        .route("/v1/ws", get(ws_upgrade))
        .route("/v1/preview/{lease_id}/{port}/{token}", any(preview_root))
        .route("/v1/preview/{lease_id}/{port}/{token}/", any(preview_root))
        .route("/v1/preview/{lease_id}/{port}/{token}/{*rest}", any(preview_path))
        .layer(build_cors(cors_origins))
        .with_state(state)
}

fn build_cors(origins: Vec<String>) -> CorsLayer {
    let allowed: Vec<_> = origins.iter().filter_map(|origin| origin.parse().ok()).collect();
    if allowed.is_empty() {
        return CorsLayer::new();
    }
    CorsLayer::new()
        .allow_origin(allowed)
        .allow_methods(Any)
        .allow_headers(Any)
}
