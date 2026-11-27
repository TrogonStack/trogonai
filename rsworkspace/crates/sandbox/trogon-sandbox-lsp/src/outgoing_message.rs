use serde::Serialize;
use serde_json::Value;

use crate::constants::JSONRPC_VERSION;
use crate::request_id::RequestId;

/// A request this client sends.
#[derive(Serialize)]
pub(crate) struct OutgoingRequest<'a> {
    jsonrpc: &'static str,
    id: RequestId,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

impl<'a> OutgoingRequest<'a> {
    pub(crate) fn new(id: RequestId, method: &'a str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            method,
            params,
        }
    }
}

/// A notification this client sends.
#[derive(Serialize)]
pub(crate) struct OutgoingNotification<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

impl<'a> OutgoingNotification<'a> {
    pub(crate) fn new(method: &'a str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            method,
            params,
        }
    }
}

/// The answer to a request of the server: a result, or an error with a code and message.
#[derive(Serialize)]
pub(crate) struct OutgoingResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(flatten)]
    outcome: ResponseOutcome,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ResponseOutcome {
    Result(Value),
    Error { code: i64, message: String },
}

impl OutgoingResponse {
    pub(crate) fn new(id: Value, outcome: ResponseOutcome) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            outcome,
        }
    }
}
