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
    /// Keeps `kind` and every other field, as both upstream SDKs pass an origin through. A `kind` inside
    /// `additional_fields` is dropped, because an origin object, like the JavaScript one upstream, has one `kind`.
    pub fn new(kind: impl Into<String>, mut additional_fields: JsonObject) -> Self {
        additional_fields.shift_remove("kind");
        Self {
            kind: kind.into(),
            additional_fields,
        }
    }

    /// Opt-in: refuses a `kind` inside `additional_fields` instead of dropping it.
    pub fn validated(kind: impl Into<String>, additional_fields: JsonObject) -> Result<Self, MessageOriginError> {
        if additional_fields.contains_key("kind") {
            return Err(MessageOriginError::ReservedField);
        }
        Ok(Self::new(kind, additional_fields))
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
        Self::validated(value.kind, value.additional_fields)
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
