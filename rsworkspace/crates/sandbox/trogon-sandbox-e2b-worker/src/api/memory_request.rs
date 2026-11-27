use serde::Serialize;

/// Body of the snapshot and pause calls: the memory is always kept.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct MemoryRequest {
    memory: MemoryKept,
}

/// Always serialized as `true`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(into = "bool")]
struct MemoryKept;

impl From<MemoryKept> for bool {
    fn from(_: MemoryKept) -> Self {
        true
    }
}

impl MemoryRequest {
    pub(crate) fn keep_memory() -> Self {
        Self { memory: MemoryKept }
    }
}
