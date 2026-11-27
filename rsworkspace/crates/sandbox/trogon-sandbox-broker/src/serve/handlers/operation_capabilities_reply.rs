use serde::Serialize;
use trogon_sandbox_contract::{CancelOperationResponse, OperationCapabilities};

/// Reply to an operation capabilities request. A denial is sent in the shape of an operation
/// cancel denial, which is what clients of this subject read.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum OperationCapabilitiesReply {
    Capabilities(OperationCapabilities),
    Denied(CancelOperationResponse),
}

#[cfg(test)]
mod tests;
