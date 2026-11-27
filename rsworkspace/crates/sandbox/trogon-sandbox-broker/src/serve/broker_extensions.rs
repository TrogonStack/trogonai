use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use tracing::warn;
use trogon_std::env::ReadEnv;

use super::constants::OPERATION_OWNER_PREFIX;
use super::entity_shadow_setup;
use super::serve_error::ServeError;
use crate::cli::Cli;
use crate::command_logs::{CommandLogRetention, JetStreamCommandLogStore};
use crate::core::BrokerCore;
use crate::entity_store::SandboxEntityMode;
use crate::generated_ulid::generated_ulid;
use crate::operations::{NatsKvOperationStore, OPERATIONS_BUCKET, Operations};

/// Adds the optional parts of the broker. Command logs, durable operations and the entity mirror
/// each fall back to running without them when their stream or bucket cannot be opened; only a bad
/// dispatch signer configuration stops the broker.
pub(super) async fn attach(
    broker: BrokerCore,
    cli: &Cli,
    env: &impl ReadEnv,
    jetstream: &jetstream::Context,
) -> Result<BrokerCore, ServeError> {
    let broker = with_command_logs(broker, cli, jetstream).await;
    let broker = with_operations(broker, cli, jetstream).await;
    let broker = broker
        .with_dispatch_signer_from_env(env)
        .map_err(|source| ServeError::DispatchSigner { source: source.into() })?;
    Ok(with_entity_shadow(broker, cli, jetstream).await)
}

async fn with_command_logs(broker: BrokerCore, cli: &Cli, jetstream: &jetstream::Context) -> BrokerCore {
    let retention = CommandLogRetention {
        max_bytes: cli.command_log_max_bytes,
        max_age: Duration::from_secs(cli.command_log_max_age_secs),
        max_chunks_per_execution: cli.command_log_max_chunks_per_execution,
        exit_max_age: Duration::from_secs(cli.command_exit_max_age_secs),
    };
    match JetStreamCommandLogStore::new(jetstream.clone(), retention, cli.jetstream_replicas).await {
        Ok(store) => broker.with_command_logs(Arc::new(store)),
        Err(error) => {
            // An operator stream already covering these subjects makes ours overlap. Run without
            // durable logs rather than refuse to start.
            warn!(error = %error, "command log stream unavailable; read_logs replies unsupported");
            broker
        }
    }
}

async fn with_operations(broker: BrokerCore, cli: &Cli, jetstream: &jetstream::Context) -> BrokerCore {
    match NatsKvOperationStore::new(jetstream.clone(), OPERATIONS_BUCKET, cli.jetstream_replicas).await {
        Ok(store) => {
            let owner = format!("{OPERATION_OWNER_PREFIX}{}", generated_ulid());
            broker.with_operations(Arc::new(Operations::new(Arc::new(store), owner)))
        }
        Err(error) => {
            // Without the bucket, pause, resume and delete still work but run inline: an
            // operation id is not remembered across a restart.
            warn!(error = %error, "operation store unavailable; lifecycle operations are not durable");
            broker
        }
    }
}

async fn with_entity_shadow(broker: BrokerCore, cli: &Cli, jetstream: &jetstream::Context) -> BrokerCore {
    if cli.entity_mode == SandboxEntityMode::Off {
        return broker;
    }
    match entity_shadow_setup::open(cli, jetstream).await {
        Ok(shadow) => broker.with_entity_shadow(Arc::new(shadow)),
        // A mirror that cannot start must not take the broker down.
        Err(error) => {
            warn!(error = %error, "sandbox entity store unavailable; running without the entity mirror");
            broker
        }
    }
}
