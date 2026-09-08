use trogon_decider_runtime::{SnapshotCadence, WritePrecondition};
use trogon_decider_wit::host;

use crate::CommandType;

/// Load-time-validated command declaration from a module descriptor.
///
/// Mirrors the WIT `command-spec` record with the command type already parsed
/// into its domain value object and both policies already projected onto the
/// native types the execution path uses, so consumers never re-validate raw
/// descriptor values.
#[derive(Debug, Clone)]
pub struct WasmCommandSpec {
    command_type: CommandType,
    write_precondition: WritePrecondition,
    snapshot_cadence: SnapshotCadence,
}

impl WasmCommandSpec {
    pub(crate) fn new(
        command_type: CommandType,
        write_precondition: WritePrecondition,
        snapshot_cadence: SnapshotCadence,
    ) -> Self {
        Self {
            command_type,
            write_precondition,
            snapshot_cadence,
        }
    }

    /// Returns the command type this specification declares.
    pub fn command_type(&self) -> &CommandType {
        &self.command_type
    }

    /// Returns the concurrency guard the module declares for this command.
    pub fn write_precondition(&self) -> WritePrecondition {
        self.write_precondition
    }

    /// Returns the snapshot cadence the module declares for this command.
    pub fn snapshot_cadence(&self) -> SnapshotCadence {
        self.snapshot_cadence
    }
}

/// Projects the WIT write precondition onto the domain enum the execution path resolves against.
///
/// Both types are foreign to this crate, so a `From` impl would violate the orphan rules; a plain
/// function keeps the mapping local instead.
pub(crate) fn to_write_precondition(value: host::WritePrecondition) -> WritePrecondition {
    match value {
        host::WritePrecondition::StreamUnchanged => WritePrecondition::StreamUnchanged,
        host::WritePrecondition::NoStream => WritePrecondition::NoStream,
        host::WritePrecondition::StreamExists => WritePrecondition::StreamExists,
        host::WritePrecondition::Any => WritePrecondition::Any,
    }
}

/// Projects the WIT snapshot policy onto the domain cadence.
///
/// The WIT `frequency` case carries a plain `u64` because the component model cannot express a
/// non-zero integer, so a declared zero degrades to [`SnapshotCadence::Never`] rather than
/// becoming a snapshot on every command.
pub(crate) fn to_snapshot_cadence(value: host::SnapshotPolicy) -> SnapshotCadence {
    match value {
        host::SnapshotPolicy::NoSnapshot => SnapshotCadence::Never,
        host::SnapshotPolicy::Frequency(frequency) => SnapshotCadence::every_events(frequency),
    }
}
