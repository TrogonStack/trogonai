//! Axum router for the ARD registry HTTP surface.

use std::sync::Arc;

use ard_catalog::{ExploreRequestWire, ListAgentsQueryWire, SearchRequestWire};
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

use crate::explore_request::ValidatedExploreRequest;
use crate::extract::{ArdJson, ArdQuery};
use crate::http_error::RegistryHttpError;
use crate::list_agents_request::ValidatedListAgentsQuery;
use crate::registry::Registry;
use crate::search_request::ValidatedSearchRequest;

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
    ArdJson(wire): ArdJson<SearchRequestWire>,
) -> Result<Json<ard_catalog::SearchResponseWire>, RegistryHttpError> {
    let request = ValidatedSearchRequest::try_from_wire(wire)?;
    Ok(Json(registry.search(request)))
}

async fn get_agents(
    State(registry): State<Arc<Registry>>,
    ArdQuery(wire): ArdQuery<ListAgentsQueryWire>,
) -> Result<Json<ard_catalog::ListResponseWire>, RegistryHttpError> {
    let query = ValidatedListAgentsQuery::try_from_wire(wire)?;
    Ok(Json(registry.list_agents(query)))
}

async fn post_explore(
    State(registry): State<Arc<Registry>>,
    ArdJson(wire): ArdJson<ExploreRequestWire>,
) -> Result<Json<ard_catalog::ExploreResponseWire>, RegistryHttpError> {
    let request = ValidatedExploreRequest::try_from_wire(wire)?;
    Ok(Json(registry.explore(request)))
}

#[cfg(test)]
mod tests;
