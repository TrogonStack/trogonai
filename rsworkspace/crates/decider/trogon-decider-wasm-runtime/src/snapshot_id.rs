use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::{ModuleName, ModuleVersion};

/// Snapshot identity for one stream owned by one version of a WASM decider module.
///
/// The NATS snapshot store keys snapshots by a static [`trogon_decider_runtime::SnapshotType`]
/// name plus this identity string. A WASM module cannot contribute a static
/// type name (its identity is only known at load time), so this crate uses one
/// fixed [`crate::OpaqueSnapshotPayload`] type name and folds module identity
/// into the snapshot id instead. The logical identity is
/// `{module_name}@{module_version}/{stream_id}`; the stored identity is that
/// complete value encoded as unpadded base64url with a `v1_` format prefix.
/// This keeps stream ids containing arbitrary characters within the NATS
/// Key/Value key alphabet without sacrificing identity uniqueness.
///
/// A module version bump therefore changes every snapshot id it produces. The
/// old snapshot is simply not found on the next read, and execution falls back
/// to a full replay from the beginning of the stream. Normal module-version
/// changes therefore require no snapshot migration.
///
/// The `v1_` representation replaces the earlier raw logical identity. NATS
/// could not persist that legacy form because its `@` delimiter is not valid in
/// a Key/Value key. A non-NATS store that persisted legacy ids must retain
/// replayable history or migrate those keys before adopting this format.
///
/// The logical id is collision-free before encoding: [`ModuleName`] and
/// [`ModuleVersion`] reject `@` and `/`, so the first `@` and the first `/`
/// after it delimit the segments unambiguously, and the stream id is the
/// trailing segment where any content is safe.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WasmSnapshotId(String);

impl WasmSnapshotId {
    /// Renders the snapshot id for one stream under one module identity.
    pub fn new(module_name: &ModuleName, module_version: &ModuleVersion, stream_id: &str) -> Self {
        let logical_identity = format!("{module_name}@{module_version}/{stream_id}");
        let mut snapshot_id = String::from("v1_");
        URL_SAFE_NO_PAD.encode_string(logical_identity, &mut snapshot_id);
        Self(snapshot_id)
    }

    /// Returns the rendered snapshot id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WasmSnapshotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for WasmSnapshotId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for WasmSnapshotId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests;
