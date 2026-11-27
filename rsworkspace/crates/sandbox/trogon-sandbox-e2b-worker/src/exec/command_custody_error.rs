use async_nats::jetstream::kv;

use crate::custody_bucket_error::CustodyBucketError;

use super::command_owner_error::CommandOwnerError;
use super::command_record_error::CommandRecordError;
use super::failure::custody_unavailable;
use trogon_sandbox_contract::LeaseError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CommandCustodyError {
    #[error("command custody requires JetStream")]
    NotConfigured,
    #[error(transparent)]
    Bucket(#[from] CustodyBucketError),
    #[error("command custody has an unreadable authority marker")]
    UnreadableMarker,
    #[error("command custody owner, native resource or assignment changed")]
    Changed,
    #[error("command custody cannot change immutable launch authority")]
    Immutable,
    #[error(transparent)]
    Owner(#[from] CommandOwnerError),
    #[error(transparent)]
    Record(#[from] CommandRecordError),
    #[error("command custody key cannot be encoded: {0}")]
    Key(#[from] serde_json::Error),
    #[error("command custody entry is unreadable: {0}")]
    Entry(#[from] kv::EntryError),
    #[error("command custody record cannot be created: {0}")]
    Create(#[from] kv::CreateError),
    #[error("command custody record cannot be replaced: {0}")]
    Update(#[from] kv::UpdateError),
}

impl From<CommandCustodyError> for LeaseError {
    fn from(_: CommandCustodyError) -> Self {
        custody_unavailable()
    }
}
