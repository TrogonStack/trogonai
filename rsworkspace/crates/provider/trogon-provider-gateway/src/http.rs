use axum::{Json, Router, http::StatusCode, routing::get};
use serde::Serialize;

#[derive(Serialize)]
struct RootResponse {
    message: &'static str,
    endpoints: [&'static str; 2],
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

pub fn router() -> Router {
    Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz).head(healthz_head))
}

async fn root() -> Json<RootResponse> {
    Json(RootResponse {
        message: "Trogon Provider Gateway",
        endpoints: ["GET /healthz", "HEAD /healthz"],
    })
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn healthz_head() -> StatusCode {
    StatusCode::OK
}

#[cfg(test)]
mod tests;
