use trogon_decider_wit::host;

use crate::CommandType;

/// Load-time-validated command declaration from a module descriptor.
///
/// Mirrors the WIT `command-spec` record with the command type already parsed
/// into its domain value object, so consumers never re-validate raw descriptor
/// strings.
#[derive(Debug, Clone)]
pub struct WasmCommandSpec {
    command_type: CommandType,
    write_precondition: Option<host::WritePrecondition>,
}

impl WasmCommandSpec {
    pub(crate) fn new(command_type: CommandType, write_precondition: Option<host::WritePrecondition>) -> Self {
        Self {
            command_type,
            write_precondition,
        }
    }

    /// Returns the command type this specification declares.
    pub fn command_type(&self) -> &CommandType {
        &self.command_type
    }

    /// Returns the write precondition the module pins for this command, if any.
    pub fn write_precondition(&self) -> Option<host::WritePrecondition> {
        self.write_precondition
    }
}
