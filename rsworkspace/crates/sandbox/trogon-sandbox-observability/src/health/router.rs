use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;

use super::Health;

/// Path answering liveness on every listener.
pub const LIVENESS_PATH: &str = "/-/liveness";
/// Path answering readiness on every listener.
pub const READINESS_PATH: &str = "/-/readiness";

impl Health {
    /// The operational routes. Merge them into a listener's router after its other layers so
    /// authentication and application routing cannot hide them. Responses carry a status only.
    pub fn router(&self) -> Router {
        let liveness = self.clone();
        let readiness = self.clone();
        Router::new()
            .route(
                LIVENESS_PATH,
                get(move || {
                    let health = liveness.clone();
                    async move { StatusCode::from(health.liveness()) }
                }),
            )
            .route(
                READINESS_PATH,
                get(move || {
                    let health = readiness.clone();
                    async move { StatusCode::from(health.readiness()) }
                }),
            )
    }
}
