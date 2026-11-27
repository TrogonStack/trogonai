use async_nats::jetstream::context::KeyValueError;
use async_nats::jetstream::kv::EntryError;

/// Failures of resolving a secret reference to its bytes.
#[derive(Debug, thiserror::Error)]
pub enum K8sSecretError {
    #[error("NatsKv SecretRef requires a NATS JetStream context; call with_nats() on K8sMicroVmProvider")]
    JetStreamRequired,
    #[error("failed to open KV bucket {bucket}")]
    OpenBucket {
        bucket: String,
        #[source]
        source: KeyValueError,
    },
    #[error("failed to fetch secret key {key:?}")]
    Fetch {
        key: String,
        #[source]
        source: EntryError,
    },
    #[error("secret key {key:?} not found in bucket {bucket}")]
    NotFound { key: String, bucket: String },
}
