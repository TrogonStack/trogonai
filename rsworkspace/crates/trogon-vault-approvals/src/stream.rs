//! JetStream stream provisioning for proposals.

use std::time::Duration;

use async_nats::jetstream;

use crate::error::ApprovalError;
use crate::subjects::{PROPOSALS_STREAM, stream_subjects};

/// Create or open the `VAULT_PROPOSALS` stream.
///
/// Captures `vault.proposals.*.create` (durable proposal records) and
/// `vault.proposals.*.state.*` (approve/reject state transitions) with a
/// 90-day retention window.
///
/// `vault.proposals.*.approve` and `vault.proposals.*.reject` are intentionally
/// excluded — those subjects carry plaintext API keys and must never be
/// persisted by JetStream (at-most-once, ephemeral core NATS only).
/// Status request-reply subjects (`vault.proposals.*.status.*`) are also
/// excluded so they are not persisted in the stream.
///
/// Idempotent — safe to call multiple times.
pub async fn ensure_proposals_stream(js: &jetstream::Context) -> Result<(), ApprovalError> {
    ensure_proposals_stream_with_max_age(js, Duration::from_secs(90 * 24 * 3600)).await
}

/// Like [`ensure_proposals_stream`] but with a configurable max-age.
pub async fn ensure_proposals_stream_with_max_age(
    js:      &jetstream::Context,
    max_age: Duration,
) -> Result<(), ApprovalError> {
    js.get_or_create_stream(jetstream::stream::Config {
        name:     PROPOSALS_STREAM.to_string(),
        subjects: stream_subjects(),
        storage:  jetstream::stream::StorageType::File,
        max_age,
        ..Default::default()
    })
    .await
    .map_err(|e| ApprovalError::JetStream(e.to_string()))?;
    Ok(())
}
