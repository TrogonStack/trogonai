use bytes::Bytes;

/// One request the client sent through the bus.
#[derive(Debug, Clone)]
pub(crate) struct Sent {
    pub subject: String,
    pub payload: Bytes,
}

impl Sent {
    /// The request body decoded as JSON.
    pub(crate) fn json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_slice(&self.payload)
    }
}
