use serde::Serialize;

/// The JSON body of an HTTP error response.
#[derive(Debug, Serialize)]
pub(crate) struct ErrorBody {
    pub error: &'static str,
}
