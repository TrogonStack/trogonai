use serde::Serialize;

/// The reply to an operation verb this worker does not serve.
#[derive(Serialize)]
pub(crate) struct DeniedBody {
    pub(crate) outcome: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) reason: String,
}
