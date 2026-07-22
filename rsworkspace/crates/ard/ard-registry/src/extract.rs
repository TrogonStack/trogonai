//! Custom extractors that map deserialization failures to the ARD wire error shape.

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::http_error::RegistryHttpError;

/// JSON body extractor that maps rejections to [`RegistryHttpError::InvalidArgument`].
pub struct ArdJson<T>(pub T);

impl<T, S> FromRequest<S> for ArdJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = RegistryHttpError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        axum::Json::<T>::from_request(req, state)
            .await
            .map(|axum::Json(value)| ArdJson(value))
            .map_err(RegistryHttpError::from)
    }
}

/// Query string extractor that maps rejections to [`RegistryHttpError::InvalidArgument`].
pub struct ArdQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ArdQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = RegistryHttpError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(value)| ArdQuery(value))
            .map_err(RegistryHttpError::from)
    }
}
