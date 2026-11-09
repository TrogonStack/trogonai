use axum::{Router, extract::State, http::StatusCode, routing::get};

use crate::{Health, Readiness};

pub fn router(health: Health) -> Router {
    Router::new()
        .route("/-/liveness", get(liveness))
        .route("/-/readiness", get(readiness))
        .with_state(health)
}

async fn liveness() -> StatusCode {
    StatusCode::OK
}

async fn readiness(State(health): State<Health>) -> StatusCode {
    match health.readiness() {
        Readiness::Ready => StatusCode::OK,
        Readiness::Draining => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests;
