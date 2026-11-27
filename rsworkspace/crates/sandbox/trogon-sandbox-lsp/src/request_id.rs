use serde::Serialize;

/// Identifier of a request this client sent. Counts from 1.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize)]
#[serde(transparent)]
pub struct RequestId(u64);

impl RequestId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}
