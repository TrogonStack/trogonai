mod authenticate;
mod cancel;
mod initialize;
mod load_session;
mod new_session;
mod prompt;
mod set_session_mode;

use crate::config::Config;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, Error, ExtNotification,
    ExtRequest, ExtResponse, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    Result, SessionNotification, SetSessionModeRequest, SetSessionModeResponse,
};
use opentelemetry::metrics::Meter;
use tokio::sync::mpsc;
use trogon_std::time::GetElapsed;

pub struct Bridge<N: RequestClient + PublishClient + SubscribeClient + FlushClient, C: GetElapsed> {
    pub(crate) nats: N,
    pub(crate) clock: C,
    pub(crate) config: Config,
    pub(crate) metrics: Metrics,
    /// Sender for ACP `session/update` notifications produced while processing a prompt.
    /// The binary wires this to an `AgentSideConnection::session_notification()` forwarding task.
    pub(crate) notification_sender: mpsc::Sender<SessionNotification>,
}

impl<N: RequestClient + PublishClient + SubscribeClient + FlushClient, C: GetElapsed> Bridge<N, C> {
    pub fn new(
        nats: N,
        clock: C,
        meter: &Meter,
        config: Config,
        notification_sender: mpsc::Sender<SessionNotification>,
    ) -> Self {
        Self {
            nats,
            clock,
            config,
            metrics: Metrics::new(meter),
            notification_sender,
        }
    }

    pub(crate) fn nats(&self) -> &N {
        &self.nats
    }
}

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + SubscribeClient + FlushClient, C: GetElapsed> Agent
    for Bridge<N, C>
{
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        initialize::handle(self, args).await
    }

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        authenticate::handle(self, args).await
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        new_session::handle(self, args).await
    }

    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        load_session::handle(self, args).await
    }

    async fn set_session_mode(
        &self,
        args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        set_session_mode::handle(self, args).await
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        prompt::handle(self, args).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        cancel::handle(self, args).await
    }

    async fn ext_method(&self, _args: ExtRequest) -> Result<ExtResponse> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn ext_notification(&self, _args: ExtNotification) -> Result<()> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Bridge;
    use crate::config::Config;
    use agent_client_protocol::{Agent, ExtNotification, ExtRequest, PromptRequest};
    use tokio::sync::mpsc;
    use trogon_nats::AdvancedMockNatsClient;

    fn mock_bridge() -> Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock> {
        let (tx, _rx) = mpsc::channel(1);
        Bridge::new(
            AdvancedMockNatsClient::new(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
            tx,
        )
    }

    fn empty_raw_value() -> Arc<serde_json::value::RawValue> {
        Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap())
    }

    #[tokio::test]
    async fn prompt_returns_error_when_subscribe_fails() {
        // AdvancedMockNatsClient.subscribe() always returns Err — so prompt returns InternalError
        let bridge = mock_bridge();
        let result = bridge.prompt(PromptRequest::new("s1", vec![])).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ext_methods_return_not_implemented() {
        let bridge = mock_bridge();
        let msg = "not yet implemented";

        let err = bridge
            .ext_method(ExtRequest::new("ext", empty_raw_value()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains(msg));

        let err = bridge
            .ext_notification(ExtNotification::new("ext", empty_raw_value()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains(msg));
    }

    #[tokio::test]
    async fn ext_methods_use_internal_error_code() {
        use agent_client_protocol::ErrorCode;

        let bridge = mock_bridge();

        macro_rules! check_internal {
            ($fut:expr) => {{
                let err = $fut.await.unwrap_err();
                assert_eq!(
                    err.code,
                    ErrorCode::InternalError.into(),
                    "stub must return InternalError, got {:?}",
                    err.code
                );
            }};
        }

        check_internal!(bridge.ext_method(ExtRequest::new("ext", empty_raw_value())));
        check_internal!(bridge.ext_notification(ExtNotification::new("ext", empty_raw_value())));
    }
}
