use bytes::Bytes;
use jsonrpc_nats::{Message, ResponseId, to_json_value};
use serde::Serialize;
use serde_json::Value;

/// Stdio outbound frame.
///
/// Success responses prefer the validated NATS body (id already rewritten to the
/// edge client id) so the bytes the agent produced reach stdout unaltered.
/// Everything the bridge builds locally goes out as a canonical JSON-RPC message.
#[derive(Debug)]
pub enum OutboundFrame {
    /// Canonical JSON-RPC body bytes after typed validate + id rewrite.
    RawBody(Bytes),
    Message(Message),
}

impl OutboundFrame {
    /// A2A streams every chunk as a JSON-RPC success response repeating the
    /// request id, so a stream event and the terminal response share one shape.
    pub fn success(id: ResponseId, result: Value) -> Self {
        Self::Message(Message::Success { id, result })
    }

    pub fn error(id: ResponseId, code: i32, message: impl Into<String>) -> Self {
        Self::Message(Message::Error {
            id,
            code,
            message: message.into(),
            data: None,
        })
    }

    /// Stamp the caller's id onto a locally-built error whose id was not known
    /// at construction time.
    pub fn with_error_id(self, id: ResponseId) -> Self {
        match self {
            Self::Message(Message::Error {
                code, message, data, ..
            }) => Self::Message(Message::Error {
                id,
                code,
                message,
                data,
            }),
            other => other,
        }
    }

    pub fn error_code(&self) -> Option<i32> {
        match self {
            Self::Message(Message::Error { code, .. }) => Some(*code),
            _ => None,
        }
    }
}

impl Serialize for OutboundFrame {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::RawBody(body) => {
                let value: Value = serde_json::from_slice(body).map_err(serde::ser::Error::custom)?;
                value.serialize(serializer)
            }
            Self::Message(message) => to_json_value(message).serialize(serializer),
        }
    }
}

#[cfg(test)]
mod tests;
