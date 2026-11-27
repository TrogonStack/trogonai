#[derive(Debug, Clone, Copy, Default)]
pub struct WorkerSelectionConstraints<'a> {
    pub native_volume_bindings: &'a [trogon_sandbox_contract::NativeVolumeBinding],
    pub snapshot_input: Option<trogon_sandbox_contract::ArtifactType>,
    pub snapshot_output: Option<trogon_sandbox_contract::ArtifactType>,
    /// The lease restores a provider-native snapshot, which only a worker that
    /// advertises native snapshots can do.
    pub native_snapshot_input: bool,
}
