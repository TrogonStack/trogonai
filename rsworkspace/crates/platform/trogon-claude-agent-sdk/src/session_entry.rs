use crate::JsonObject;

/// One transcript entry exactly as the CLI or a store produced it.
///
/// Entries are opaque JSON objects; the SDK never rejects one for its shape.
#[derive(Clone, Eq, PartialEq)]
pub struct SessionEntry(JsonObject);

#[derive(Debug, thiserror::Error)]
pub enum SessionEntryError {
    #[error("session entry must be a JSON object")]
    NotObject,
}

impl SessionEntry {
    pub fn new(data: JsonObject) -> Self {
        Self(data)
    }

    pub fn as_object(&self) -> &JsonObject {
        &self.0
    }
    pub fn into_object(self) -> JsonObject {
        self.0
    }
    pub fn kind(&self) -> &str {
        self.0
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
    }
    pub fn uuid(&self) -> Option<&str> {
        self.0.get("uuid").and_then(serde_json::Value::as_str)
    }
    pub fn is_sidechain(&self) -> bool {
        self.0
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }
}

impl From<JsonObject> for SessionEntry {
    fn from(value: JsonObject) -> Self {
        Self::new(value)
    }
}

impl TryFrom<serde_json::Value> for SessionEntry {
    type Error = SessionEntryError;
    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        match value {
            serde_json::Value::Object(data) => Ok(Self::new(data)),
            _ => Err(SessionEntryError::NotObject),
        }
    }
}

impl std::fmt::Debug for SessionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEntry")
            .field("kind", &self.kind())
            .finish_non_exhaustive()
    }
}
