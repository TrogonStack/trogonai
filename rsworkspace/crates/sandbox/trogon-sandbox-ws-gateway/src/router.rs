use axum::Router;
use axum::http::HeaderValue;
use axum::routing::{any, get, post};
use tower_http::cors::{Any, CorsLayer};

use crate::{AppState, CorsOriginError, create_session, preview_path, preview_root, ws_upgrade};

/// The gateway's routes. `cors_origins` lists the browser origins allowed to call it; none allows no cross-origin
/// calls.
pub fn router(state: AppState, cors_origins: Vec<HeaderValue>) -> Router {
    Router::new()
        .route("/v1/sessions", post(create_session))
        .route("/v1/ws", get(ws_upgrade))
        .route("/v1/preview/{lease_id}/{port}/{token}", any(preview_root))
        .route("/v1/preview/{lease_id}/{port}/{token}/", any(preview_root))
        .route("/v1/preview/{lease_id}/{port}/{token}/{*rest}", any(preview_path))
        .layer(build_cors(cors_origins))
        .with_state(state)
}

/// The configured browser origins as header values. An origin that is not a valid header value is refused, so a typo
/// cannot silently leave a browser origin out.
pub fn parse_cors_origins(origins: &[String]) -> Result<Vec<HeaderValue>, CorsOriginError> {
    origins
        .iter()
        .map(|origin| {
            origin.parse().map_err(|_| CorsOriginError {
                origin: origin.escape_debug().to_string(),
            })
        })
        .collect()
}

fn build_cors(origins: Vec<HeaderValue>) -> CorsLayer {
    if origins.is_empty() {
        return CorsLayer::new();
    }
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(Any)
        .allow_headers(Any)
}
