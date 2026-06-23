//! Default `a2a-nats-server` agent binary.
//!
//! Boots a [`NoopHandler`] that returns `UnsupportedOperation` (or
//! `PushNotificationNotSupported` for push ops) on every method — the
//! production agent author plugs in their own [`A2aExecutor`] impl by
//! depending on `a2a-nats` directly. Connect-and-serve is gated to
//! `cfg(not(coverage))` because `trogon-nats::NatsJetStreamClient` is
//! excluded during coverage runs.

#[cfg(not(coverage))]
#[tokio::main]
async fn main() {
    use a2a_nats::jetstream::{StreamProvisionOptions, provision_streams_with_options};
    use a2a_nats::nats_connect_timeout;
    use a2a_nats::server::Bridge;
    use a2a_nats_server::NoopHandler;
    use a2a_nats_server::runtime::{RuntimeError, parse_env};
    use tokio_util::sync::CancellationToken;
    use tracing::{error, info};
    use trogon_nats::jetstream::NatsJetStreamClient;
    use trogon_std::env::SystemEnv;

    tracing_subscriber::fmt::init();

    let env = SystemEnv;
    let validated = match parse_env(&env) {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "A2A agent exited with error");
            std::process::exit(1);
        }
    };

    let connect_timeout = nats_connect_timeout(&env);
    let nats_client = match trogon_nats::connect(&validated.nats_config, connect_timeout).await {
        Ok(c) => c,
        Err(e) => {
            let err = RuntimeError::NatsConnect(e);
            error!(error = %err, "A2A agent exited with error");
            std::process::exit(1);
        }
    };

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = NatsJetStreamClient::new(js_context);

    if let Err(e) =
        provision_streams_with_options(&js_client, &validated.prefix, &StreamProvisionOptions::from_env(&env)).await
    {
        let err = RuntimeError::Provision(e);
        error!(error = %err, "A2A agent exited with error");
        std::process::exit(1);
    }

    let shutdown = CancellationToken::new();
    let shutdown_for_task = shutdown.clone();
    tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        shutdown_for_task.cancel();
    });

    let bridge = Bridge::new(validated.config, NoopHandler, nats_client, js_client);
    if let Err(e) = bridge.run_with_agent_id(&validated.agent_id, shutdown).await {
        let err = RuntimeError::Bridge(e);
        error!(error = %err, "A2A agent exited with error");
        std::process::exit(1);
    }

    info!("A2A agent shutdown complete");
}

#[cfg(coverage)]
fn main() {}

#[cfg(test)]
mod tests;
