use a2a_nats::agent::{A2aHandler, Bridge};
use a2a_nats::jetstream::{provision_streams_with_options, StreamProvisionOptions};
use a2a_nats::{
    A2aAgentId, A2aPrefix, AgentIdError, Config, DEFAULT_A2A_PREFIX, ENV_A2A_PREFIX, NatsConfig,
    apply_timeout_overrides, nats_connect_timeout,
};
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_std::env::ReadEnv;

const ENV_A2A_AGENT_ID: &str = "A2A_AGENT_ID";

#[derive(Debug)]
pub enum RuntimeError {
    MissingAgentId,
    InvalidAgentId(AgentIdError),
    InvalidPrefix(a2a_nats::A2aPrefixError),
    NatsConnect(trogon_nats::ConnectError),
    Provision(a2a_nats::jetstream::ProvisionError),
    Bridge(a2a_nats::agent::BridgeError),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingAgentId => write!(f, "A2A_AGENT_ID env var is required but not set"),
            Self::InvalidAgentId(e) => write!(f, "invalid agent id: {e}"),
            Self::InvalidPrefix(e) => write!(f, "invalid A2A prefix: {e}"),
            Self::NatsConnect(e) => write!(f, "NATS connection failed: {e}"),
            Self::Provision(e) => write!(f, "JetStream provisioning failed: {e}"),
            Self::Bridge(e) => write!(f, "bridge error: {e}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

pub async fn run_with_env<H, E>(handler: H, env: &E) -> Result<(), RuntimeError>
where
    H: A2aHandler,
    E: ReadEnv,
{
    let prefix_raw = env
        .var(ENV_A2A_PREFIX)
        .unwrap_or_else(|_| DEFAULT_A2A_PREFIX.to_string());
    let prefix = A2aPrefix::new(prefix_raw).map_err(RuntimeError::InvalidPrefix)?;

    let agent_id_raw = env.var(ENV_A2A_AGENT_ID).map_err(|_| RuntimeError::MissingAgentId)?;
    let agent_id = A2aAgentId::new(&agent_id_raw).map_err(RuntimeError::InvalidAgentId)?;

    let nats_config = NatsConfig::from_env(env);
    let base_config = Config::new(prefix.clone(), nats_config.clone());
    let config = apply_timeout_overrides(base_config, env);

    let connect_timeout = nats_connect_timeout(env);
    let nats_client = trogon_nats::connect(&nats_config, connect_timeout)
        .await
        .map_err(RuntimeError::NatsConnect)?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = NatsJetStreamClient::new(js_context);

    provision_streams_with_options(&js_client, &prefix, &StreamProvisionOptions::from_env(env))
        .await
        .map_err(RuntimeError::Provision)?;

    let shutdown = CancellationToken::new();
    let shutdown_for_task = shutdown.clone();

    tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        shutdown_for_task.cancel();
    });

    let bridge = Bridge::new(config, handler, nats_client, js_client);
    bridge
        .run_with_agent_id(&agent_id, shutdown)
        .await
        .map_err(RuntimeError::Bridge)?;

    info!("A2A agent shutdown complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    fn subscriber() -> impl tracing::Subscriber {
        tracing_subscriber::fmt().with_test_writer().finish()
    }

    #[test]
    fn runtime_error_display_missing_agent_id() {
        let e = RuntimeError::MissingAgentId;
        assert!(e.to_string().contains("A2A_AGENT_ID"));
    }

    #[test]
    fn runtime_error_display_invalid_agent_id() {
        let inner = A2aAgentId::new("a.b").unwrap_err();
        let e = RuntimeError::InvalidAgentId(inner);
        assert!(e.to_string().contains("invalid agent id"));
    }

    #[test]
    fn runtime_error_display_invalid_prefix() {
        let inner = A2aPrefix::new("").unwrap_err();
        let e = RuntimeError::InvalidPrefix(inner);
        assert!(e.to_string().contains("invalid A2A prefix"));
    }

    #[test]
    fn runtime_error_display_provision() {
        let inner = a2a_nats::jetstream::ProvisionError("stream: fail".into());
        let e = RuntimeError::Provision(inner);
        assert!(e.to_string().contains("JetStream provisioning failed"));
    }

