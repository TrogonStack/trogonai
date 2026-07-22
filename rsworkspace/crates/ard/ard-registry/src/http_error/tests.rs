use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use ard_catalog::{FederationMode, SearchRequestWire};

use super::RegistryHttpError;
use crate::registry_error::RegistryError;
use crate::search_request::SearchRequestError;

#[tokio::test]
async fn maps_invalid_search_request_to_400() {
    let error = RegistryHttpError::from(RegistryError::SearchRequest(SearchRequestError::EmptyQuery));
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn search_request_wire_deserializes_federation_mode() {
    let body = serde_json::json!({
        "query": {
            "text": "assistant"
        },
        "federation": "referrals"
    });
    let request: SearchRequestWire = serde_json::from_value(body).unwrap();
    assert_eq!(request.federation, FederationMode::Referrals);
}
