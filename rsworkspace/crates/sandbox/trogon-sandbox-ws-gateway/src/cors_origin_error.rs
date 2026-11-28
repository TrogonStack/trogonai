/// A configured CORS origin that cannot be used as a header value.
#[derive(Debug, thiserror::Error)]
#[error("CORS origin `{origin}` is not a valid header value")]
pub struct CorsOriginError {
    pub(crate) origin: String,
}
