use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::error_body::ErrorBody;

/// An HTTP error response with a machine-readable `code`. A failure behind the response is logged, never sent.
pub(crate) fn deny(status: StatusCode, code: &'static str, source: Option<&dyn std::error::Error>) -> Response {
    if let Some(source) = source {
        tracing::warn!(error = %source, code, "request failed");
    }
    (status, Json(ErrorBody { error: code })).into_response()
}
