//! Default `a2a-nats-server` agent binary.
//!
//! Boots a [`NoopHandler`] that returns `UnsupportedOperation` (or
//! `PushNotificationNotSupported` for push ops) on every method — the
//! production agent author plugs in their own [`A2aExecutor`] impl by
//! depending on `a2a-nats` directly. Connect-and-serve is gated to
//! `cfg(not(coverage))` because `trogon-nats::NatsJetStreamClient` is
//! excluded during coverage runs.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use a2a_nats::server::{A2aError, A2aExecutor, TaskEventStream};

#[allow(dead_code)] // only constructed by the cfg(not(coverage)) main entry point
struct NoopHandler;

#[async_trait::async_trait]
impl A2aExecutor for NoopHandler {
    async fn message_send(
        &self,
        _req: a2a::types::SendMessageRequest,
    ) -> Result<a2a::types::SendMessageResponse, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn message_stream(
        &self,
        _req: a2a::types::SendMessageRequest,
    ) -> Result<(a2a::types::Task, TaskEventStream), A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_get(&self, _req: a2a::types::GetTaskRequest) -> Result<a2a::types::Task, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_list(&self, _req: a2a::types::ListTasksRequest) -> Result<a2a::types::ListTasksResponse, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_cancel(&self, _req: a2a::types::CancelTaskRequest) -> Result<a2a::types::Task, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_resubscribe(&self, _req: a2a::types::SubscribeToTaskRequest) -> Result<a2a::types::Task, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn push_notification_set(
        &self,
        _req: a2a::types::TaskPushNotificationConfig,
    ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn push_notification_get(
        &self,
        _req: a2a::types::GetTaskPushNotificationConfigRequest,
    ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn push_notification_list(
        &self,
        _req: a2a::types::ListTaskPushNotificationConfigsRequest,
    ) -> Result<a2a::types::ListTaskPushNotificationConfigsResponse, A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn push_notification_delete(
        &self,
        _req: a2a::types::DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn agent_card(
        &self,
        _req: a2a::types::GetExtendedAgentCardRequest,
    ) -> Result<a2a::agent_card::AgentCard, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }
}

#[cfg(not(coverage))]
#[tokio::main]
async fn main() {
    use a2a_nats::jetstream::{StreamProvisionOptions, provision_streams_with_options};
    use a2a_nats::nats_connect_timeout;
    use a2a_nats::server::Bridge;
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
