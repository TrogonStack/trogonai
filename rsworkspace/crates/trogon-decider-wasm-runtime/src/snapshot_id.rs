use crate::{ModuleName, ModuleVersion};

/// Snapshot identity for one stream owned by one version of a WASM decider module.
///
/// The NATS snapshot store keys snapshots by a static [`trogon_decider_runtime::SnapshotType`]
/// name plus this identity string. A WASM module cannot contribute a static
/// type name (its identity is only known at load time), so this crate uses one
/// fixed [`crate::OpaqueSnapshotPayload`] type name and folds module identity
/// into the snapshot id instead: `{module_name}@{module_version}/{stream_id}`.
///
/// A module version bump therefore changes every snapshot id it produces. The
/// old snapshot is simply not found on the next read, and execution falls back
/// to a full replay from the beginning of the stream. No migration step is
/// required.
///
/// The rendered id is collision-free without escaping: [`ModuleName`] and
/// [`ModuleVersion`] reject `@` and `/`, so the first `@` and the first `/`
/// after it delimit the segments unambiguously, and the stream id is the
/// trailing segment where any content is safe.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WasmSnapshotId(String);

impl WasmSnapshotId {
    /// Renders the snapshot id for one stream under one module identity.
    pub fn new(module_name: &ModuleName, module_version: &ModuleVersion, stream_id: &str) -> Self {
        Self(format!("{module_name}@{module_version}/{stream_id}"))
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
