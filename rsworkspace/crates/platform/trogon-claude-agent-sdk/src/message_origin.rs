use crate::JsonObject;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "MessageOriginWire", into = "MessageOriginWire")]
pub struct MessageOrigin {
    kind: String,
    additional_fields: JsonObject,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MessageOriginWire {
    kind: String,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MessageOriginError {
    #[error("message origin extension fields contain a reserved kind field")]
    ReservedField,
}

impl MessageOrigin {
    pub fn new(kind: impl Into<String>, additional_fields: JsonObject) -> Result<Self, MessageOriginError> {
        let kind = kind.into();
        if additional_fields.contains_key("kind") {
            return Err(MessageOriginError::ReservedField);
        }
        Ok(Self {
            kind,
            additional_fields,
        })
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn additional_fields(&self) -> &JsonObject {
        &self.additional_fields
    }
}

impl TryFrom<MessageOriginWire> for MessageOrigin {
    type Error = MessageOriginError;
    fn try_from(value: MessageOriginWire) -> Result<Self, Self::Error> {
        Self::new(value.kind, value.additional_fields)
    }
}

impl From<MessageOrigin> for MessageOriginWire {
    fn from(value: MessageOrigin) -> Self {
        Self {
            kind: value.kind,
            additional_fields: value.additional_fields,
        }
    }
}

#[cfg(test)]
#[path = "message_origin_tests.rs"]
mod tests;
