use async_nats::jetstream;
use async_trait::async_trait;
use trogon_sandbox_contract::LaunchKey;

use super::command_custody_error::CommandCustodyError;
use super::command_owner::CommandOwner;
use super::command_record::CommandRecord;
use super::exec_error::ExecError;
use super::stored_command::StoredCommand;

/// Durable, compare-and-swap custody of keyed command launches.
#[async_trait]
pub(super) trait CommandStore: Send + Sync {
    fn is_configured(&self) -> bool;

    fn configure(&self, context: jetstream::Context) -> Result<(), ExecError>;

    async fn load(
        &self,
        owner: &CommandOwner,
        launch_key: &LaunchKey,
    ) -> Result<Option<StoredCommand>, CommandCustodyError>;

    /// `None` when the key already exists.
    async fn create(&self, record: CommandRecord) -> Result<Option<StoredCommand>, CommandCustodyError>;

    /// `None` when another writer replaced the record first.
    async fn replace(
        &self,
        expected: &StoredCommand,
        next: CommandRecord,
    ) -> Result<Option<StoredCommand>, CommandCustodyError>;
}
