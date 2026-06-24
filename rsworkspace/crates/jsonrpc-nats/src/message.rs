use crate::id::{RequestId, ResponseId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical JSON-RPC 2.0 message used by the codec.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Success {
        id: ResponseId,
        result: Value,
    },
    Error {
        id: ResponseId,
        code: i32,
        message: String,
        data: Option<Value>,
    },
}

impl Message {
    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Request { method, .. } | Self::Notification { method, .. } => Some(method),
            Self::Success { .. } | Self::Error { .. } => None,
        }
    }

    pub fn is_notification(&self) -> bool {
        matches!(self, Self::Notification { .. })
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }
}

/// Wire error body: `message` and optional `data` only; `code` lives in the header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
