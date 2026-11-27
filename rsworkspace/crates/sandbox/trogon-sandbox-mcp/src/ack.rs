use rmcp::handler::server::wrapper::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The output of a tool that only confirms the broker accepted the request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Ack {
    pub ack: bool,
}

impl Ack {
    pub(crate) fn reply() -> Json<Self> {
        Json(Self { ack: true })
    }
}
