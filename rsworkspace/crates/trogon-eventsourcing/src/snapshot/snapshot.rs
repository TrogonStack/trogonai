use serde::{Deserialize, Serialize};

use crate::StreamPosition;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot<T> {
    /// The stream position covered by the snapshot payload.
    ///
    /// This is a replay/checkpoint boundary. It is intentionally a
    /// `StreamPosition`, not a revision, because adapters are allowed to use
    /// sparse but comparable positions.
    pub position: StreamPosition,
    pub payload: T,
}

impl<T> Snapshot<T> {
    pub fn new(position: StreamPosition, payload: T) -> Self {
        Self { position, payload }
    }
}
