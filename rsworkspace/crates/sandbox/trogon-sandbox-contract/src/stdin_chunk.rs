use serde::{Deserialize, Serialize};

use crate::StdinChunkError;

/// The bytes of one stdin write, at most [`StdinChunk::MAX_LEN`].
///
/// The bound keeps the JSON number array encoding, about four times the byte count, well under a bus message limit.
/// Construction and deserialization both enforce it. It serializes as a JSON array of numbers.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<u8>", into = "Vec<u8>")]
pub struct StdinChunk(Vec<u8>);

impl StdinChunk {
    /// Most bytes one chunk may hold.
    pub const MAX_LEN: usize = 64 * 1024;

    pub fn new(data: Vec<u8>) -> Result<Self, StdinChunkError> {
        if data.len() > Self::MAX_LEN {
            return Err(StdinChunkError::TooLarge { max: Self::MAX_LEN });
        }
        Ok(Self(data))
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<Vec<u8>> for StdinChunk {
    type Error = StdinChunkError;

    fn try_from(data: Vec<u8>) -> Result<Self, Self::Error> {
        Self::new(data)
    }
}

impl From<StdinChunk> for Vec<u8> {
    fn from(chunk: StdinChunk) -> Self {
        chunk.0
    }
}
