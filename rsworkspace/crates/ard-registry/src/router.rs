//! Axum router for the ARD registry HTTP surface.

use std::sync::Arc;

use ard_catalog::{ExploreRequestWire, ListAgentsQueryWire, SearchRequestWire};
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

use crate::http_error::RegistryHttpError;
use crate::registry::Registry;

/// Build the ARD registry HTTP router.
pub fn router(registry: Arc<Registry>) -> Router {
    Router::new()
        .route("/.well-known/ai-catalog.json", get(get_manifest))
        .route("/search", post(post_search))
        .route("/agents", get(get_agents))
        .route("/explore", post(post_explore))
        .with_state(registry)
        .layer(TraceLayer::new_for_http())
}

async fn get_manifest(State(registry): State<Arc<Registry>>) -> impl IntoResponse {
    Json(registry.manifest().clone().into_wire())
}

async fn post_search(
    State(registry): State<Arc<Registry>>,
    Json(request): Json<SearchRequestWire>,
) -> Result<Json<ard_catalog::SearchResponseWire>, RegistryHttpError> {
    registry.search(request).map(Json).map_err(RegistryHttpError::from)
}

async fn get_agents(
    State(registry): State<Arc<Registry>>,
    Query(query): Query<ListAgentsQueryWire>,
) -> Result<Json<ard_catalog::ListResponseWire>, RegistryHttpError> {
    registry.list_agents(query).map(Json).map_err(RegistryHttpError::from)
}

async fn post_explore(
    State(registry): State<Arc<Registry>>,
    Json(request): Json<ExploreRequestWire>,
) -> Result<Json<ard_catalog::ExploreResponseWire>, RegistryHttpError> {
    registry.explore(request).map(Json).map_err(RegistryHttpError::from)
}

#[cfg(test)]
mod tests;
