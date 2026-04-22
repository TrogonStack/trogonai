//! Dead-letter queue (DLQ) for events the router cannot route.
//!
//! When the LLM returns `Unroutable`, the router publishes the original event
//! payload to `trogon.unroutable.{event_type}` so that an operator or a
//! monitoring consumer can inspect and replay unroutable events.
//!
//! # Infrastructure
//!
//! Call [`provision_unroutable_stream`] once at startup (in `main.rs`) to
//! create the backing JetStream stream.  Once provisioned, any NATS `publish`
//! to a subject matching `trogon.unroutable.>` is durably captured.

use async_nats::jetstream::stream::{Config as StreamConfig, StorageType};

/// JetStream stream that captures all unroutable-event messages.
pub const UNROUTABLE_STREAM: &str = "UNROUTABLE_EVENTS";

/// NATS subject prefix for dead-lettered events.
///
/// The router publishes to `{UNROUTABLE_SUBJECT_PREFIX}.{event_type}`, e.g.
/// `trogon.unroutable.github.pull_request`.
pub const UNROUTABLE_SUBJECT_PREFIX: &str = "trogon.unroutable";

/// Create (or open) the `UNROUTABLE_EVENTS` JetStream stream.
///
/// The stream captures every message published to `trogon.unroutable.>` and
/// retains them for 7 days.  Calling this function when the stream already
/// exists is a no-op.
pub async fn provision_unroutable_stream(
    js: &async_nats::jetstream::Context,
) -> Result<(), String> {
    let config = StreamConfig {
        name: UNROUTABLE_STREAM.to_string(),
        subjects: vec!["trogon.unroutable.>".to_string()],
        storage: StorageType::File,
        max_age: std::time::Duration::from_secs(7 * 24 * 3600),
        ..Default::default()
    };

    match js.create_stream(config).await {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = e.to_string();
            // Treat "stream name already in use" as success (idempotent).
            if msg.contains("stream name already in use")
                || msg.contains("already exists")
                || msg.contains("STREAM_NAME_EXIST")
            {
                Ok(())
            } else {
                Err(format!("failed to provision {UNROUTABLE_STREAM}: {msg}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_consistent() {
        assert!(UNROUTABLE_SUBJECT_PREFIX.starts_with("trogon."));
        assert!(!UNROUTABLE_STREAM.is_empty());
    }
}
