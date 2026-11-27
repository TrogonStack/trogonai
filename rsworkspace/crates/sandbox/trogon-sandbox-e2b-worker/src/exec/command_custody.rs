use std::sync::OnceLock;

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use tokio::sync::OnceCell;
use trogon_sandbox_contract::LaunchKey;
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use crate::custody_bucket::{ensure_durable, open_custody_bucket};

use super::command_custody_error::CommandCustodyError;
use super::command_owner::CommandOwner;
use super::command_record::CommandRecord;
use super::command_store::CommandStore;
use super::constants::{COMMAND_CUSTODY_BUCKET, MAX_CUSTODY_RECORD_BYTES};
use super::exec_error::ExecError;
use super::stored_command::StoredCommand;

/// Durable, compare-and-swap custody of keyed command launches in a JetStream bucket.
#[derive(Default)]
pub(crate) struct CommandCustody {
    context: OnceLock<jetstream::Context>,
    store: OnceCell<kv::Store>,
}

impl CommandCustody {
    fn key(owner: &CommandOwner, launch_key: &LaunchKey) -> Result<String, CommandCustodyError> {
        owner.validate()?;
        Ok(tenant_scoped_kv_key(
            &owner.tenant,
            &serde_json::to_string(&(owner.lease_id.as_str(), launch_key.as_str()))?,
        ))
    }

    pub(super) async fn create_in(
        store: &kv::Store,
        key: String,
        record: CommandRecord,
    ) -> Result<Option<StoredCommand>, CommandCustodyError> {
        match store.create(key, record.encode()?).await {
            Ok(revision) => Ok(Some(StoredCommand { record, revision })),
            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) async fn replace_in(
        store: &kv::Store,
        key: String,
        revision: u64,
        next: CommandRecord,
    ) -> Result<Option<StoredCommand>, CommandCustodyError> {
        match store.update(key, next.encode()?, revision).await {
            Ok(revision) => Ok(Some(StoredCommand { record: next, revision })),
            Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn storage(&self) -> Result<&kv::Store, CommandCustodyError> {
        let context = self.context.get().ok_or(CommandCustodyError::NotConfigured)?;
        let store = self
            .store
            .get_or_try_init(|| {
                open_custody_bucket(
                    context,
                    kv::Config {
                        bucket: COMMAND_CUSTODY_BUCKET.into(),
                        history: 1,
                        max_value_size: i32::try_from(MAX_CUSTODY_RECORD_BYTES).unwrap_or(i32::MAX),
                        ..Default::default()
                    },
                )
            })
            .await?;
        ensure_durable(store).await?;
        Ok(store)
    }
}

#[async_trait]
impl CommandStore for CommandCustody {
    fn is_configured(&self) -> bool {
        self.context.get().is_some()
    }

    fn configure(&self, context: jetstream::Context) -> Result<(), ExecError> {
        self.context
            .set(context)
            .map_err(|_| ExecError::CustodyAlreadyConfigured)
    }

    async fn load(
        &self,
        owner: &CommandOwner,
        launch_key: &LaunchKey,
    ) -> Result<Option<StoredCommand>, CommandCustodyError> {
        let Some(entry) = self.storage().await?.entry(Self::key(owner, launch_key)?).await? else {
            return Ok(None);
        };
        if entry.operation != kv::Operation::Put || entry.value.len() > MAX_CUSTODY_RECORD_BYTES {
            return Err(CommandCustodyError::UnreadableMarker);
        }
        let record: CommandRecord = serde_json::from_slice(&entry.value)?;
        record.validate()?;
        if record.owner != *owner || record.launch_key != *launch_key {
            return Err(CommandCustodyError::Changed);
        }
        Ok(Some(StoredCommand {
            record,
            revision: entry.revision,
        }))
    }

    async fn create(&self, record: CommandRecord) -> Result<Option<StoredCommand>, CommandCustodyError> {
        let key = Self::key(&record.owner, &record.launch_key)?;
        Self::create_in(self.storage().await?, key, record).await
    }

    async fn replace(
        &self,
        expected: &StoredCommand,
        next: CommandRecord,
    ) -> Result<Option<StoredCommand>, CommandCustodyError> {
        if expected.record.owner != next.owner
            || expected.record.launch_key != next.launch_key
            || expected.record.execution_id != next.execution_id
        {
            return Err(CommandCustodyError::Immutable);
        }
        let key = Self::key(&next.owner, &next.launch_key)?;
        Self::replace_in(self.storage().await?, key, expected.revision, next).await
    }
}
