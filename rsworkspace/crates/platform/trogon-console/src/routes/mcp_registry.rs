use axum::{Json, response::IntoResponse};

use crate::models::mcp_registry::known_mcp_servers;

pub async fn list_mcp_servers() -> impl IntoResponse {
    Json(known_mcp_servers())
}
