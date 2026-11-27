use crate::ordinary_command_scripts::REMOVE_ROOT;
use crate::sha256_pin::Sha256Pin;
use crate::{DaytonaApi, OrdinaryCommandRuntimeError};

/// An ordinary command runtime that was installed in one sandbox, under a directory only this install owns.
#[derive(Debug, Clone)]
pub(crate) struct InstalledRuntime {
    pub(crate) root: String,
    pub(crate) helper: String,
    pub(crate) bridge: String,
    pub(crate) digest: Sha256Pin,
    pub(crate) nonce: String,
}

impl InstalledRuntime {
    /// Removes the runtime directory when this install's nonce still owns it.
    pub(crate) async fn cleanup(&self, api: &DaytonaApi, sandbox_id: &str) -> Result<(), OrdinaryCommandRuntimeError> {
        let result = api
            .run_node(sandbox_id, REMOVE_ROOT, &[&self.root, &self.nonce], None)
            .await?;
        if result == "absent" {
            Ok(())
        } else {
            Err(OrdinaryCommandRuntimeError::CleanupUnconfirmed)
        }
    }
}
