use trogon_decider_runtime::{
    SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType, SnapshotTypeName,
};

/// Opaque snapshot payload produced by a guest decider's `snapshot()` export.
///
/// The host never inspects these bytes; the guest owns their encoding. Every
/// WASM decider module shares one [`SnapshotType`] name because a static trait
/// method cannot see a runtime module identity. Module identity instead lives
/// in [`crate::WasmSnapshotId`], which the caller supplies as the storage key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueSnapshotPayload(Vec<u8>);

impl OpaqueSnapshotPayload {
    /// Wraps guest-produced snapshot bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns the wrapped bytes for a fresh session constructor call.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the wrapper and returns the wrapped bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl SnapshotType for OpaqueSnapshotPayload {
    type Error = std::convert::Infallible;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        Ok(SNAPSHOT_TYPE_NAME.with(|name| name.clone()))
    }
}

impl SnapshotPayloadEncode for OpaqueSnapshotPayload {
    type Error = std::convert::Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.0.clone())
    }
}

impl SnapshotPayloadDecode for OpaqueSnapshotPayload {
    type Error = std::convert::Infallible;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        Ok(Self(payload.payload.to_vec()))
    }
}

thread_local! {
    static SNAPSHOT_TYPE_NAME: SnapshotTypeName = {
        #[allow(
            clippy::unwrap_used,
            reason = "fixed non-empty ASCII literal without control characters; construction cannot fail"
        )]
        SnapshotTypeName::new("wasm-decider-opaque.v1").unwrap()
    };
}

#[cfg(test)]
mod tests;
