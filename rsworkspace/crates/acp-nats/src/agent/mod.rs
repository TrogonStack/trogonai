mod authenticate;
mod initialize;
mod load_session;
mod new_session;

use crate::config::Config;
use crate::nats::{FlushClient, PublishClient, RequestClient};
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, Error, ExtNotification,
    ExtRequest, ExtResponse, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    Result, SetSessionModeRequest, SetSessionModeResponse,
};
use opentelemetry::metrics::Meter;
use trogon_std::time::GetElapsed;

pub struct Bridge<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> {
    pub(crate) nats: N,
    pub(crate) clock: C,
    pub(crate) config: Config,
    pub(crate) metrics: Metrics,
}

impl<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> Bridge<N, C> {
    pub fn new(nats: N, clock: C, meter: &Meter, config: Config) -> Self {
        Self {
            nats,
            clock,
            config,
            metrics: Metrics::new(meter),
        }
    }

    pub(crate) fn nats(&self) -> &N {
        &self.nats
    }
}

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> Agent for Bridge<N, C> {
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
        _args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
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
    use agent_client_protocol::{
        Agent, CancelNotification, ExtNotification, ExtRequest, PromptRequest,
        SetSessionModeRequest,
    };
    use trogon_nats::AdvancedMockNatsClient;

    fn mock_bridge() -> Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock> {
        Bridge::new(
            AdvancedMockNatsClient::new(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
        )
    }

    fn empty_raw_value() -> Arc<serde_json::value::RawValue> {
        Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap())
    }

    #[tokio::test]
    async fn stub_methods_return_not_implemented() {
        let bridge = mock_bridge();
        let msg = "not yet implemented";

        assert!(
            bridge
                .set_session_mode(SetSessionModeRequest::new("s1", "m1"))
                .await
                .is_err()
        );
        assert!(
            bridge
                .prompt(PromptRequest::new("s1", vec![]))
                .await
                .is_err()
        );
        assert!(bridge.cancel(CancelNotification::new("s1")).await.is_err());
        assert!(
            bridge
                .ext_method(ExtRequest::new("ext", empty_raw_value()))
                .await
                .is_err()
        );
        assert!(
            bridge
                .ext_notification(ExtNotification::new("ext", empty_raw_value()))
                .await
                .is_err()
        );

        let err = bridge
            .set_session_mode(SetSessionModeRequest::new("s1", "m1"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains(msg));
    }
}
