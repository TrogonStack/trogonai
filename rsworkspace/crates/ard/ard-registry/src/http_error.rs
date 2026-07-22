//! Typed HTTP error mapping for the ARD registry edge.

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use ard_catalog::RegistryErrorResponseWire;

use crate::registry_error::RegistryError;

/// HTTP-facing registry error with ARD-compatible JSON shape.
#[derive(Debug, thiserror::Error)]
pub enum RegistryHttpError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Json(#[from] JsonRejection),
    #[error(transparent)]
    Query(#[from] QueryRejection),
}

impl RegistryHttpError {
    fn status_code(&self) -> StatusCode {
        match self {
            RegistryHttpError::Registry(error) => match error {
                RegistryError::SearchRequest(_)
                | RegistryError::PageToken(_)
                | RegistryError::InvalidPageSize { .. }
                | RegistryError::Facet(_)
                | RegistryError::Filter(_) => StatusCode::BAD_REQUEST,
            },
            RegistryHttpError::Json(_) | RegistryHttpError::Query(_) => StatusCode::BAD_REQUEST,
        }
    }

    fn error_code(&self) -> &'static str {
        match self {
            RegistryHttpError::Registry(error) => match error {
                RegistryError::SearchRequest(_)
                | RegistryError::PageToken(_)
                | RegistryError::InvalidPageSize { .. }
                | RegistryError::Facet(_)
                | RegistryError::Filter(_) => "INVALID_ARGUMENT",
            },
            RegistryHttpError::Json(_) | RegistryHttpError::Query(_) => "INVALID_ARGUMENT",
        }
    }
}

impl IntoResponse for RegistryHttpError {
    fn into_response(self) -> Response {
        let message = self.to_string();
        let body = RegistryErrorResponseWire::new(self.error_code(), message);
        (self.status_code(), Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests;