    #[test]
    fn runtime_error_display_bridge() {
        let inner = a2a_nats::agent::BridgeError::Subscribe("no sub".into());
        let e = RuntimeError::Bridge(inner);
        assert!(e.to_string().contains("bridge error"));
    }

    #[test]
    fn runtime_error_implements_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(RuntimeError::MissingAgentId);
        assert!(!e.to_string().is_empty());
    }

    #[tokio::test]
    async fn run_with_env_missing_agent_id_returns_error() {
        let _guard = tracing::subscriber::set_default(subscriber());
        let env = InMemoryEnv::new();
        let result = run_with_env(StubHandler, &env).await;
        assert!(matches!(result, Err(RuntimeError::MissingAgentId)));
    }

    #[tokio::test]
    async fn run_with_env_invalid_agent_id_returns_error() {
        let _guard = tracing::subscriber::set_default(subscriber());
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_AGENT_ID, "a.b");
        let result = run_with_env(StubHandler, &env).await;
        assert!(matches!(result, Err(RuntimeError::InvalidAgentId(_))));
    }

    #[tokio::test]
    async fn run_with_env_invalid_prefix_returns_error() {
        let _guard = tracing::subscriber::set_default(subscriber());
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_AGENT_ID, "bot");
        env.set(ENV_A2A_PREFIX, "bad prefix!");
        let result = run_with_env(StubHandler, &env).await;
        assert!(matches!(result, Err(RuntimeError::InvalidPrefix(_))));
    }

    #[tokio::test]
    async fn run_with_env_defaults_to_a2a_prefix_when_env_not_set() {
        let _guard = tracing::subscriber::set_default(subscriber());
        let env = InMemoryEnv::new();
        // No A2A_AGENT_ID — proves env default path runs before the missing-id error.
        let result = run_with_env(StubHandler, &env).await;
        assert!(matches!(result, Err(RuntimeError::MissingAgentId)));
    }

    struct StubHandler;

    #[async_trait::async_trait]
    impl A2aHandler for StubHandler {
        async fn message_send(
            &self,
            _req: a2a_types::SendMessageRequest,
        ) -> Result<a2a_types::SendMessageResponse, a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::unsupported_operation("stub"))
        }

        async fn message_stream(
            &self,
            _req: a2a_types::SendMessageRequest,
        ) -> Result<(a2a_types::Task, a2a_nats::agent::TaskEventStream), a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::unsupported_operation("stub"))
        }

        async fn tasks_get(
            &self,
            _req: a2a_types::GetTaskRequest,
        ) -> Result<a2a_types::Task, a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::unsupported_operation("stub"))
        }

        async fn tasks_list(
            &self,
            _req: a2a_types::ListTasksRequest,
        ) -> Result<a2a_types::ListTasksResponse, a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::unsupported_operation("stub"))
        }

        async fn tasks_cancel(
            &self,
            _req: a2a_types::CancelTaskRequest,
        ) -> Result<a2a_types::Task, a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::unsupported_operation("stub"))
        }

        async fn tasks_resubscribe(
            &self,
            _req: a2a_types::SubscribeToTaskRequest,
        ) -> Result<a2a_types::Task, a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::unsupported_operation("stub"))
        }

        async fn push_notification_set(
            &self,
            _req: a2a_types::TaskPushNotificationConfig,
        ) -> Result<a2a_types::TaskPushNotificationConfig, a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::push_notification_not_supported("stub"))
        }

        async fn push_notification_get(
            &self,
            _req: a2a_types::GetTaskPushNotificationConfigRequest,
        ) -> Result<a2a_types::TaskPushNotificationConfig, a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::push_notification_not_supported("stub"))
        }

        async fn push_notification_list(
            &self,
            _req: a2a_types::ListTaskPushNotificationConfigsRequest,
        ) -> Result<a2a_types::ListTaskPushNotificationConfigsResponse, a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::push_notification_not_supported("stub"))
        }

        async fn push_notification_delete(
            &self,
            _req: a2a_types::DeleteTaskPushNotificationConfigRequest,
        ) -> Result<(), a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::push_notification_not_supported("stub"))
        }

        async fn agent_card(
            &self,
            _req: a2a_types::GetExtendedAgentCardRequest,
        ) -> Result<a2a_types::AgentCard, a2a_nats::agent::A2aError> {
            Err(a2a_nats::agent::A2aError::unsupported_operation("stub"))
        }
    }
}
