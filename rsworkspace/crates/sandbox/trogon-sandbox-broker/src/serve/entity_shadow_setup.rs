use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use tracing::warn;

use crate::cli::Cli;
use crate::entity_shadow::{EntityEventSink, EntityShadow, JetStreamEntityEventSink, NoopEntityEventSink};
use crate::entity_store::{EntityStoreError, NatsKvSandboxEntityStore};

/// Opens the entity buckets and event stream. Called only when the entity mode is not `off`, so an
/// `off` broker never creates them. A missing event stream drops entity events instead of failing.
pub(super) async fn open(cli: &Cli, jetstream: &jetstream::Context) -> Result<EntityShadow, EntityStoreError> {
    let store = NatsKvSandboxEntityStore::new(jetstream.clone()).await?;
    let events: Arc<dyn EntityEventSink> =
        match JetStreamEntityEventSink::new(jetstream.clone(), cli.jetstream_replicas).await {
            Ok(sink) => Arc::new(sink),
            Err(error) => {
                warn!(error = %error, "entity event stream unavailable; entity events are dropped");
                Arc::new(NoopEntityEventSink)
            }
        };
    Ok(EntityShadow::new(
        cli.entity_mode,
        Arc::new(store),
        events,
        // A start may legitimately sit unchanged for a whole dispatch.
        Duration::from_secs(cli.provider_dispatch_timeout_secs).saturating_mul(2),
    ))
}
