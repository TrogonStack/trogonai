use serde::Serialize;

/// Body of `POST /v2/sandboxes/{id}/connect`.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct ConnectRequest {
    pub(crate) timeout: u32,
}
