use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::WsErrorBody;

/// One message on the websocket, tagged by `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WsFrame {
    Request {
        id: String,
        tool: String,
        payload: Value,
    },
    Response {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<WsErrorBody>,
    },
    Event {
        stream_id: String,
        event: Value,
    },
}

impl WsFrame {
    pub fn response_ok(id: impl Into<String>, result: Value) -> Self {
        Self::Response {
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    pub fn response_err(id: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::response_error_body(id, WsErrorBody::new(code, message))
    }

    pub fn response_error_body(id: impl Into<String>, error: WsErrorBody) -> Self {
        Self::Response {
            id: id.into(),
            result: None,
            error: Some(error),
        }
    }

    pub fn event(stream_id: impl Into<String>, event: Value) -> Self {
        Self::Event {
            stream_id: stream_id.into(),
            event,
        }
    }
}
