use std::error::Error;

/// Why an active-lease store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum ActiveLeaseStoreError {
    /// The NATS KV bucket rejected or failed the operation.
    #[error("{0}")]
    Nats(Box<dyn Error + Send + Sync>),
    /// A lease record could not be encoded or decoded.
    #[error(transparent)]
    Codec(#[from] serde_json::Error),
    /// A stored lease record could not be decoded, so the listing is not trustworthy.
    #[error("active lease record {key} is not a decodable lease: {source}")]
    Undecodable { key: String, source: serde_json::Error },
    /// Concurrent writers kept winning the revision check.
    #[error("active lease {operation} exhausted CAS retries")]
    CasRetriesExhausted { operation: &'static str },
    /// A replacement tried to move a record to another lease id.
    #[error("replacement changes the lease id")]
    LeaseIdChanged,
}

impl ActiveLeaseStoreError {
    pub(crate) fn nats(error: impl Error + Send + Sync + 'static) -> Self {
        Self::Nats(Box::new(error))
    }
}
