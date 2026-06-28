//! Typed HTTP error mapping for the ARD registry edge.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use ard_catalog::RegistryErrorResponseWire;

use crate::registry_error::RegistryError;

/// HTTP-facing registry error with ARD-compatible JSON shape.
#[derive(Debug, thiserror::Error)]
pub enum RegistryHttpError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error("{0}")]
    InvalidArgument(String),
}

impl RegistryHttpError {
    fn status_code(&self) -> StatusCode {
        match self {
            RegistryHttpError::Registry(error) => match error {
                RegistryError::SearchRequest(_)
                | RegistryError::PageToken(_)
                | RegistryError::InvalidPageSize { .. } => StatusCode::BAD_REQUEST,
            },
            RegistryHttpError::InvalidArgument(_) => StatusCode::BAD_REQUEST,
        }
    }

    fn error_code(&self) -> &'static str {
        match self {
            RegistryHttpError::Registry(error) => match error {
                RegistryError::SearchRequest(_)
                | RegistryError::PageToken(_)
                | RegistryError::InvalidPageSize { .. } => "INVALID_ARGUMENT",
            },
            RegistryHttpError::InvalidArgument(_) => "INVALID_ARGUMENT",
        }
    }
}

impl IntoResponse for RegistryHttpError {
    fn into_response(self) -> Response {
        let message = match &self {
            RegistryHttpError::Registry(error) => error.to_string(),
            RegistryHttpError::InvalidArgument(msg) => msg.clone(),
        };
        let body = RegistryErrorResponseWire::new(self.error_code(), message);
        (self.status_code(), Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests;
