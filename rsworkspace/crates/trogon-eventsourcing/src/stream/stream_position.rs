use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

/// A comparable high-watermark for one event stream.
///
/// `StreamPosition` is deliberately **not** a stream revision, event count, or
/// gapless version. It is a monotonic position that can answer questions like
/// "has this stream advanced past the position I observed?".
///
/// Valid use cases:
/// - optimistic concurrency with `StreamWritePrecondition::At`
/// - projection freshness checks
/// - snapshot checkpoints
/// - dropping stale realtime updates
///
/// Invalid assumptions:
/// - positions are gapless
/// - positions identify the Nth event in a stream
/// - callers can safely add `1`, subtract, or apply modulo arithmetic
///
/// Storage adapters decide how to map this value. EventStoreDB can map it to a
/// stream revision. JetStream can map it to the last subject sequence. Both are
/// valid because this type promises ordering, not revision semantics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct StreamPosition(NonZeroU64);

impl StreamPosition {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    pub const fn try_new(value: u64) -> Result<Self, InvalidStreamPosition> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(InvalidStreamPosition { value }),
        }
    }
}

impl TryFrom<u64> for StreamPosition {
    type Error = InvalidStreamPosition;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<StreamPosition> for u64 {
    fn from(value: StreamPosition) -> Self {
        value.get()
    }
}

impl std::fmt::Display for StreamPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidStreamPosition {
    value: u64,
}

impl InvalidStreamPosition {
    pub const fn value(self) -> u64 {
        self.value
    }
}

impl std::fmt::Display for InvalidStreamPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stream position must be greater than zero, got {}", self.value)
    }
}

impl std::error::Error for InvalidStreamPosition {}
