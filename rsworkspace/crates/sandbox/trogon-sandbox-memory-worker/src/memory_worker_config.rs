use trogon_sandbox_contract::ProviderKind;

use crate::CommandTable;

/// What a [`MemoryWorker`](crate::MemoryWorker) answers and runs.
pub struct MemoryWorkerConfig {
    /// The provider kind whose subjects this worker answers.
    pub kind: ProviderKind,
    /// The commands its sandboxes can run.
    pub commands: CommandTable,
}

impl Default for MemoryWorkerConfig {
    fn default() -> Self {
        Self {
            kind: ProviderKind::Mock,
            commands: CommandTable::default(),
        }
    }
}
