use async_nats::jetstream::context::{CreateKeyValueError, KeyValueError};
use async_nats::jetstream::stream::InfoError;

/// Why a custody bucket could not be opened as durable authority.
#[derive(Debug, thiserror::Error)]
pub enum CustodyBucketError {
    #[error("custody bucket unavailable: create={create}; load={load}")]
    Unavailable {
        create: CreateKeyValueError,
        load: KeyValueError,
    },
    #[error("custody bucket stream could not be inspected")]
    Inspect(#[from] InfoError),
    #[error("custody bucket must retain nonexpiring file-backed authority without eviction")]
    NotDurable,
}
